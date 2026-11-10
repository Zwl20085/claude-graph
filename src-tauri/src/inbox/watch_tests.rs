use super::*;
use crate::contract::{DialogKind, DialogPrompt, PromptOption, PromptSource, ReplyQuestion};

#[test]
fn the_echoed_pointer_line_is_dropped() {
    let r = "> Node \"Board task\" (task): read the brief at C:/x/brief.md and do it\n\nFound 3.\n";
    assert_eq!(without_echo(r), "Found 3.\n");
    assert_eq!(without_echo("Found 3.\n"), "Found 3.\n");
    assert_eq!(without_echo("> a quote\n\nx"), "> a quote\n\nx");
    assert_eq!(without_echo("> Node read the brief at x"), "");
}

fn prompt(p: TurnPrompt) -> SessionPrompt {
    SessionPrompt {
        run_id: "r".into(),
        sid: "s1".into(),
        node_id: NODE_ID.into(),
        prompt: p,
    }
}

#[test]
fn prompts_become_question_or_dialog_outcomes() {
    let reply = ReplyQuestion {
        id: "q".into(),
        question: "Which?".into(),
        choices: vec![crate::contract::ReplyChoice {
            label: "a".into(),
            detail: None,
        }],
        reply_excerpt: String::new(),
    };
    assert_eq!(
        question_text(&prompt(TurnPrompt::Reply(reply.clone()))),
        Outcome::Question {
            text: "Which?\n1. a".into()
        }
    );
    let with_excerpt = ReplyQuestion {
        reply_excerpt: "1. Years?\n2. arXiv?".into(),
        ..reply
    };
    assert_eq!(
        question_text(&prompt(TurnPrompt::Reply(with_excerpt))),
        Outcome::Question {
            text: "1. Years?\n2. arXiv?".into()
        }
    );
    let dialog = DialogPrompt {
        id: "d".into(),
        kind: DialogKind::Permission,
        question: "Do you want to proceed?".into(),
        header: Some("Bash: mkdir x".into()),
        options: vec![
            PromptOption {
                index: 0,
                label: "Yes".into(),
                detail: None,
            },
            PromptOption {
                index: 1,
                label: "No".into(),
                detail: None,
            },
        ],
        selected: Some(0),
        multi: false,
        allows_text: false,
        source: PromptSource::Screen,
    };
    assert_eq!(
        question_text(&prompt(TurnPrompt::Dialog(dialog))),
        Outcome::Dialog {
            text: "Bash: mkdir x: Do you want to proceed? (Yes / No)".into()
        }
    );
}
