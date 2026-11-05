//! Session state machine: turns hook events into per-session states and picks what the buddy shows.

use crate::i18n::t;
use serde_json::{json, Value};
use std::collections::HashMap;

const MAX_STEPS: usize = 6;
const MAX_TEXT: usize = 120;
const DONE_DECAY_MS: u64 = 5 * 60_000;
const WORKING_STALE_MS: u64 = 15 * 60_000;
const STALE_MS: u64 = 30 * 60_000;
/// Turns shorter than this finish silently: a sound for every one-liner would be noise.
const MIN_WORK_MS_FOR_NOTIFY: u64 = 10_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    Working,
    Waiting,
    Done,
    Error,
}

impl State {
    pub fn parse(s: &str) -> Option<State> {
        Some(match s {
            "idle" => State::Idle,
            "working" => State::Working,
            "waiting" => State::Waiting,
            "done" => State::Done,
            "error" => State::Error,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Working => "working",
            State::Waiting => "waiting",
            State::Done => "done",
            State::Error => "error",
        }
    }

    /// Higher wins when several sessions compete for the character.
    fn priority(self) -> u8 {
        match self {
            State::Idle => 0,
            State::Done => 1,
            State::Working => 2,
            State::Error => 3,
            State::Waiting => 4,
        }
    }

    pub fn needs_attention(self) -> bool {
        matches!(self, State::Done | State::Waiting | State::Error)
    }
}

#[derive(Default, Clone, Debug)]
pub struct Event {
    pub session_id: String,
    pub event: String,
    pub state: Option<String>,
    pub cwd: Option<String>,
    pub notification_type: Option<String>,
    pub message: Option<String>,
    pub transcript: Option<String>,
    pub title: Option<String>,
    pub task: Option<String>,
    pub activity: Option<String>,
    pub summary: Option<String>,
    pub step: Option<String>,
    pub source: Option<String>,
    pub label: Option<String>,
    pub url: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Step {
    pub name: String,
    pub status: StepStatus,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum StepStatus {
    Running,
    Done,
    Fail,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub label: String,
    pub source: String,
    pub state: State,
    pub updated_at: u64,
    pub cwd: Option<String>,
    pub pid: Option<u32>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub task: Option<String>,
    pub summary: Option<String>,
    pub message: Option<String>,
    pub activity: Option<String>,
    pub step: Option<String>,
    pub steps: Vec<Step>,
    pub ended: bool,
    pub work_started_at: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub session: Session,
    pub to: State,
    pub notify: bool,
}

enum Target {
    Ignore,
    End,
    To(State),
}

fn target_for(ev: &Event) -> Result<Target, String> {
    if let Some(s) = &ev.state {
        return State::parse(s).map(Target::To).ok_or_else(|| format!("unknown state: {s}"));
    }
    Ok(match ev.event.as_str() {
        "SessionStart" => Target::To(State::Idle),
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => Target::To(State::Working),
        "Notification" => match ev.notification_type.as_deref() {
            None | Some("permission_prompt") | Some("elicitation_dialog") => Target::To(State::Waiting),
            _ => Target::Ignore,
        },
        "Stop" => Target::To(State::Done),
        "StopFailure" => Target::To(State::Error),
        "SessionEnd" => Target::End,
        other => return Err(format!("unknown event: {other}")),
    })
}

fn label_for(ev: &Event) -> String {
    if let Some(l) = &ev.label {
        return l.clone();
    }
    if let Some(last) = ev.cwd.as_deref().and_then(|c| c.split(['\\', '/']).filter(|p| !p.is_empty()).last()) {
        return last.to_string();
    }
    ev.session_id.chars().take(8).collect()
}

/// Collapses whitespace and cuts to `max` characters; None when nothing is left.
pub fn text(v: Option<&str>, max: usize) -> Option<String> {
    let t = v?.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return None;
    }
    if t.chars().count() > max {
        let cut: String = t.chars().take(max.saturating_sub(1)).collect();
        return Some(cut + "…");
    }
    Some(t)
}

/// A new step closes the previous one, so the history reads implement → test → fix.
fn update_steps(s: &mut Session, target: State, step: Option<String>) {
    let name = step.clone().or_else(|| s.step.clone());
    if target == State::Working {
        if let Some(step) = step {
            if let Some(last) = s.steps.last_mut() {
                if last.status == StepStatus::Running && last.name != step {
                    last.status = StepStatus::Done;
                }
            }
            let same_running = s.steps.last().is_some_and(|l| l.status == StepStatus::Running && l.name == step);
            if !same_running {
                s.steps.push(Step { name: step.clone(), status: StepStatus::Running });
            }
            s.step = Some(step);
        }
    } else if matches!(target, State::Done | State::Error) {
        if let Some(name) = name {
            let status = if target == State::Done { StepStatus::Done } else { StepStatus::Fail };
            match s.steps.last_mut() {
                Some(last) if last.name == name => last.status = status,
                _ => s.steps.push(Step { name: name.clone(), status }),
            }
            s.step = Some(name);
        }
    }
    if s.steps.len() > MAX_STEPS {
        let extra = s.steps.len() - MAX_STEPS;
        s.steps.drain(..extra);
    }
}

#[derive(Default)]
pub struct Store {
    sessions: HashMap<String, Session>,
}

impl Store {
    pub fn apply(&mut self, ev: &Event, now: u64) -> Result<Option<Transition>, String> {
        if ev.session_id.is_empty() {
            return Err("session_id is required".into());
        }
        let target = target_for(ev)?;
        if let Target::End = target {
            let Some(existing) = self.sessions.get_mut(&ev.session_id) else { return Ok(None) };
            match existing.state {
                // Closed or killed while Claude was still at it: that is worth knowing.
                State::Working => return Ok(self.fail(&ev.session_id, t("Phiên kết thúc khi Claude đang chạy", "The session ended while Claude was working"), now)),
                // Headless runs send Stop and SessionEnd back to back; keep the result visible.
                State::Done | State::Waiting | State::Error => existing.ended = true,
                State::Idle => {
                    self.sessions.remove(&ev.session_id);
                }
            }
            return Ok(None);
        }

        let from = self.sessions.get(&ev.session_id).map(|s| s.state);
        let s = self.sessions.entry(ev.session_id.clone()).or_insert_with(|| Session {
            id: ev.session_id.clone(),
            label: label_for(ev),
            source: ev.source.clone().filter(|x| !x.is_empty()).unwrap_or_else(|| "cli".into()),
            state: State::Idle,
            updated_at: now,
            cwd: None,
            pid: None,
            url: None,
            title: None,
            task: None,
            summary: None,
            message: None,
            activity: None,
            step: None,
            steps: Vec::new(),
            ended: false,
            work_started_at: None,
        });
        if let Some(l) = &ev.label {
            s.label = l.clone();
        }
        if let Some(src) = ev.source.as_ref().filter(|x| !x.is_empty()) {
            s.source = src.clone();
        }
        if let Some(c) = ev.cwd.as_ref().filter(|x| !x.is_empty()) {
            s.cwd = Some(c.clone());
        }
        if let Some(p) = ev.pid.filter(|p| *p > 0) {
            s.pid = Some(p);
        }
        if let Some(u) = &ev.url {
            let lower = u.to_ascii_lowercase();
            if u.len() <= 2000 && (lower.starts_with("http://") || lower.starts_with("https://")) {
                s.url = Some(u.clone());
            }
        }
        if let Some(t) = text(ev.title.as_deref(), 80) {
            s.title = Some(t);
        }
        if let Some(t) = text(ev.task.as_deref(), MAX_TEXT) {
            s.task = Some(t);
            s.summary = None;
        }
        s.updated_at = now;

        let Target::To(target) = target else { return Ok(None) };
        s.ended = false;
        if target == State::Working && s.state != State::Working && s.state != State::Waiting {
            s.work_started_at = Some(now);
        }
        s.state = target;
        s.message = text(ev.message.as_deref(), 300);
        update_steps(s, target, text(ev.step.as_deref(), 60));
        s.activity = if target == State::Working {
            text(ev.activity.as_deref(), 80).or_else(|| if from == Some(State::Working) { s.activity.clone() } else { None })
        } else {
            None
        };
        if target == State::Done {
            s.summary = text(ev.summary.as_deref(), MAX_TEXT);
        }
        let notify = should_notify(ev, s, from, target, now);
        Ok(Some(Transition { session: s.clone(), to: target, notify }))
    }

