//! Claude Buddy (Rust): a desktop companion showing what your Claude Code sessions are doing.
//!
//!   claude-buddy-rs                  run the app
//!   claude-buddy-rs hook <port>      forward one Claude Code hook event to the running app
//!   claude-buddy-rs --install-hooks  add this app's hooks to Claude Code's settings
//!   claude-buddy-rs --remove-hooks   remove this app's hooks from Claude Code's settings
//!   claude-buddy-rs --version        print the version
//!   claude-buddy-rs --cut-sheets <folder> <output folder>   cut sprite sheets into animated PNGs

#![windows_subsystem = "windows"]
// Linux only has the part without a window so far, so much of what it serves goes unused.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code, unused_imports, unused_variables))]

mod anim;
mod codex;
mod config;
#[cfg(windows)]
mod focus;
#[cfg(target_os = "macos")]
#[path = "focus_mac.rs"]
mod focus;
#[cfg(windows)]
mod fx;
#[cfg(windows)]
#[path = "host_win.rs"]
mod host;
#[cfg(target_os = "macos")]
#[path = "host_mac.rs"]
mod host;
mod hook;
mod hooks_config;
mod i18n;
mod library;
mod media;
#[cfg(windows)]
mod mf;
mod motion;
#[cfg(target_os = "macos")]
mod mp4_mac;
#[cfg(any(windows, target_os = "macos"))]
mod picker;
mod render;
mod sheet;
mod server;
#[cfg(any(windows, target_os = "macos"))]
mod sound;
mod store;
#[cfg(target_os = "macos")]
mod text_mac;
mod theme;
#[cfg(any(windows, target_os = "macos"))]
mod update;
mod usage;
mod webm;
#[cfg(windows)]
mod window;
#[cfg(target_os = "macos")]
#[path = "window_mac.rs"]
mod window;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The two that run constantly in the background never show text.
    if !matches!(args.first().map(String::as_str), Some("hook" | "--scan-usage" | "--version")) {
        i18n::apply(&config::Config::load().language);
    }
    match args.first().map(String::as_str) {
        Some("hook") => {
            if let Some(port) = args.get(1).and_then(|p| p.parse().ok()) {
                hook::run(port);
            }
        }
        Some("--scan-usage") => usage::scan_main(),
        Some("--version") => println!("{}", env!("CARGO_PKG_VERSION")),
        #[cfg(any(windows, target_os = "macos"))]
        Some("--after-update") => {
            update::finish(args.get(1).and_then(|p| p.parse().ok()));
            window::run();
        }
        Some("--prepare") => std::process::exit(anim::prepare_main(&args[1..])),
        Some("--cut-sheets") => std::process::exit(sheet::cut_main(&args[1..])),
        Some("--install-hooks") => std::process::exit(hooks_config::cli(true)),
        Some("--remove-hooks") => std::process::exit(hooks_config::cli(false)),
        #[cfg(any(windows, target_os = "macos"))]
        _ => window::run(),
        #[cfg(not(any(windows, target_os = "macos")))]
        _ => {
            eprintln!("Claude Buddy has no window on this system yet. Hooks, usage and character tools work from the command line.");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    // The details Explorer shows come from app.rc, which Cargo can't fill in, so a version bump has to touch both.
    #[test]
    fn file_details_match_the_package_version() {
        let rc = include_str!("../app.rc");
        let version = env!("CARGO_PKG_VERSION");
        let numbers = format!("{},0", version.replace('.', ","));
        assert!(rc.contains(&format!("\"FileVersion\", \"{version}\"")));
        assert!(rc.contains(&format!("\"ProductVersion\", \"{version}\"")));
        assert!(rc.contains(&format!("FILEVERSION {numbers}")));
        assert!(rc.contains(&format!("PRODUCTVERSION {numbers}")));
    }
}
