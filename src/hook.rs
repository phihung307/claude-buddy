//! `claude-buddy-rs hook <port>`: the command Claude Code runs for each hook event.
//! Forwards stdin to the running app, prints nothing and always exits 0, so Claude Code never shows
//! a hook error and nothing reaches its context, even when the app isn't running.

#[cfg(windows)]
use crate::focus;
use crate::server::MAX_HOOK_BODY;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// The Claude process that started this hook. Shell wrappers may sit in between, so ancestors are walked.
#[cfg(windows)]
fn claude_pid() -> Option<u32> {
    let procs = focus::processes();
    let mut p = procs.get(&std::process::id())?.parent;
    let direct_parent = p;
    for _ in 0..8 {
        let info = procs.get(&p)?;
        if focus::is_claude_process(&info.name) {
            return Some(p);
        }
        p = info.parent;
    }
    Some(direct_parent)
}

/// `ps` lines of pid, parent and command. The command may hold spaces, and macOS gives its whole path.
#[cfg(not(windows))]
fn parse_ps(text: &str) -> std::collections::HashMap<u32, (u32, &str)> {
    text.lines()
        .filter_map(|l| {
            let (pid, rest) = l.trim_start().split_once(char::is_whitespace)?;
            let (parent, command) = rest.trim_start().split_once(char::is_whitespace)?;
            Some((pid.parse().ok()?, (parent.parse().ok()?, command.trim().rsplit('/').next()?)))
        })
        .collect()
}

#[cfg(not(windows))]
fn claude_pid() -> Option<u32> {
    let out = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,comm="]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let procs = parse_ps(&text);
    let direct_parent = std::os::unix::process::parent_id();
    let mut p = direct_parent;
    for _ in 0..8 {
        let (parent, name) = procs.get(&p)?;
        if matches!(*name, "claude" | "node" | "bun") {
            return Some(p);
        }
        p = *parent;
    }
    Some(direct_parent)
}

fn env_header(name: &str) -> String {
    std::env::var(name).unwrap_or_default().chars().filter(|c| !c.is_control()).collect()
}

pub fn run(port: u16) {
    let mut body = Vec::new();
    let read = std::io::stdin().lock().take(MAX_HOOK_BODY as u64 + 1).read_to_end(&mut body);
    if read.is_err() || body.is_empty() || body.len() > MAX_HOOK_BODY {
        return;
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));

    // Only SessionStart needs it, and a process snapshot is the slowest thing this command could do.
    let is_start = body.windows(12).any(|w| w == b"SessionStart");
    let pid = if is_start { claude_pid() } else { None };
    let mut head = format!(
        "POST /hook HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Buddy-Source: {}\r\nX-Buddy-Label: {}\r\nX-Buddy-Url: {}\r\n",
        body.len(),
        env_header("CLAUDE_BUDDY_SOURCE"),
        env_header("CLAUDE_BUDDY_LABEL"),
        env_header("CLAUDE_BUDDY_URL"),
    );
    if let Some(pid) = pid {
        head.push_str(&format!("X-Buddy-Pid: {pid}\r\n"));
    }
    head.push_str("\r\n");
    if s.write_all(head.as_bytes()).is_err() || s.write_all(&body).is_err() {
        return;
    }
    let mut sink = [0u8; 256];
    while matches!(s.read(&mut sink), Ok(n) if n > 0) {}
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::parse_ps;

    #[test]
    fn reads_ps_lines_with_paths_and_spaces() {
        let procs = parse_ps("    1     0 launchd
  512     1 /Applications/My Tools/claude
  900   512 /usr/local/bin/node
not a process
");
        assert_eq!(procs.get(&1), Some(&(0, "launchd")));
        assert_eq!(procs.get(&512), Some(&(1, "claude")));
        assert_eq!(procs.get(&900), Some(&(512, "node")));
        assert_eq!(procs.len(), 3);
    }
}