    /// Marks a running session as failed (e.g. its Claude process disappeared).
    pub fn fail(&mut self, id: &str, message: &str, now: u64) -> Option<Transition> {
        let s = self.sessions.get_mut(id)?;
        if s.state != State::Working && s.state != State::Waiting {
            return None;
        }
        s.state = State::Error;
        s.message = Some(message.to_string());
        s.activity = None;
        s.ended = true; // nothing will report on it any more
        s.updated_at = now;
        update_steps(s, State::Error, None);
        Some(Transition { session: s.clone(), to: State::Error, notify: true })
    }

    /// Marks a done/waiting/error session as seen. Returns true if anything changed.
    pub fn acknowledge(&mut self, id: &str) -> bool {
        let Some(s) = self.sessions.get_mut(id) else { return false };
        if !s.state.needs_attention() {
            return false;
        }
        if s.ended {
            self.sessions.remove(id);
            return true;
        }
        s.state = State::Idle;
        s.message = None;
        true
    }

    /// Time-based housekeeping. Returns true if anything changed.
    pub fn tick(&mut self, now: u64) -> bool {
        let mut changed = false;
        self.sessions.retain(|_, s| {
            let age = now.saturating_sub(s.updated_at);
            if age > STALE_MS || (s.ended && age > DONE_DECAY_MS && s.state != State::Error) {
                changed = true;
                return false;
            }
            if s.state == State::Done && age > DONE_DECAY_MS {
                s.state = State::Idle;
                s.message = None;
                changed = true;
            } else if s.state == State::Working && age > WORKING_STALE_MS {
                // An interrupted turn (Esc) never sends Stop; don't spin forever.
                s.state = State::Idle;
                s.activity = None;
                changed = true;
            }
            true
        });
        changed
    }

