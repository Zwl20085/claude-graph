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

/// The live grid after `raw` (as the session's emulator holds it; line ends
/// as a pty delivers them, `\r\n`).
fn grid(raw: &str) -> Vec<String> {
    let n = |v| std::num::NonZeroU16::new(v).unwrap();
    let mut p = vt100::Parser::new(n(50), n(160), 0);
    let crlf = raw.replace("\r\n", "\n").replace('\n', "\r\n");
    p.process(crlf.as_bytes());
    p.screen().rows(0, 160).collect()
}

/// [`startup_text`] on the screen and the grid of the same output.
fn seen(raw: &str, quiet: Duration) -> StartupDialog {
    startup_text(&Screen::of(raw), &grid(raw), quiet)
}

/// C5: the kind comes from what the grid shows now, not from the order of
/// markers in the output stream.
#[test]
fn the_kind_is_read_from_the_live_grid() {
    let screen = Screen::of(&format!("{TRUST}\r\n{MCP}"));
    // The grid shows only the trust dialog (the other was drawn over).
    assert_eq!(
        startup_text(&screen, &grid(TRUST), SETTLED),
        Open(TRUST_TEXT)
    );
    assert_eq!(startup_text(&screen, &grid(MCP), SETTLED), Open(MCP_TEXT));
    // Both show: the bottom-most named row wins.
    let rows = vec![
        "Do you trust the files?".to_string(),
        "New MCP server found".to_string(),
    ];
    assert_eq!(startup_text(&screen, &rows, SETTLED), Open(MCP_TEXT));
    // No row names a known dialog: the generic text.
    assert_eq!(
        startup_text(&screen, &[String::new()], SETTLED),
        Open(OTHER_TEXT)
    );
}

#[test]
fn start_up_dialogs_are_named_by_fixed_text() {
    assert_eq!(seen(TRUST, SETTLED), Open(TRUST_TEXT));
    assert_eq!(seen(MCP, SETTLED), Open(MCP_TEXT));
    // Trust answered, then the MCP dialog: the later one is named.
    let both = format!("{TRUST}\r\n{MCP}");
    assert_eq!(seen(&both, SETTLED), Open(MCP_TEXT));
    let fake = "Do you trust the files in this folder?\r\n\u{276f} Yes, proceed\r\nEnter to confirm \u{b7} Esc to exit";
    assert_eq!(seen(fake, SETTLED), Open(TRUST_TEXT));
    // Another dialog (markers only): named generically, never by its text.
    let other = "Pick a theme\r\nEnter to confirm \u{b7} Esc to cancel";
    assert_eq!(seen(other, SETTLED), Open(OTHER_TEXT));
}

#[test]
fn no_dialog_or_one_still_drawing_is_not_reported() {
    assert_eq!(seen(IDLE, SETTLED), Closed);
    assert_eq!(seen("", SETTLED), Closed);
    // Answered: the input box is drawn after the dialog.
    let answered = format!("{TRUST}\r\n{IDLE}");
    assert_eq!(seen(&answered, SETTLED), Closed);
    // Open but output still arriving (drawn, or redrawn by an arrow key):
    // neither a new dialog nor an answered one.
    assert_eq!(seen(TRUST, Duration::from_millis(100)), Drawing);
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
