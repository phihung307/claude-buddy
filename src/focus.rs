//! Brings a session's terminal/IDE window to the front: first the windows of the Claude process's
//! ancestors, then any terminal or editor whose title names the session's folder.

use crate::store::Session;
use std::collections::HashMap;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_MENU};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow,
    ShowWindow, GW_OWNER, SW_RESTORE,
};

/// Folder names too generic to identify a window by title.
const GENERIC: [&str; 11] = ["src", "source", "repos", "code", "projects", "users", "home", "documents", "desktop", "dev", "work"];
const SYSTEM_ROOTS: [&str; 5] = ["explorer.exe", "svchost.exe", "services.exe", "wininit.exe", "winlogon.exe"];
const CONSOLE_HOSTS: [&str; 2] = ["conhost.exe", "openconsole.exe"];
const TITLE_APPS: [&str; 16] = [
    "windowsterminal.exe", "code.exe", "code - insiders.exe", "cursor.exe", "windsurf.exe", "idea64.exe", "pycharm64.exe",
    "webstorm64.exe", "rider64.exe", "goland64.exe", "devenv.exe", "conhost.exe", "openconsole.exe", "wezterm-gui.exe",
    "alacritty.exe", "mintty.exe",
];

#[derive(Clone, Debug)]
pub struct Proc {
    pub parent: u32,
    /// Lowercase executable name.
    pub name: String,
}

pub fn is_claude_process(name: &str) -> bool {
    matches!(name, "claude.exe" | "node.exe" | "bun.exe" | "claude" | "node" | "bun")
}

pub fn processes() -> HashMap<u32, Proc> {
    let mut out = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
            out.insert(e.th32ProcessID, Proc { parent: e.th32ParentProcessID, name });
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    out
}

/// Words to look for in window titles, best first: an explicit label for non-CLI sources, then the
/// last folders of the working directory (editors title their windows with the workspace folder).
pub fn title_hints(s: &Session) -> Vec<String> {
    let mut hints = Vec::new();
    if s.source != "cli" {
        hints.push(s.label.clone());
    }
    let cwd = s.cwd.as_deref().unwrap_or("");
    let is_drive = |p: &str| p.len() == 2 && p.ends_with(':');
    let parts: Vec<&str> = cwd.split(['\\', '/']).filter(|p| !p.is_empty() && !is_drive(p)).collect();
    for p in parts.iter().rev().take(3) {
        if p.chars().count() > 2 && !GENERIC.contains(&p.to_lowercase().as_str()) && !hints.iter().any(|h| h == p) {
            hints.push(p.to_string());
        }
    }
    hints
}

/// Earlier hints score higher; 0 = no hint in the title.
pub fn score(title: &str, hints: &[String]) -> i32 {
    let t = title.to_lowercase();
    hints.iter().position(|h| t.contains(&h.to_lowercase())).map_or(0, |i| 100 - i as i32)
}

#[derive(Clone, Debug)]
pub struct Win {
    pub handle: isize,
    pub pid: u32,
    pub title: String,
}

unsafe extern "system" fn collect(h: HWND, l: LPARAM) -> i32 {
    let out = &mut *(l as *mut Vec<Win>);
    // Visible, titled top-level windows only (owned popups are skipped).
    if IsWindowVisible(h) != 0 && GetWindow(h, GW_OWNER).is_null() {
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        if n > 0 {
            let mut pid = 0;
            GetWindowThreadProcessId(h, &mut pid);
            out.push(Win { handle: h as isize, pid, title: String::from_utf16_lossy(&buf[..n as usize]) });
        }
    }
    1
}

fn windows() -> Vec<Win> {
    let mut out: Vec<Win> = Vec::new();
    unsafe { EnumWindows(Some(collect), &mut out as *mut _ as LPARAM) };
    out
}

/// First-listed (topmost) window wins a tie.
fn best<'a>(list: impl Iterator<Item = &'a Win>, hints: &[String]) -> Option<&'a Win> {
    list.fold(None, |acc: Option<&Win>, w| match acc {
        Some(a) if score(&a.title, hints) >= score(&w.title, hints) => Some(a),
        _ => Some(w),
    })
}

pub fn pick_window<'a>(wins: &'a [Win], procs: &HashMap<u32, Proc>, pid: Option<u32>, hints: &[String]) -> Option<&'a Win> {
    // Only trust a pid that still is a Claude process: ids are reused once it exits.
    if let Some(start) = pid.filter(|p| procs.get(p).is_some_and(|i| is_claude_process(&i.name))) {
        let mut p = start;
        for _ in 0..15 {
            let Some(info) = procs.get(&p) else { break };
            if SYSTEM_ROOTS.contains(&info.name.as_str()) {
                break;
            }
            // Windows of this ancestor, or of a console host it started (classic consoles belong to conhost).
            let hosts: Vec<u32> = procs
                .iter()
                .filter(|(_, c)| c.parent == p && CONSOLE_HOSTS.contains(&c.name.as_str()))
                .map(|(id, _)| *id)
                .collect();
            let own = wins.iter().filter(|w| w.pid == p || hosts.contains(&w.pid));
            if let Some(w) = best(own, hints) {
                return Some(w);
            }
            p = info.parent;
        }
    }
    // No live pid: a terminal or editor window whose title names the session's folder.
    let candidates = wins.iter().filter(|w| {
        procs.get(&w.pid).is_some_and(|i| TITLE_APPS.contains(&i.name.as_str())) && score(&w.title, hints) > 0
    });
    best(candidates, hints)
}