    pub fn list(&self) -> Vec<Session> {
        let mut v: Vec<Session> = self.sessions.values().cloned().collect();
        v.sort_by(|a, b| b.state.priority().cmp(&a.state.priority()).then(b.updated_at.cmp(&a.updated_at)));
        v
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// Sessions a live process should still be behind: (id, pid).
    pub fn running_pids(&self) -> Vec<(String, u32)> {
        self.sessions
            .values()
            .filter(|s| matches!(s.state, State::Working | State::Waiting))
            .filter_map(|s| s.pid.map(|p| (s.id.clone(), p)))
            .collect()
    }
}

fn should_notify(ev: &Event, s: &Session, from: Option<State>, to: State, now: u64) -> bool {
    if from == Some(to) && !(ev.step.is_some() && ev.state.is_some()) {
        return false;
    }
    match to {
        State::Error => true,
        State::Done | State::Waiting => {
            let short_turn = s.work_started_at.is_some_and(|w| now.saturating_sub(w) < MIN_WORK_MS_FOR_NOTIFY);
            !(to == State::Done && ev.state.is_none() && short_turn)
        }
        _ => false,
    }
}

pub fn overall(sessions: &[Session]) -> State {
    sessions.first().map_or(State::Idle, |s| s.state)
}

pub fn status_of(s: &Session) -> String {
    let step = s.step.as_deref();
    match s.state {
        State::Working => match (step, s.activity.as_deref()) {
            (Some(st), _) => format!("{st}…"),
            (None, Some(a)) => format!("{a}…"),
            _ => t("đang chạy…", "working…").into(),
        },
        State::Waiting => step.map_or(t("cần bạn", "needs you").into(), |st| crate::tf!("{st} · cần bạn", "{st} · needs you")),
        State::Done => step.map_or(t("xong ✓", "done ✓").into(), |st| crate::tf!("{st} xong ✓", "{st} done ✓")),
        State::Error => step.map_or(t("lỗi ✗", "failed ✗").into(), |st| crate::tf!("{st} lỗi ✗", "{st} failed ✗")),
        State::Idle => t("rảnh", "idle").into(),
    }
}

/// Label plus the session name, so sessions of the same project can be told apart.
pub fn name_of(s: &Session) -> String {
    const TITLE_IN_LINE: usize = 24;
    match &s.title {
        Some(t) if *t != s.label => {
            if t.chars().count() > TITLE_IN_LINE {
                let cut: String = t.chars().take(TITLE_IN_LINE - 1).collect();
                format!("{} · {}…", s.label, cut.trim_end())
            } else {
                format!("{} · {t}", s.label)
            }
        }
        _ => s.label.clone(),
    }
}

pub fn detail_of(s: &Session) -> Option<String> {
    match s.state {
        State::Waiting => Some(s.message.clone().unwrap_or_else(|| t("Claude đang chờ bạn phản hồi", "Claude is waiting for your reply").into())),
        State::Error => Some(s.message.clone().unwrap_or_else(|| t("Phiên gặp lỗi hoặc bị dừng", "The session failed or was stopped").into())),
        State::Done => Some(s.summary.clone().or_else(|| s.message.clone()).unwrap_or_else(|| t("Claude đã hoàn thành lượt làm việc", "Claude finished its turn").into())),
        State::Working => {
            if s.step.is_some() && s.activity.is_some() {
                s.activity.clone()
            } else {
                s.task.clone().or_else(|| s.message.clone())
            }
        }
        State::Idle => None,
    }
}

/// Finished steps before the current one: "✓ implement · ✗ test".
pub fn history_of(s: &Session) -> Option<String> {
    let n = s.steps.len();
    let past: Vec<String> = s
        .steps
        .iter()
        .enumerate()
        .filter(|(i, x)| !(*i + 1 == n && (x.status == StepStatus::Running || matches!(s.state, State::Done | State::Error))))
        .map(|(_, x)| {
            let mark = match x.status {
                StepStatus::Fail => "✗",
                StepStatus::Done => "✓",
                StepStatus::Running => "…",
            };
            format!("{mark} {}", x.name)
        })
        .collect();
    (!past.is_empty()).then(|| past.join(" · "))
}

pub fn sessions_json(sessions: &[Session]) -> Value {
    Value::Array(
        sessions
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "label": s.label,
                    "title": s.title,
                    "source": s.source,
                    "state": s.state.name(),
                    "status": status_of(s),
                    "detail": detail_of(s),
                    "history": history_of(s),
                    "cwd": s.cwd,
                    "pid": s.pid,
                    "updatedAt": s.updated_at,
                })
            })
            .collect(),
    )
}

