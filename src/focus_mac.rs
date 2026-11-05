//! Finding the app a session runs in, by walking up from its Claude process to the first program with
//! windows. macOS only lets this bring the app forward, not pick one of its windows, without asking
//! the user for Accessibility access.

use crate::hook::process_table;
use crate::store::Session;
use objc2_app_kit::{NSApplicationActivationOptions, NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};

const MAX_ANCESTORS: usize = 16;

fn ancestors(pid: u32) -> Vec<u32> {
    let table = process_table();
    let mut chain = vec![pid];
    let mut p = pid;
    while chain.len() < MAX_ANCESTORS {
        let Some(info) = table.get(&p) else { break };
        if info.0 <= 1 {
            break;
        }
        p = info.0;
        chain.push(p);
    }
    chain
}

/// Whether the app this session runs in is the one in front.
pub fn looking_at(s: &Session) -> bool {
    let Some(pid) = s.pid else { return false };
    let Some(front) = NSWorkspace::sharedWorkspace().frontmostApplication() else { return false };
    ancestors(pid).contains(&(front.processIdentifier() as u32))
}

/// Returns false when no app could be identified.
pub fn focus_session(s: &Session) -> bool {
    let Some(pid) = s.pid else { return false };
    for p in ancestors(pid) {
        let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(p as i32) else { continue };
        if app.activationPolicy() == NSApplicationActivationPolicy::Regular {
            return app.activateWithOptions(NSApplicationActivationOptions::empty());
        }
    }
    false
}