/// Whether the window in front is this session's own terminal or editor: then you already see what it
/// is saying, and a popup would only get in the way.
pub fn looking_at(s: &Session) -> bool {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return false;
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        let mut pid = 0;
        GetWindowThreadProcessId(h, &mut pid);
        let front = [Win { handle: h as isize, pid, title: String::from_utf16_lossy(&buf[..n]) }];
        pick_window(&front, &processes(), s.pid, &title_hints(s)).is_some()
    }
}

/// Returns false when no window could be identified.
pub fn focus_session(s: &Session) -> bool {
    let wins = windows();
    let Some(w) = pick_window(&wins, &processes(), s.pid, &title_hints(s)) else { return false };
    unsafe {
        let h = w.handle as HWND;
        if IsIconic(h) != 0 {
            ShowWindow(h, SW_RESTORE);
        }
        // A synthetic Alt tap lets a background process move another window to the foreground.
        keybd_event(VK_MENU as u8, 0, 0, 0);
        keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
        SetForegroundWindow(h);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Event, Store};

    fn session(cwd: &str, source: Option<&str>, label: Option<&str>) -> Session {
        let mut st = Store::default();
        let ev = Event {
            session_id: "s".into(),
            event: "SessionStart".into(),
            cwd: Some(cwd.into()),
            source: source.map(Into::into),
            label: label.map(Into::into),
            ..Default::default()
        };
        st.apply(&ev, 0).unwrap();
        st.get("s").unwrap().clone()
    }

    fn proc(parent: u32, name: &str) -> Proc {
        Proc { parent, name: name.into() }
    }

    fn win(handle: isize, pid: u32, title: &str) -> Win {
        Win { handle, pid, title: title.into() }
    }

    #[test]
    fn hints_skip_generic_folders_and_drive() {
        let h = title_hints(&session("C:\\Users\\me\\source\\repos\\shop-api", None, None));
        assert_eq!(h, ["shop-api"]);
        let h = title_hints(&session("D:\\work\\acme\\billing\\api", None, None));
        assert_eq!(h, ["api", "billing", "acme"]);
    }

    #[test]
    fn non_cli_sources_prefer_their_label() {
        let h = title_hints(&session("D:\\acme", Some("portal"), Some("Backend")));
        assert_eq!(h, ["Backend", "acme"]);
    }

    #[test]
    fn earlier_hints_score_higher_ignoring_case() {
        let hints = vec!["api".to_string(), "billing".to_string()];
        assert_eq!(score("Billing - VS Code", &hints), 99);
        assert_eq!(score("my API", &hints), 100);
        assert_eq!(score("Notepad", &hints), 0);
    }

    #[test]
    fn walks_up_from_claude_to_the_terminal_window() {
        let procs: HashMap<u32, Proc> =
            [(10, proc(9, "claude.exe")), (9, proc(8, "pwsh.exe")), (8, proc(1, "windowsterminal.exe")), (1, proc(0, "explorer.exe"))].into();
        let wins = [win(1, 77, "Other"), win(2, 8, "Terminal")];
        assert_eq!(pick_window(&wins, &procs, Some(10), &[]).unwrap().handle, 2);
    }

    #[test]
    fn classic_console_windows_belong_to_conhost() {
        let procs: HashMap<u32, Proc> = [(10, proc(9, "claude.exe")), (9, proc(1, "cmd.exe")), (50, proc(9, "conhost.exe"))].into();
        let wins = [win(5, 50, "C:\\WINDOWS\\system32\\cmd.exe")];
        assert_eq!(pick_window(&wins, &procs, Some(10), &[]).unwrap().handle, 5);
    }

    #[test]
    fn reused_pid_falls_back_to_titles() {
        let procs: HashMap<u32, Proc> = [(10, proc(1, "notepad.exe")), (20, proc(1, "code.exe"))].into();
        let wins = [win(1, 10, "shop-api notes"), win(2, 20, "main.rs - shop-api - VS Code")];
        assert_eq!(pick_window(&wins, &procs, Some(10), &["shop-api".into()]).unwrap().handle, 2);
    }

    #[test]
    fn an_unrelated_front_window_is_not_the_session() {
        let procs: HashMap<u32, Proc> = [(10, proc(9, "claude.exe")), (9, proc(1, "windowsterminal.exe")), (30, proc(1, "chrome.exe"))].into();
        let hints = vec!["shop-api".to_string()];
        // A browser tab that merely mentions the project is not where the session is.
        assert!(pick_window(&[win(1, 30, "shop-api docs - Chrome")], &procs, Some(10), &hints).is_none());
        assert!(pick_window(&[win(2, 9, "pwsh")], &procs, Some(10), &hints).is_some());
    }

    #[test]
    fn nothing_matching_means_no_window() {
        let procs: HashMap<u32, Proc> = [(20, proc(1, "code.exe"))].into();
        let wins = [win(2, 20, "other project")];
        assert!(pick_window(&wins, &procs, None, &["shop-api".into()]).is_none());
    }

    #[test]
    fn walk_stops_at_system_processes() {
        let procs: HashMap<u32, Proc> = [(10, proc(1, "claude.exe")), (1, proc(0, "explorer.exe"))].into();
        let wins = [win(9, 1, "Desktop")];
        assert!(pick_window(&wins, &procs, Some(10), &[]).is_none());
    }
}
