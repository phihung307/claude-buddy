//! Claude Buddy (Rust): a desktop companion showing what your Claude Code sessions are doing.
//!
//!   claude-buddy-rs                  run the app
//!   claude-buddy-rs hook <port>      forward one Claude Code hook event to the running app
//!   claude-buddy-rs --install-hooks  add this app's hooks to Claude Code's settings
//!   claude-buddy-rs --remove-hooks   remove this app's hooks from Claude Code's settings
//!   claude-buddy-rs --cut-sheets <folder> <output folder>   cut sprite sheets into animated PNGs

#![windows_subsystem = "windows"]
// Elsewhere only the part without a window exists so far, so much of what it serves goes unused.
#![cfg_attr(not(windows), allow(dead_code, unused_imports, unused_variables))]

mod anim;
mod codex;
mod config;
#[cfg(windows)]
mod focus;
mod hook;
mod hooks_config;
mod i18n;
mod library;
#[cfg(target_os = "macos")]
mod mac;
mod media;
#[cfg(windows)]
mod mf;
mod motion;
#[cfg(windows)]
mod picker;
mod render;
mod sheet;
mod server;
#[cfg(windows)]
mod sound;
mod store;
mod theme;
mod usage;
mod webm;
#[cfg(windows)]
mod window;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The two that run constantly in the background never show text.
    if !matches!(args.first().map(String::as_str), Some("hook" | "--scan-usage")) {
        i18n::apply(&config::Config::load().language);
    }
    match args.first().map(String::as_str) {
        Some("hook") => {
            if let Some(port) = args.get(1).and_then(|p| p.parse().ok()) {
                hook::run(port);
            }
        }
        Some("--scan-usage") => usage::scan_main(),
        Some("--prepare") => std::process::exit(anim::prepare_main(&args[1..])),
        Some("--cut-sheets") => std::process::exit(sheet::cut_main(&args[1..])),
        Some("--install-hooks") => std::process::exit(hooks_config::cli(true)),
        Some("--remove-hooks") => std::process::exit(hooks_config::cli(false)),
        #[cfg(windows)]
        _ => window::run(),
        #[cfg(target_os = "macos")]
        _ => mac::run(),
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
