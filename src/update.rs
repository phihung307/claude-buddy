//! Keeps an installed copy on the latest release. Only what GitHub calls the latest counts, so a
//! pre-release reaches nobody until it is promoted. The new copy is downloaded, checked and run once
//! beside the old one. Restarting costs Claude Code nothing, as its hooks just skip an app that is
//! away for a moment, so the swap only waits for the main window: Codex may be drawing behind it.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const REPO: &str = "taivt0109/claude-buddy";
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);
/// How long a new copy waits for the old one to let go of the port and the single-instance lock.
const OLD_COPY_WAIT: Duration = Duration::from_secs(10);
const DOWNLOAD_SECONDS: &str = "300";

#[cfg(windows)]
const ASSET: &str = "claude-buddy-rs.exe";
#[cfg(target_os = "macos")]
const ASSET: &str = "ClaudeBuddy-macos.zip";
#[cfg(not(any(windows, target_os = "macos")))]
const ASSET: &str = "";

static ENABLED: AtomicBool = AtomicBool::new(true);
static READY: AtomicBool = AtomicBool::new(false);

/// Where the installers put the app. A copy anywhere else, like a build from source, is left alone.
fn installed_exe() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Programs").join("ClaudeBuddy").join("claude-buddy-rs.exe")
    } else {
        installed_app().join("Contents").join("MacOS").join("claude-buddy-rs")
    }
}

fn installed_app() -> PathBuf {
    crate::config::home().join("Applications").join("Claude Buddy.app")
}

fn is_installed() -> bool {
    let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
    std::env::current_exe().is_ok_and(|e| same(&e, &installed_exe()))
}

/// Next to what it replaces, so the swap is a rename on the same disk.
fn work_dir() -> PathBuf {
    let parent = if cfg!(windows) { installed_exe() } else { installed_app() };
    parent.parent().unwrap_or(Path::new(".")).join(".claude-buddy-update")
}

/// The version in a release tag, when it is newer than `current`.
fn newer(tag: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<[u32; 3]> {
        let mut parts = v.trim_start_matches('v').split('.').map(|p| p.parse().ok());
        let found = [parts.next()??, parts.next()??, parts.next()??];
        parts.next().is_none().then_some(found)
    };
    matches!((parse(tag), parse(current)), (Some(t), Some(c)) if t > c)
}

fn curl() -> Command {
    let path = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_default()).join("System32").join("curl.exe")
    } else {
        PathBuf::from("/usr/bin/curl")
    };
    let mut c = quiet_command(&path);
    c.args(["-fsSL", "--max-time", DOWNLOAD_SECONDS]);
    c
}

fn quiet_command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn download(url: &str, to: &Path) -> Option<()> {
    let ok = curl().stdout(Stdio::null()).arg("-o").arg(to).arg(url).status().ok()?.success();
    ok.then_some(())
}

fn latest_tag(api: &str) -> Option<String> {
    let out = curl().arg(format!("{api}/releases/latest")).output().ok()?;
    let release: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    release.get("tag_name")?.as_str().map(str::to_string)
}

fn checksum_matches(file: &Path, listed: &str) -> bool {
    let Some(want) = listed.split_whitespace().next() else { return false };
    let Ok(bytes) = std::fs::read(file) else { return false };
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    got.eq_ignore_ascii_case(want)
}

/// The copy inside the work folder that a swap would put in place.
fn staged_exe(work: &Path) -> PathBuf {
    if cfg!(windows) {
        work.join(ASSET)
    } else {
        staged_app(work).join("Contents").join("MacOS").join("claude-buddy-rs")
    }
}

fn staged_app(work: &Path) -> PathBuf {
    work.join("unpacked").join("Claude Buddy.app")
}

/// Fetches a release and proves the new copy runs here, which also catches one an antivirus took away.
fn prepare(base: &str, tag: &str) -> Option<()> {
    let work = work_dir();
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).ok()?;
    let file = work.join(ASSET);
    let sums = work.join("sha256");
    download(&format!("{base}/releases/download/{tag}/{ASSET}"), &file)?;
    download(&format!("{base}/releases/download/{tag}/{ASSET}.sha256"), &sums)?;
    if !checksum_matches(&file, &std::fs::read_to_string(&sums).ok()?) {
        return None;
    }
    if cfg!(target_os = "macos") {
        let unpacked = quiet_command(Path::new("/usr/bin/ditto")).stdout(Stdio::null()).args(["-x", "-k"]).arg(&file).arg(work.join("unpacked")).status().ok()?;
        if !unpacked.success() {
            return None;
        }
    }
    let said = quiet_command(&staged_exe(&work)).arg("--version").output().ok()?;
    (String::from_utf8_lossy(&said.stdout).trim() == tag.trim_start_matches('v')).then_some(())
}

