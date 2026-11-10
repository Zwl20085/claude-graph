//! A Claude Code start-up dialog (workspace trust, a new MCP server) that
//! holds an Inbox session before its message is typed. The session itself
//! waits for the owner without a timeout and never types into the dialog
//! ([`crate::terminal::deliver::gate`], `terminal/startup.rs`); this module
//! only lets the watcher see it, so the tip can show ❓ meanwhile.
//!
//! The text that goes into the tip is fixed per dialog kind and never taken
//! from the screen: a dialog can name things from project files (an MCP
//! server's name), and tip text must stay the runner's own words.

use std::time::Duration;

use crate::terminal::api::Terminals;
use crate::terminal::deliver::DIALOG_SETTLE;
use crate::terminal::screen::Screen;

/// What the tip says for Claude Code's workspace-trust dialog.
pub const TRUST_TEXT: &str =
    "Claude Code start-up: do you trust the files in this folder? (workspace trust)";
/// … for its "New MCP server found in this project" dialog.
pub const MCP_TEXT: &str = "Claude Code start-up: new MCP server found in this project";
/// … for any other dialog on screen before the message was typed.
pub const OTHER_TEXT: &str = "Claude Code start-up dialog";

/// Screen text (normalized) that names the trust dialog.
const TRUST_MARKERS: [&str; 4] = [
    "accessing workspace",
    "quick safety check",
    "trust this folder",
    "do you trust",
];
const MCP_MARKER: &str = "new mcp server";

fn last_of(text: &str, needles: &[&str]) -> Option<usize> {
    needles.iter().filter_map(|n| text.rfind(n)).max()
}

/// What a session's screen shows before its message was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupDialog {
    /// No dialog waits.
    Closed,
    /// A dialog is open but output is still arriving (being drawn, or
    /// redrawn by a key): neither a new dialog nor an answered one yet.
    Drawing,
    /// A dialog waits, fully drawn; the tip's fixed text for it.
    Open(&'static str),
}

pub use StartupDialog::{Closed, Drawing, Open};

/// The dialog on `screen`; fully drawn once `quiet` is at least
/// [`DIALOG_SETTLE`] (as for the session's own notification).
pub fn startup_text(screen: &Screen, quiet: Duration) -> StartupDialog {
    if !screen.dialog_open() {
        return Closed;
    }
    if quiet < DIALOG_SETTLE {
        return Drawing;
    }
    let text = screen.text();
    let trust = last_of(text, &TRUST_MARKERS);
    let mcp = last_of(text, &[MCP_MARKER]);
    Open(match (trust, mcp) {
        (Some(t), Some(m)) if t > m => TRUST_TEXT,
        (_, Some(_)) => MCP_TEXT,
        (Some(_), None) => TRUST_TEXT,
        (None, None) => OTHER_TEXT,
    })
}

/// The start-up dialog a session of `run` waits at. Only sessions with a
/// pty and no pending turn prompt count (a dialog during a turn is the
/// turn's own prompt). The caller asks only while the node has not started
/// (its message not yet taken).
pub fn startup_dialog(terminals: &Terminals, run: &str) -> StartupDialog {
    terminals
        .all_of(run)
        .iter()
        .filter(|s| s.pending().is_none())
        .filter_map(|s| s.pty())
        .map(|pty| {
            let quiet = pty
                .output
                .last_output()
                .map_or(Duration::ZERO, |t| t.elapsed());
            startup_text(&pty.output.screen(), quiet)
        })
        .find(|d| *d != Closed)
        .unwrap_or(Closed)
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