pub fn toast_text(s: &Session, to: State) -> (String, String) {
    let title = match to {
        State::Waiting => t("Claude cần bạn", "Claude needs you"),
        State::Done => t("Claude đã xong", "Claude is done"),
        State::Error => t("Claude gặp lỗi", "Claude hit an error"),
        _ => "Claude Buddy",
    };
    let name = name_of(s);
    let body = match detail_of(s) {
        Some(d) if !d.is_empty() => format!("{name}\n{d}"),
        _ => name,
    };
    (title.to_string(), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, event: &str) -> Event {
        Event { session_id: id.into(), event: event.into(), ..Default::default() }
    }

    #[test]
    fn maps_hook_events_to_states() {
        let mut st = Store::default();
        st.apply(&ev("a", "SessionStart"), 0).unwrap();
        assert_eq!(st.get("a").unwrap().state, State::Idle);
        st.apply(&ev("a", "PreToolUse"), 1).unwrap();
        assert_eq!(st.get("a").unwrap().state, State::Working);
        st.apply(&ev("a", "Stop"), 2).unwrap();
        assert_eq!(st.get("a").unwrap().state, State::Done);
        st.apply(&ev("a", "StopFailure"), 3).unwrap();
        assert_eq!(st.get("a").unwrap().state, State::Error);
    }

    #[test]
    fn rejects_bad_events() {
        let mut st = Store::default();
        assert!(st.apply(&ev("", "Stop"), 0).is_err());
        assert!(st.apply(&ev("a", "Nope"), 0).is_err());
        let mut bad = ev("a", "Stop");
        bad.state = Some("sleeping".into());
        assert!(st.apply(&bad, 0).is_err());
    }

    #[test]
    fn only_blocking_notifications_mean_waiting() {
        let mut st = Store::default();
        st.apply(&ev("a", "PreToolUse"), 0).unwrap();
        let mut idle_prompt = ev("a", "Notification");
        idle_prompt.notification_type = Some("idle_prompt".into());
        assert!(st.apply(&idle_prompt, 1).unwrap().is_none());
        assert_eq!(st.get("a").unwrap().state, State::Working);
        let mut perm = ev("a", "Notification");
        perm.notification_type = Some("permission_prompt".into());
        assert_eq!(st.apply(&perm, 2).unwrap().unwrap().to, State::Waiting);
    }

    #[test]
    fn session_end_while_working_is_an_error() {
        let mut st = Store::default();
        st.apply(&ev("a", "PreToolUse"), 0).unwrap();
        let t = st.apply(&ev("a", "SessionEnd"), 1).unwrap().unwrap();
        assert_eq!(t.to, State::Error);
        assert!(t.notify);
        assert!(st.get("a").unwrap().ended);
    }

    #[test]
    fn session_end_keeps_done_until_acknowledged_and_drops_idle() {
        let mut st = Store::default();
        st.apply(&ev("a", "Stop"), 0).unwrap();
        st.apply(&ev("a", "SessionEnd"), 1).unwrap();
        assert!(st.get("a").is_some());
        assert!(st.acknowledge("a"));
        assert!(st.get("a").is_none());

        st.apply(&ev("b", "SessionStart"), 0).unwrap();
        st.apply(&ev("b", "SessionEnd"), 1).unwrap();
        assert!(st.get("b").is_none());
    }

    #[test]
    fn short_turns_finish_silently() {
        let mut st = Store::default();
        st.apply(&ev("a", "UserPromptSubmit"), 0).unwrap();
        assert!(!st.apply(&ev("a", "Stop"), 2_000).unwrap().unwrap().notify);
        st.apply(&ev("a", "UserPromptSubmit"), 10_000).unwrap();
        assert!(st.apply(&ev("a", "Stop"), 30_000).unwrap().unwrap().notify);
    }

    #[test]
    fn tick_decays_and_expires() {
        let mut st = Store::default();
        st.apply(&ev("done", "Stop"), 0).unwrap();
        st.apply(&ev("work", "PreToolUse"), 0).unwrap();
        assert!(!st.tick(1_000));
        assert!(st.tick(DONE_DECAY_MS + 1));
        assert_eq!(st.get("done").unwrap().state, State::Idle);
        assert_eq!(st.get("work").unwrap().state, State::Working);
        assert!(st.tick(WORKING_STALE_MS + 1));
        assert_eq!(st.get("work").unwrap().state, State::Idle);
        assert!(st.tick(STALE_MS + 1));
        assert!(st.list().is_empty());
    }

    #[test]
    fn waiting_outranks_everything() {
        let mut st = Store::default();
        st.apply(&ev("w", "PreToolUse"), 5).unwrap();
        st.apply(&ev("d", "Stop"), 6).unwrap();
        let mut n = ev("n", "Notification");
        n.notification_type = Some("permission_prompt".into());
        st.apply(&n, 1).unwrap();
        let l = st.list();
        assert_eq!(l[0].id, "n");
        assert_eq!(overall(&l), State::Waiting);
        assert_eq!(l[1].id, "w");
    }

    #[test]
    fn steps_build_a_history() {
        let mut st = Store::default();
        let step = |name: &str, state: &str| {
            let mut e = ev("a", "x");
            e.step = Some(name.into());
            e.state = Some(state.into());
            e
        };
        st.apply(&step("implement", "working"), 0).unwrap();
        st.apply(&step("test", "working"), 1).unwrap();
        st.apply(&step("test", "error"), 2).unwrap();
        let s = st.get("a").unwrap();
        assert_eq!(status_of(s), "test lỗi ✗");
        assert_eq!(history_of(s).unwrap(), "✓ implement");
    }

    #[test]
    fn text_truncates_on_char_boundaries() {
        assert_eq!(text(Some("  a \n b "), 10).unwrap(), "a b");
        assert_eq!(text(Some("   "), 10), None);
        assert_eq!(text(Some("đang chạy rất lâu"), 5).unwrap(), "đang…");
    }

    #[test]
    fn labels_come_from_the_working_folder() {
        let mut st = Store::default();
        let mut e = ev("abcdefghijkl", "SessionStart");
        e.cwd = Some("C:\\Users\\me\\proj-x\\".into());
        st.apply(&e, 0).unwrap();
        assert_eq!(st.get("abcdefghijkl").unwrap().label, "proj-x");
        st.apply(&ev("zzzzzzzzzzzz", "SessionStart"), 0).unwrap();
        assert_eq!(st.get("zzzzzzzzzzzz").unwrap().label, "zzzzzzzz");
    }
}
