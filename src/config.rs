//! Settings kept between runs, in %APPDATA%\claude-buddy-rs\config.json.

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

/// Not the original Claude Buddy's 47321, so both can run side by side.
pub const DEFAULT_PORT: u16 = 47329;
const OLD_CHARACTERS: [&str; 3] = ["mochi", "meo-muop", "gau-bo"];

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub port: u16,
    pub position: Option<(i32, i32)>,
    pub muted: bool,
    pub toasts: bool,
    pub character: String,
    /// How far the character sits from its usual spot inside the window, in logical pixels: past a screen
    /// edge the window stops and the character keeps going, so it can reach every edge.
    pub shift: f32,
    pub lift: f32,
    /// Character size: 0 small, 1 normal, 2 large.
    pub size: u8,
    /// Get out of the way of full-screen apps and presentations.
    pub auto_hide: bool,
    /// See-through window backgrounds, where Windows offers them.
    pub translucent: bool,
    /// The first-run "connect to Claude Code?" question has been asked.
    pub asked_hooks: bool,
    /// "vi", "en", or "auto" to follow Windows.
    pub language: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { port: DEFAULT_PORT, position: None, muted: false, toasts: true, character: "mochi".into(), shift: 0.0, lift: 0.0, size: 1, auto_hide: true, translucent: true, asked_hooks: false, language: "auto".into() }
    }
}

pub fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAUDE_BUDDY_RS_HOME") {
        return PathBuf::from(d);
    }
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("claude-buddy-rs")
}

fn parse(text: &str) -> Config {
    let d = Config::default();
    let Ok(j) = serde_json::from_str::<Value>(text) else { return d };
    let flag = |k: &str, def| j.get(k).and_then(Value::as_bool).unwrap_or(def);
    let int = |k: &str| j.get(k).and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok());
    Config {
        port: j.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0).unwrap_or(d.port),
        position: int("x").zip(int("y")),
        muted: flag("muted", d.muted),
        toasts: flag("toasts", d.toasts),
        character: match j.get("character") {
            Some(Value::String(id)) if !id.is_empty() => id.clone(),
            // Before custom characters this was an index into the built-in list.
            Some(Value::Number(n)) => OLD_CHARACTERS.get(n.as_u64().unwrap_or(0) as usize).unwrap_or(&OLD_CHARACTERS[0]).to_string(),
            _ => d.character,
        },
        shift: j.get("shift").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        lift: j.get("lift").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        size: j.get("size").and_then(Value::as_u64).map_or(d.size, |v| v.min(2) as u8),
        auto_hide: flag("autoHide", d.auto_hide),
        translucent: flag("translucent", d.translucent),
        asked_hooks: flag("askedHooks", d.asked_hooks),
        language: j.get("language").and_then(Value::as_str).unwrap_or(&d.language).to_string(),
    }
}

fn serialize(c: &Config) -> String {
    let mut j = json!({
        "port": c.port,
        "muted": c.muted,
        "toasts": c.toasts,
        "character": c.character,
        "shift": c.shift,
        "lift": c.lift,
        "size": c.size,
        "autoHide": c.auto_hide,
        "translucent": c.translucent,
        "askedHooks": c.asked_hooks,
        "language": c.language,
    });
    if let Some((x, y)) = c.position {
        j["x"] = json!(x);
        j["y"] = json!(y);
    }
    serde_json::to_string_pretty(&j).unwrap_or_default()
}

impl Config {
    pub fn load() -> Config {
        fs::read_to_string(dir().join("config.json")).map_or_else(|_| Config::default(), |t| parse(&t))
    }

    /// Losing a setting is not worth interrupting the user for.
    pub fn save(&self) {
        let d = dir();
        let _ = fs::create_dir_all(&d);
        let tmp = d.join("config.json.tmp");
        if fs::write(&tmp, serialize(self)).is_ok() {
            let _ = fs::rename(&tmp, d.join("config.json"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let c = Config { port: 5000, position: Some((-20, 300)), muted: true, toasts: false, character: "bear".into(), shift: -12.0, lift: 40.0, size: 2, auto_hide: false, translucent: false, asked_hooks: true, language: "en".into() };
        assert_eq!(parse(&serialize(&c)), c);
    }

    #[test]
    fn broken_or_partial_files_fall_back_to_defaults() {
        assert_eq!(parse("not json"), Config::default());
        let c = parse(r#"{"muted":true,"port":0}"#);
        assert!(c.muted);
        assert_eq!(c.port, DEFAULT_PORT);
    }

    #[test]
    fn old_numeric_characters_keep_working() {
        assert_eq!(parse(r#"{"character":2}"#).character, "gau-bo");
        assert_eq!(parse(r#"{"character":99}"#).character, "mochi");
    }
}
