//! Local-only HTTP endpoint, plus the mapping from raw Claude Code hook input to an Event.
//! POST /hook always answers 200 with an empty body, so a hook can never disturb Claude Code.

use crate::i18n::t;
use crate::store::{sessions_json, text, Event, Store, Transition};
use crate::usage;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_HEADER: usize = 16 * 1024;
const MAX_EVENT_BODY: usize = 16 * 1024;
/// Raw hook input can carry big tool inputs (e.g. a Write of a large file); only a few fields are kept.
pub const MAX_HOOK_BODY: usize = 8 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 32;
const TITLE_TAIL_BYTES: u64 = 256 * 1024;
const TITLE_INTERVAL_MS: u64 = 15_000;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// State shared between the server threads and the UI thread.
#[derive(Default)]
pub struct Shared {
    pub store: Mutex<Store>,
    /// Transitions the UI thread has not announced yet.
    pub pending: Mutex<Vec<Transition>>,
    /// Footer message for the panel (e.g. a window that could not be found).
    pub note: Mutex<Option<String>>,
    /// HWND to wake up when something changed.
    pub hwnd: AtomicIsize,
    /// Tokens per day, replaced whenever a usage scan finishes.
    pub usage: Mutex<usage::Days>,
    pub scanning: AtomicBool,
    /// At least one scan has finished, so the numbers mean something.
    pub scanned: AtomicBool,
    titles: Mutex<HashMap<String, u64>>,
    connections: AtomicUsize,
}

fn str_field<'a>(j: &'a Value, k: &str) -> Option<&'a str> {
    j.get(k).and_then(Value::as_str)
}

fn owned(j: &Value, k: &str) -> Option<String> {
    str_field(j, k).map(str::to_string)
}

