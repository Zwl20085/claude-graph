use std::path::Path;
use std::time::Duration;

use super::*;
use crate::contract::{SessionStatus, TerminalSession};
use crate::terminal::api::SessionShared;
use crate::terminal::screen::Screen;

const TRUST: &str = "\x1b[1m\x1b[3;2HAccessing\x1b[1Cworkspace:\x1b[m\x1b[7;2HQuick\x1b[1Csafety\x1b[1Ccheck:\x1b[15;2H\u{276f}\x1b[1CNo,\x1b[1Cexit\x1b[m\x1b[16;4HYes,\x1b[1CI\x1b[1Ctrust\x1b[1Cthis\x1b[1Cfolder\x1b[18;2HEnter\x1b[1Cto\x1b[1Cconfirm\x1b[1C\u{b7}\x1b[1CEsc\x1b[1Cto\x1b[1Ccancel\x1b[m";
const MCP: &str = include_str!("../../tests/fixtures/dialog_mcp.txt");
const IDLE: &str = "fake-claude interactive\r\n  ? for shortcuts\r\n>\u{a0}";
const SETTLED: Duration = Duration::from_secs(1);

#[test]
fn start_up_dialogs_are_named_by_fixed_text() {
    assert_eq!(startup_text(&Screen::of(TRUST), SETTLED), Open(TRUST_TEXT));
    assert_eq!(startup_text(&Screen::of(MCP), SETTLED), Open(MCP_TEXT));
    // Trust answered, then the MCP dialog: the later one is named.
    let both = format!("{TRUST}\r\n{MCP}");
    assert_eq!(startup_text(&Screen::of(&both), SETTLED), Open(MCP_TEXT));
    let fake = "Do you trust the files in this folder?\r\n\u{276f} Yes, proceed\r\nEnter to confirm \u{b7} Esc to exit";
    assert_eq!(startup_text(&Screen::of(fake), SETTLED), Open(TRUST_TEXT));
    // Another dialog (markers only): named generically, never by its text.
    let other = "Pick a theme\r\nEnter to confirm \u{b7} Esc to cancel";
    assert_eq!(startup_text(&Screen::of(other), SETTLED), Open(OTHER_TEXT));
}

#[test]
fn no_dialog_or_one_still_drawing_is_not_reported() {
    assert_eq!(startup_text(&Screen::of(IDLE), SETTLED), Closed);
    assert_eq!(startup_text(&Screen::of(""), SETTLED), Closed);
    // Answered: the input box is drawn after the dialog.
    let answered = format!("{TRUST}\r\n{IDLE}");
    assert_eq!(startup_text(&Screen::of(&answered), SETTLED), Closed);
    // Open but output still arriving (drawn, or redrawn by an arrow key):
    // neither a new dialog nor an answered one.
    assert_eq!(
        startup_text(&Screen::of(TRUST), Duration::from_millis(100)),
        Drawing
    );
}

#[test]
fn the_dialog_text_is_never_taken_from_the_screen() {
    // A server name (from a project file) never reaches the tip.
    assert!(!MCP_TEXT.contains("deepseek"));
    for t in [TRUST_TEXT, MCP_TEXT, OTHER_TEXT] {
        assert!(!t.contains('`') && !t.contains("agent:"), "{t}");
    }
}

fn session(run: &str) -> std::sync::Arc<SessionShared> {
    SessionShared::new(
        run,
        Path::new("/g"),
        TerminalSession {
            sid: "s1".into(),
            title: "T".into(),
            node_ids: vec!["task".into()],
            status: SessionStatus::Starting,
            pid: None,
            alive: true,
            current_node: None,
            harness: "claude".into(),
            pending_prompt: None,
        },
    )
}

#[test]
fn a_run_without_a_pty_has_no_start_up_dialog() {
    let terminals = Terminals::default();
    assert_eq!(startup_dialog(&terminals, "R1"), Closed);
    terminals.insert(session("R1"));
    assert_eq!(startup_dialog(&terminals, "R1"), Closed);
}
