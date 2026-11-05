//! Claude Buddy (Rust): a desktop companion showing what your Claude Code sessions are doing.
//!
//!   claude-buddy-rs                  run the app
//!   claude-buddy-rs hook <port>      forward one Claude Code hook event to the running app
//!   claude-buddy-rs --install-hooks  add this app's hooks to Claude Code's settings
//!   claude-buddy-rs --remove-hooks   remove this app's hooks from Claude Code's settings
//!   claude-buddy-rs --cut-sheets <folder> <output folder>   cut sprite sheets into animated PNGs

#![windows_subsystem = "windows"]

mod anim;
mod codex;
mod config;
mod focus;
mod fx;
mod hook;
mod hooks_config;
mod i18n;
mod library;
mod media;
mod mf;
mod motion;
mod picker;
mod render;
mod sheet;
mod server;
mod sound;
mod store;
mod theme;
mod usage;
mod webm;
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
        Some("--install-hooks") => std::process::exit(window::hooks_cli(true)),
        Some("--remove-hooks") => std::process::exit(window::hooks_cli(false)),
        _ => window::run(),
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