fn first_line(v: Option<&str>) -> String {
    v.unwrap_or("")
        .lines()
        .map(|l| l.trim_start_matches(['#', '>', '*', '-', '`', ' ', '\t']).trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

fn base(p: Option<&str>) -> &str {
    p.unwrap_or("").rsplit(['\\', '/']).next().unwrap_or("")
}

/// Human-readable "what Claude is doing" for a tool call, e.g. "chạy npm test", "sửa main.rs".
pub fn activity_for(tool: &str, input: &Value) -> Option<String> {
    let f = |k| str_field(input, k);
    Some(match tool {
        "Bash" | "PowerShell" => {
            let d = first_line(f("description"));
            crate::tf!("chạy {}", "running {}", if d.is_empty() { first_line(f("command")) } else { d })
        }
        "Edit" | "MultiEdit" | "Write" => crate::tf!("sửa {}", "editing {}", base(f("file_path"))),
        "NotebookEdit" => crate::tf!("sửa {}", "editing {}", base(f("notebook_path").or(f("file_path")))),
        "Read" => crate::tf!("đọc {}", "reading {}", base(f("file_path"))),
        "Grep" | "Glob" => crate::tf!("tìm {}", "searching {}", f("pattern").unwrap_or("")),
        "WebFetch" | "WebSearch" => t("tra cứu web", "searching the web").into(),
        "Task" | "Agent" => crate::tf!("giao việc: {}", "delegating: {}", first_line(f("description"))),
        "TodoWrite" => {
            let todos = input.get("todos").and_then(Value::as_array)?;
            let done = todos.iter().filter(|x| str_field(x, "status") == Some("completed")).count();
            let doing = todos.iter().find(|x| str_field(x, "status") == Some("in_progress"))?;
            let form = first_line(str_field(doing, "activeForm"));
            let name = if form.is_empty() { first_line(str_field(doing, "content")) } else { form };
            format!("{name} ({done}/{})", todos.len())
        }
        "" => return None,
        other => other.strip_prefix("mcp__").unwrap_or(other).replace("__", ": "),
    })
}

fn stop_error(kind: &str) -> String {
    match kind {
        "rate_limit" => t("Hết hạn mức sử dụng (rate limit)", "Rate limit reached").into(),
        "overloaded" => t("Máy chủ Claude đang quá tải", "Claude's servers are overloaded").into(),
        "authentication_failed" => t("Lỗi đăng nhập / xác thực", "Sign-in or authentication error").into(),
        "billing_error" => t("Lỗi thanh toán", "Billing error").into(),
        "server_error" => t("Lỗi máy chủ Claude", "Claude server error").into(),
        "" => t("Lượt chạy bị lỗi", "The turn failed").into(),
        other => crate::tf!("Lỗi: {other}", "Error: {other}"),
    }
}

/// Messages Claude Code injects as a prompt (<task-notification>…) are not the user's request.
fn is_injected_prompt(p: &str) -> bool {
    let Some(rest) = p.trim_start().strip_prefix('<') else { return false };
    let Some((tag, _)) = rest.split_once('>') else { return false };
    tag.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && tag.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// Header set by the hook from an env var; empty or unexpanded values count as absent.
fn header_value(headers: &HashMap<String, String>, name: &str, max: usize) -> Option<String> {
    let v = headers.get(name)?.trim();
    (!v.is_empty() && !v.starts_with(['%', '$'])).then(|| v.chars().take(max).collect())
}

pub fn hook_to_event(j: &Value, headers: &HashMap<String, String>) -> Option<Event> {
    let session_id = str_field(j, "session_id").filter(|s| !s.is_empty())?;
    let name = str_field(j, "hook_event_name")?;
    let mut ev = Event {
        session_id: session_id.into(),
        event: name.into(),
        cwd: owned(j, "cwd"),
        notification_type: owned(j, "notification_type"),
        transcript: owned(j, "transcript_path"),
        title: owned(j, "session_title"),
        ..Default::default()
    };
    match name {
        "Notification" => ev.message = str_field(j, "message").and_then(|m| text(Some(m), 300)),
        "UserPromptSubmit" => {
            let prompt = str_field(j, "prompt").unwrap_or("");
            if !is_injected_prompt(prompt) {
                ev.task = text(Some(&first_line(Some(prompt))), 120);
            }
            ev.activity = Some(t("đang suy nghĩ", "thinking").into());
        }
        "PreToolUse" => {
            let input = j.get("tool_input").cloned().unwrap_or(Value::Null);
            ev.activity = activity_for(str_field(j, "tool_name").unwrap_or(""), &input).and_then(|a| text(Some(&a), 80));
        }
        "Stop" => ev.summary = text(Some(&first_line(str_field(j, "last_assistant_message"))), 120),
        "StopFailure" => {
            let head = stop_error(str_field(j, "error").unwrap_or(""));
            let details = str_field(j, "error_details").unwrap_or("");
            let joined = if details.is_empty() { head } else { format!("{head} · {details}") };
            ev.message = text(Some(&joined), 300);
        }
        _ => {}
    }
    ev.source = header_value(headers, "x-buddy-source", 100);
    ev.label = header_value(headers, "x-buddy-label", 100);
    ev.url = header_value(headers, "x-buddy-url", 2000);
    ev.pid = header_value(headers, "x-buddy-pid", 12).and_then(|p| p.parse().ok());
    Some(ev)
}

/// A BuddyEvent POSTed by another local tool.
pub fn event_from_json(j: &Value) -> Result<Event, String> {
    if !j.is_object() {
        return Err("expected a JSON object".into());
    }
    Ok(Event {
        session_id: owned(j, "session_id").unwrap_or_default(),
        event: owned(j, "event").unwrap_or_default(),
        state: owned(j, "state"),
        cwd: owned(j, "cwd"),
        notification_type: owned(j, "notification_type"),
        message: owned(j, "message"),
        transcript: owned(j, "transcript"),
        title: owned(j, "title"),
        task: owned(j, "task"),
        activity: owned(j, "activity"),
        summary: owned(j, "summary"),
        step: owned(j, "step"),
        source: owned(j, "source"),
        label: owned(j, "label"),
        url: owned(j, "url"),
        pid: j.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()),
    })
}

/// Latest name set with /rename or --name wins over the latest generated one.
pub fn title_from_transcript_tail(tail: &str) -> Option<String> {
    let (mut custom, mut ai) = (None, None);
    for line in tail.lines().filter(|l| l.contains("-title\"")) {
        // The first line of the tail is usually cut off.
        let Ok(j) = serde_json::from_str::<Value>(line) else { continue };
        match str_field(&j, "type") {
            Some("custom-title") => custom = owned(&j, "customTitle").or(custom),
            Some("ai-title") => ai = owned(&j, "aiTitle").or(ai),
            _ => {}
        }
    }
    text(custom.or(ai).as_deref(), 80)
}

fn read_title(path: &str) -> Option<String> {
    if !path.to_ascii_lowercase().ends_with(".jsonl") {
        return None;
    }
    let mut f = File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let len = size.min(TITLE_TAIL_BYTES);
    f.seek(SeekFrom::Start(size - len)).ok()?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf).ok()?;
    title_from_transcript_tail(&String::from_utf8_lossy(&buf))
}

impl Shared {
    fn title_for(&self, ev: &Event, now: u64) -> Option<String> {
        let transcript = ev.transcript.as_deref()?;
        {
            let mut seen = self.titles.lock().unwrap();
            if seen.get(&ev.session_id).is_some_and(|t| now.saturating_sub(*t) < TITLE_INTERVAL_MS) {
                return None;
            }
            if seen.len() >= 500 {
                seen.clear();
            }
            seen.insert(ev.session_id.clone(), now);
        }
        read_title(transcript)
    }

    pub fn on_event(&self, mut ev: Event) -> Result<(), String> {
        let now = now_ms();
        if ev.title.is_none() {
            ev.title = self.title_for(&ev, now);
        }
        let t = self.store.lock().unwrap().apply(&ev, now)?;
        if let Some(t) = t {
            self.pending.lock().unwrap().push(t);
        }
        #[cfg(windows)]
        crate::window::wake(self.hwnd.load(Ordering::Relaxed));
        Ok(())
    }
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body_start: Vec<u8>,
}

fn read_head(s: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEADER {
            return None;
        }
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Some(Request { method, path, headers, body_start: buf[end + 4..].to_vec() })
}

fn reply(s: &mut TcpStream, code: u16, body: &str) {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let msg = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = s.write_all(msg.as_bytes());
}

fn reply_error(s: &mut TcpStream, code: u16, msg: &str) {
    reply(s, code, &serde_json::json!({ "error": msg }).to_string());
}

/// Browsers attach Origin to cross-site POSTs and rebound hostnames show up in Host: a web page must
/// not be able to drive or read a localhost service, so only local non-browser callers get through.
fn allowed_caller(r: &Request, port: u16) -> bool {
    if r.headers.contains_key("origin") {
        return false;
    }
    match r.headers.get("host") {
        Some(h) => *h == format!("127.0.0.1:{port}") || *h == format!("localhost:{port}"),
        None => false,
    }
}

fn read_body(s: &mut TcpStream, r: &Request, limit: usize) -> Result<Vec<u8>, u16> {
    let len: usize = r.headers.get("content-length").and_then(|v| v.parse().ok()).ok_or(400u16)?;
    if len > limit {
        return Err(413);
    }
    let mut body = r.body_start.clone();
    if body.len() < len {
        let remaining = (len - body.len()) as u64;
        s.take(remaining).read_to_end(&mut body).map_err(|_| 400u16)?;
    }
    if body.len() != len {
        return Err(400);
    }
    Ok(body)
}

fn usage_json(days: &usage::Days, today: u32, ready: bool) -> Value {
    let row = |c: &usage::Counts| serde_json::json!({ "input": c[0], "output": c[1], "cacheWrite": c[2], "cacheRead": c[3], "total": usage::total(c) });
    let [d, w, m] = usage::periods(days, today);
    serde_json::json!({ "ready": ready, "day": today, "today": row(&d), "week": row(&w), "month": row(&m) })
}

fn handle(mut s: TcpStream, shared: &Shared, port: u16) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
    let Some(r) = read_head(&mut s) else { return };
    if !allowed_caller(&r, port) {
        return reply_error(&mut s, 403, "forbidden");
    }
    match (r.method.as_str(), r.path.as_str()) {
        ("GET", "/health") => reply(&mut s, 200, r#"{"ok":true}"#),
        ("GET", "/sessions") => {
            let list = shared.store.lock().unwrap().list();
            reply(&mut s, 200, &sessions_json(&list).to_string());
        }
        ("GET", "/usage") => {
            let days = shared.usage.lock().unwrap().clone();
            reply(&mut s, 200, &usage_json(&days, usage::today(), shared.scanned.load(Ordering::Acquire)).to_string());
        }
        ("POST", "/hook") => {
            // Malformed input or an event Buddy does not track: ignore silently, still answer 200.
            if let Ok(body) = read_body(&mut s, &r, MAX_HOOK_BODY) {
                let text = String::from_utf8_lossy(&body);
                // Tolerate a BOM or stray bytes some shells put before the JSON.
                let json = text.find('{').map_or("", |i| &text[i..]);
                if let Some(ev) = serde_json::from_str(json).ok().and_then(|j| hook_to_event(&j, &r.headers)) {
                    let _ = shared.on_event(ev);
                }
            }
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
        ("POST", "/event") => match read_body(&mut s, &r, MAX_EVENT_BODY) {
            Err(code) => reply_error(&mut s, code, "bad body"),
            Ok(body) => {
                let result = serde_json::from_slice::<Value>(&body)
                    .map_err(|e| e.to_string())
                    .and_then(|j| event_from_json(&j))
                    .and_then(|ev| shared.on_event(ev));
                match result {
                    Ok(()) => reply(&mut s, 200, r#"{"ok":true}"#),
                    Err(e) => reply_error(&mut s, 400, &e),
                }
            }
        },
        _ => reply_error(&mut s, 404, "not found"),
    }
}

/// Binds now, so the caller can report a busy port, then serves on a background thread.
pub fn start(shared: Arc<Shared>, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    std::thread::Builder::new().stack_size(128 * 1024).name("server".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            if shared.connections.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                continue; // dropping the stream closes it
            }
            shared.connections.fetch_add(1, Ordering::Relaxed);
            let worker = shared.clone();
            let spawned = std::thread::Builder::new().stack_size(512 * 1024).spawn(move || {
                handle(conn, &worker, port);
                worker.connections.fetch_sub(1, Ordering::Relaxed);
            });
            if spawned.is_err() {
                shared.connections.fetch_sub(1, Ordering::Relaxed);
            }
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hook(j: Value) -> Event {
        hook_to_event(&j, &HashMap::new()).unwrap()
    }

    #[test]
    fn describes_tools() {
        assert_eq!(activity_for("Bash", &json!({"description":"Run tests","command":"x"})).unwrap(), "chạy Run tests");
        assert_eq!(activity_for("Bash", &json!({"command":"npm test\nmore"})).unwrap(), "chạy npm test");
        assert_eq!(activity_for("Edit", &json!({"file_path":"C:\\a\\main.rs"})).unwrap(), "sửa main.rs");
        assert_eq!(activity_for("Grep", &json!({"pattern":"foo"})).unwrap(), "tìm foo");
        assert_eq!(activity_for("WebSearch", &json!({})).unwrap(), "tra cứu web");
        assert_eq!(activity_for("mcp__ctx__query", &json!({})).unwrap(), "ctx: query");
        let todos = json!({"todos":[{"status":"completed","content":"a"},{"status":"in_progress","activeForm":"Làm b","content":"b"}]});
        assert_eq!(activity_for("TodoWrite", &todos).unwrap(), "Làm b (1/2)");
    }

    #[test]
    fn hook_input_needs_ids() {
        assert!(hook_to_event(&json!({"hook_event_name":"Stop"}), &HashMap::new()).is_none());
        assert!(hook_to_event(&json!({"session_id":"","hook_event_name":"Stop"}), &HashMap::new()).is_none());
        assert!(hook_to_event(&json!({"session_id":"a"}), &HashMap::new()).is_none());
    }

    #[test]
    fn injected_prompts_are_not_tasks() {
        let e = hook(json!({"session_id":"a","hook_event_name":"UserPromptSubmit","prompt":"<task-notification>x"}));
        assert_eq!(e.task, None);
        let e = hook(json!({"session_id":"a","hook_event_name":"UserPromptSubmit","prompt":"# Sửa lỗi\nchi tiết"}));
        assert_eq!(e.task.unwrap(), "Sửa lỗi");
        assert_eq!(e.activity.unwrap(), "đang suy nghĩ");
    }

    #[test]
    fn stop_failure_explains_itself() {
        let e = hook(json!({"session_id":"a","hook_event_name":"StopFailure","error":"rate_limit","error_details":"chờ 5 phút"}));
        assert_eq!(e.message.unwrap(), "Hết hạn mức sử dụng (rate limit) · chờ 5 phút");
    }

    #[test]
    fn unexpanded_env_headers_are_ignored() {
        let mut h = HashMap::new();
        h.insert("x-buddy-source".to_string(), "%CLAUDE_BUDDY_SOURCE%".to_string());
        h.insert("x-buddy-label".to_string(), "Backend".to_string());
        h.insert("x-buddy-pid".to_string(), "4242".to_string());
        let e = hook_to_event(&json!({"session_id":"a","hook_event_name":"Stop"}), &h).unwrap();
        assert_eq!(e.source, None);
        assert_eq!(e.label.unwrap(), "Backend");
        assert_eq!(e.pid, Some(4242));
    }

    #[test]
    fn custom_title_beats_generated_title() {
        let tail = "ut off\n{\"type\":\"ai-title\",\"aiTitle\":\"AI\"}\n{\"type\":\"custom-title\",\"customTitle\":\"Mine\"}\n{\"type\":\"ai-title\",\"aiTitle\":\"AI2\"}\n";
        assert_eq!(title_from_transcript_tail(tail).unwrap(), "Mine");
        assert_eq!(title_from_transcript_tail("{\"type\":\"ai-title\",\"aiTitle\":\"AI\"}").unwrap(), "AI");
        assert_eq!(title_from_transcript_tail("nothing"), None);
    }

    #[test]
    fn browsers_and_rebound_hosts_are_refused() {
        let req = |hs: &[(&str, &str)]| Request {
            method: "POST".into(),
            path: "/hook".into(),
            headers: hs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body_start: vec![],
        };
        assert!(allowed_caller(&req(&[("host", "127.0.0.1:47329")]), 47329));
        assert!(allowed_caller(&req(&[("host", "localhost:47329")]), 47329));
        assert!(!allowed_caller(&req(&[("host", "evil.example:47329")]), 47329));
        assert!(!allowed_caller(&req(&[("host", "127.0.0.1:47329"), ("origin", "https://evil.example")]), 47329));
        assert!(!allowed_caller(&req(&[]), 47329));
    }
}