/// With a test server, a copy can pretend to be older than it is, so one build can update to itself.
fn current_version() -> String {
    std::env::var("CLAUDE_BUDDY_RS_UPDATE_URL").ok().and(std::env::var("CLAUDE_BUDDY_RS_UPDATE_FROM").ok()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn check(api: &str, base: &str) -> bool {
    let Some(tag) = latest_tag(api).filter(|t| newer(t, &current_version())) else { return false };
    if prepare(base, &tag).is_some() {
        return true;
    }
    let _ = std::fs::remove_dir_all(work_dir());
    false
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
}

pub fn start(on: bool) {
    set_enabled(on);
    if !is_installed() {
        return;
    }
    let local = std::env::var("CLAUDE_BUDDY_RS_UPDATE_URL").ok();
    let first = if local.is_some() { Duration::from_secs(2) } else { FIRST_CHECK };
    let (api, base) = match local {
        Some(url) => (url.clone(), url),
        None => (format!("https://api.github.com/repos/{REPO}"), format!("https://github.com/{REPO}")),
    };
    let _ = std::thread::Builder::new().stack_size(256 * 1024).spawn(move || {
        std::thread::sleep(first);
        loop {
            if ENABLED.load(Ordering::Acquire) && check(&api, &base) {
                READY.store(true, Ordering::Release);
                return;
            }
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

/// A checked copy is waiting to be put in place.
pub fn ready() -> bool {
    READY.load(Ordering::Acquire) && ENABLED.load(Ordering::Acquire)
}

/// Puts the new copy in place and starts it. True means the caller should quit now; on false the old
/// copy is back where it was.
pub fn apply() -> bool {
    READY.store(false, Ordering::Release);
    let work = work_dir();
    let (current, staged, old) = if cfg!(windows) {
        (installed_exe(), staged_exe(&work), work.join("old.exe"))
    } else {
        (installed_app(), staged_app(&work), work.join("old.app"))
    };
    if std::fs::rename(&current, &old).is_err() {
        return false;
    }
    let started = std::fs::rename(&staged, &current).is_ok()
        && quiet_command(&installed_exe()).stdout(Stdio::null()).arg("--after-update").arg(std::process::id().to_string()).spawn().is_ok();
    if !started {
        let _ = std::fs::rename(&current, &staged);
        let _ = std::fs::rename(&old, &current);
    }
    started
}

#[cfg(windows)]
fn wait_for_exit(pid: u32, wait: Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, wait.as_millis() as u32);
            CloseHandle(h);
        }
    }
}

#[cfg(unix)]
fn wait_for_exit(pid: u32, wait: Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < wait && unsafe { libc::kill(pid as i32, 0) } == 0 {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The first thing a freshly updated copy does: the old one has to be gone before this one starts,
/// and with it the file that could not be deleted while it ran.
pub fn finish(old_pid: Option<u32>) {
    if let Some(pid) = old_pid {
        wait_for_exit(pid, OLD_COPY_WAIT);
    }
    let _ = std::fs::remove_dir_all(work_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_higher_version_counts() {
        assert!(newer("v0.3.5", "0.3.4"));
        assert!(newer("v0.10.0", "0.9.9"));
        assert!(newer("v1.0.0", "0.99.99"));
        assert!(!newer("v0.3.4", "0.3.4"));
        assert!(!newer("v0.3.3", "0.3.4"));
    }

    #[test]
    fn an_odd_tag_is_not_an_update() {
        assert!(!newer("v0.4.0-beta", "0.3.4"));
        assert!(!newer("latest", "0.3.4"));
        assert!(!newer("v1.2", "0.3.4"));
        assert!(!newer("v1.2.3.4", "0.3.4"));
    }

    #[test]
    fn the_checksum_must_match_the_file() {
        let dir = std::env::temp_dir().join(format!("buddy-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f");
        std::fs::write(&file, b"abc").unwrap();
        let abc = "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
        assert!(checksum_matches(&file, &format!("{abc}  f\n")));
        assert!(!checksum_matches(&file, "00  f\n"));
        assert!(!checksum_matches(&file, ""));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
