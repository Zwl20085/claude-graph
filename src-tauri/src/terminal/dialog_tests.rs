//! Screens captured from the real Claude Code 2.1.319 (live grid rows,
//! 2026-11-08, docs/CLAUDE_CLI_NOTES.md "Turn outcomes").

use super::*;
use crate::terminal::screen::Markers;

const ASK: &str = include_str!("../../tests/fixtures/dialog_ask_single.txt");
const ASK_DOWN: &str = include_str!("../../tests/fixtures/dialog_ask_down.txt");
const MULTI: &str = include_str!("../../tests/fixtures/dialog_ask_multi.txt");
const REVIEW: &str = include_str!("../../tests/fixtures/dialog_ask_review.txt");
const PERMISSION: &str = include_str!("../../tests/fixtures/dialog_permission.txt");
const IDLE_QUESTION: &str = include_str!("../../tests/fixtures/screen_idle_question.txt");
const INTERRUPTED: &str = include_str!("../../tests/fixtures/screen_interrupted.txt");

fn rows(text: &str) -> Vec<String> {
    text.lines().map(|l| l.trim_end().to_string()).collect()
}

fn claude(text: &str) -> Option<ScreenDialog> {
    let m = Markers::claude();
    parse(&rows(text), &m.idle, &DialogStyle::claude())
}

fn labels(d: &ScreenDialog) -> Vec<&str> {
    d.options.iter().map(|o| o.label.as_str()).collect()
}

#[test]
fn ask_user_question_single() {
    let d = claude(ASK).expect("dialog");
    assert_eq!(d.kind, DialogKind::Choice);
    assert_eq!(d.question, "Which option do you want: A or B?");
    assert_eq!(d.header.as_deref(), Some("A or B"));
    assert_eq!(labels(&d), ["A", "B", "Type something.", "Chat about this"]);
    assert_eq!(d.options[0].detail.as_deref(), Some("Choose option A."));
    assert_eq!(d.selected(), Some(0));
    assert_eq!(d.text_option, Some(2));
    assert!(!d.multi && d.hinted);
    let down = claude(ASK_DOWN).expect("dialog");
    assert_eq!(down.selected(), Some(1));
    assert_eq!(down.signature(), d.signature());
}

#[test]
fn ask_user_question_multi_has_checkboxes_and_submit() {
    let d = claude(MULTI).expect("dialog");
    assert!(d.multi);
    assert_eq!(
        labels(&d),
        ["X", "Y", "Z", "Type something", "Chat about this"]
    );
    assert_eq!(d.checked, [true, false, false, false, false]);
    assert_eq!(d.header.as_deref(), Some("X, Y, or Z"));
    assert!(d.question.starts_with("Which of X, Y, Z do you want?"));
    assert_eq!(
        d.nav,
        [
            NavRow::Option(0),
            NavRow::Option(1),
            NavRow::Option(2),
            NavRow::Option(3),
            NavRow::Submit,
            NavRow::Option(4)
        ]
    );
    assert_eq!(d.text_option, Some(3));
}

#[test]
fn review_screen_is_a_two_option_dialog() {
    let d = claude(REVIEW).expect("dialog");
    assert_eq!(labels(&d), ["Submit answers", "Cancel"]);
    assert_eq!(d.question, "Ready to submit your answers?");
    assert_eq!(d.selected(), Some(0));
}

#[test]
fn permission_prompt() {
    let d = claude(PERMISSION).expect("dialog");
    assert_eq!(d.kind, DialogKind::Permission);
    assert_eq!(d.question, "Do you want to proceed?");
    assert_eq!(d.options.len(), 4);
    assert_eq!(d.options[0].label, "Yes");
    assert_eq!(d.options[3].label, "No");
    assert!(d.options[1]
        .label
        .starts_with("Yes, and always allow access"));
    assert!(d.options[1].detail.is_some(), "wrapped row kept as detail");
    assert_eq!(d.selected(), Some(0));
    assert_eq!(d.text_option, None);
}

#[test]
fn idle_screens_have_no_dialog() {
    assert_eq!(claude(IDLE_QUESTION), None);
    assert_eq!(claude(INTERRUPTED), None);
    // A numbered list in a reply above the empty input box is not a dialog.
    let reply = "\u{25cf} Which framework should I use?\n  1. vitest\n  2. jest\n\u{2500}\u{2500}\u{2500}\n\u{276f}\n\u{2500}\u{2500}\u{2500}";
    assert_eq!(claude(reply), None);
    // Options without cursor or hint do not count.
    assert_eq!(claude("Steps:\n1. build\n2. test"), None);
    assert_eq!(claude("1. only one\nEnter to select"), None);
}

#[test]
fn interrupted_turn_is_recognised() {
    let style = DialogStyle::claude();
    assert!(interrupted(&normalize(INTERRUPTED), &style));
    assert!(!interrupted(&normalize(ASK), &style));
    assert!(!interrupted(
        &normalize(INTERRUPTED),
        &DialogStyle::generic()
    ));
}

#[test]
fn codex_style_and_custom_patterns() {
    let codex = Markers::of(&["Ask Codex to do anything"], &[]);
    let screen = rows(
        "Would you like to run the following command?\n  $ mkdir x\n\u{203a} 1. Yes, proceed (y)\n  2. No, and tell Codex what to do differently (esc)\n\n\u{203a} Ask Codex to do anything",
    );
    // The composer row after the options is the idle box: nothing pending.
    assert_eq!(parse(&screen, &codex.idle, &DialogStyle::generic()), None);
    let open = &screen[..4];
    let d = parse(open, &codex.idle, &DialogStyle::generic()).expect("dialog");
    assert_eq!(d.kind, DialogKind::Permission);
    assert_eq!(d.selected(), Some(0));
    let custom = DialogStyle::custom(
        Some(r"^\s*(?P<cur>\*)?\s*\[(?P<num>\d)\]\s+(?P<label>.+)$"),
        &["pick:".into()],
    );
    let d = parse(&rows("pick:\n* [1] red\n  [2] blue"), &[], &custom).expect("custom");
    assert_eq!(labels(&d), ["red", "blue"]);
    assert!(OptionPattern::new("(").is_err());
    assert!(OptionPattern::new("no label").is_err());
}
