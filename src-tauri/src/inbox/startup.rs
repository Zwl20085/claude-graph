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
use crate::terminal::screen::{normalize, Screen};

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

/// The kind of the dialog shown on the live screen `rows` (the terminal
/// grid as it is now, not the output stream): the bottom-most row that names
/// a known dialog wins, so text of an answered dialog that the next one drew
/// over never counts. A dialog no row names is [`OTHER_TEXT`].
fn kind_of(rows: &[String]) -> &'static str {
    rows.iter()
        .rev()
        .map(|r| normalize(r))
        .find_map(|r| {
            if r.contains(MCP_MARKER) {
                Some(MCP_TEXT)
            } else if TRUST_MARKERS.iter().any(|m| r.contains(m)) {
                Some(TRUST_TEXT)
            } else {
                None
            }
        })
        .unwrap_or(OTHER_TEXT)
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

/// Whether a dialog waits per `screen` (the session's own rule,
/// [`Screen::dialog_open`]); fully drawn once `quiet` is at least
/// [`DIALOG_SETTLE`] (as for the session's own notification); its kind from
/// the live grid `rows`.
pub fn startup_text(screen: &Screen, rows: &[String], quiet: Duration) -> StartupDialog {
    if !screen.dialog_open() {
        return Closed;
    }
    if quiet < DIALOG_SETTLE {
        return Drawing;
    }
    Open(kind_of(rows))
}

/// The start-up dialog a live session of `run` waits at. Only sessions whose
/// task is alive (a dead pty keeps its last screen), with a pty and no
/// pending turn prompt count (a dialog during a turn is the turn's own
/// prompt). The caller asks only while the node has not started (its
/// message not yet taken).
pub fn startup_dialog(terminals: &Terminals, run: &str) -> StartupDialog {
    terminals
        .all_of(run)
        .iter()
        .filter(|s| s.is_task_alive() && s.pending().is_none())
        .filter_map(|s| s.pty())
        .map(|pty| {
            let quiet = pty
                .output
                .last_output()
                .map_or(Duration::ZERO, |t| t.elapsed());
            startup_text(&pty.output.screen(), &pty.output.rows(), quiet)
        })
        .find(|d| *d != Closed)
        .unwrap_or(Closed)
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
