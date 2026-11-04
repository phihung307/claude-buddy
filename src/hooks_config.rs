//! Installs or removes this app's hooks in Claude Code's settings.json.
//! Only entries pointing at this program are touched; everything else stays as the user wrote it.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const EVENTS: [(&str, Option<&str>); 8] = [
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some("*")),
    ("PostToolUse", Some("*")),
    ("Notification", Some("permission_prompt|elicitation_dialog")),
    ("Stop", None),
    ("StopFailure", None),
    ("SessionEnd", None),
];

const MARKER: &str = "claude-buddy-rs";

pub fn settings_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir).join("settings.json");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join(".claude").join("settings.json")
}

fn is_ours(h: &Value) -> bool {
    h.get("command").and_then(Value::as_str).is_some_and(|c| c.to_ascii_lowercase().contains(MARKER))
}

pub fn remove_hooks(settings: &Value) -> Value {
    let mut out = settings.clone();
    let Some(hooks) = out.get_mut("hooks").and_then(Value::as_object_mut) else { return out };
    let mut next = Map::new();
    for (event, groups) in hooks.iter() {
        let Some(groups) = groups.as_array() else {
            next.insert(event.clone(), groups.clone());
            continue;
        };
        let kept: Vec<Value> = groups
            .iter()
            .filter_map(|g| {
                let Some(list) = g.get("hooks").and_then(Value::as_array) else { return Some(g.clone()) };
                let others: Vec<Value> = list.iter().filter(|h| !is_ours(h)).cloned().collect();
                if others.is_empty() {
                    return None;
                }
                let mut g = g.clone();
                g["hooks"] = Value::Array(others);
                Some(g)
            })
            .collect();
        if !kept.is_empty() {
            next.insert(event.clone(), Value::Array(kept));
        }
    }
    if next.is_empty() {
        out.as_object_mut().map(|o| o.shift_remove("hooks"));
    } else {
        out["hooks"] = Value::Object(next);
    }
    out
}

fn group_for(matcher: Option<&str>, exe: &str, port: u16) -> Value {
    // Exec form: spawned directly, without a shell that would need quoting or add startup time.
    let hook = json!({ "type": "command", "command": exe, "args": ["hook", port.to_string()], "timeout": 5 });
    match matcher {
        Some(m) => json!({ "matcher": m, "hooks": [hook] }),
        None => json!({ "hooks": [hook] }),
    }
}

pub fn add_hooks(settings: &Value, exe: &str, port: u16) -> Value {
    let mut out = remove_hooks(settings);
    if !out.is_object() {
        out = json!({});
    }
    if !out.get("hooks").is_some_and(Value::is_object) {
        out["hooks"] = json!({});
    }
    for (event, matcher) in EVENTS {
        let group = group_for(matcher, exe, port);
        match out["hooks"].get_mut(event).and_then(Value::as_array_mut) {
            Some(list) => list.push(group),
            None => out["hooks"][event] = json!([group]),
        }
    }
    out
}

/// Whether settings hold exactly the hooks this install would write (same program, port, events).
pub fn is_installed(settings: &Value, exe: &str, port: u16) -> bool {
    EVENTS.iter().all(|(event, matcher)| {
        let want = group_for(*matcher, exe, port);
        settings
            .get("hooks")
            .and_then(|h| h.get(*event))
            .and_then(Value::as_array)
            .is_some_and(|groups| groups.iter().any(|g| *g == want))
    })
}

fn read_settings(file: &Path) -> Result<Value, String> {
    let Ok(text) = fs::read_to_string(file) else { return Ok(json!({})) };
    let text = text.trim().trim_start_matches('\u{feff}');
    if text.is_empty() {
        return Ok(json!({}));
    }
    // Broken JSON: refuse, never overwrite a file we cannot read.
    serde_json::from_str(text).map_err(|e| crate::tf!("{} không phải JSON hợp lệ: {e}", "{} is not valid JSON: {e}", file.display()))
}

pub fn status(exe: &str, port: u16) -> bool {
    read_settings(&settings_path()).is_ok_and(|s| is_installed(&s, exe, port))
}

/// Backs the file up first and writes atomically. Returns the backup path, if one was made.
pub fn apply(install: bool, exe: &str, port: u16) -> Result<Option<PathBuf>, String> {
    let file = settings_path();
    let settings = read_settings(&file)?;
    let next = if install { add_hooks(&settings, exe, port) } else { remove_hooks(&settings) };
    if next == settings {
        return Ok(None);
    }
    let serialized = serde_json::to_string_pretty(&next).map_err(|e| e.to_string())? + "\n";
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut backup = None;
    if file.exists() {
        let stamp = crate::server::now_ms();
        let b = file.with_file_name(format!("settings.json.buddy-rs-backup-{stamp}"));
        fs::copy(&file, &b).map_err(|e| e.to_string())?;
        backup = Some(b);
    }
    let tmp = file.with_file_name("settings.json.buddy-rs-tmp");
    fs::write(&tmp, serialized).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "C:\\Tools\\claude-buddy-rs.exe";

    fn users_settings() -> Value {
        json!({
            "theme": "dark",
            "hooks": {
                "Notification": [
                    { "hooks": [{ "type": "command", "command": "powershell -File C:/Users/me/.claude/notify.ps1" }] },
                    { "matcher": "permission_prompt", "hooks": [{ "type": "command", "command": "cmd.exe", "args": ["/c", "C:\\Programs\\Claude Buddy\\buddy-hook.cmd", "47321"] }] }
                ],
                "CustomEvent": "left alone"
            },
            "zeta": 1
        })
    }

    #[test]
    fn install_adds_every_event_and_keeps_others() {
        let s = add_hooks(&users_settings(), EXE, 47329);
        assert!(is_installed(&s, EXE, 47329));
        let notif = s["hooks"]["Notification"].as_array().unwrap();
        assert_eq!(notif.len(), 3);
        assert!(notif[0]["hooks"][0]["command"].as_str().unwrap().contains("notify.ps1"));
        assert!(notif[1]["hooks"][0]["args"][1].as_str().unwrap().contains("buddy-hook.cmd"));
        assert_eq!(s["theme"], "dark");
        assert_eq!(s["hooks"]["CustomEvent"], "left alone");
    }

    #[test]
    fn install_then_remove_restores_the_original_with_key_order() {
        let original = users_settings();
        let back = remove_hooks(&add_hooks(&original, EXE, 47329));
        assert_eq!(serde_json::to_string(&back).unwrap(), serde_json::to_string(&original).unwrap());
    }

    #[test]
    fn install_is_idempotent() {
        let once = add_hooks(&users_settings(), EXE, 47329);
        assert_eq!(add_hooks(&once, EXE, 47329), once);
    }

    #[test]
    fn a_different_port_or_path_is_not_installed() {
        let s = add_hooks(&json!({}), EXE, 47329);
        assert!(!is_installed(&s, EXE, 1234));
        assert!(!is_installed(&s, "D:\\claude-buddy-rs.exe", 47329));
        assert!(!is_installed(&json!({}), EXE, 47329));
    }

    #[test]
    fn removing_from_clean_settings_changes_nothing_and_drops_empty_hooks() {
        let s = json!({ "a": 1 });
        assert_eq!(remove_hooks(&s), s);
        assert_eq!(remove_hooks(&add_hooks(&s, EXE, 1)), s);
    }
}
