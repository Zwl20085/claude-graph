//! The commands behind turn outcomes (docs/ARCHITECTURE.md "Turn outcomes"):
//! `answer_prompt`, `continue_node`, `list_session_prompts`. Every id and
//! answer is checked against the session's pending prompt here; the session
//! task does the typing.

use crate::contract::{AppError, AppResult, DialogKind, PromptAnswer, SessionPrompt, TurnPrompt};
use crate::run_store::check_id;
use crate::terminal::api::{check_sid, PromptAction, Terminals, WRITE_MAX};

/// The error for a free-text answer to a prompt without a text option.
pub const ONLY_OPTIONS: &str = "this prompt only takes one of its options";

fn stale(prompt_id: &str) -> AppError {
    AppError::new(
        "not_found",
        format!("prompt {prompt_id} is no longer waiting (answered or gone)"),
    )
}

/// `answer` is valid for `prompt`.
pub fn check_answer(prompt: &TurnPrompt, answer: &PromptAnswer) -> AppResult<()> {
    let bad = |msg: &str| Err(AppError::new("invalid_input", msg.to_string()));
    let (count, multi, text_ok) = match prompt {
        TurnPrompt::Dialog(d) => (
            d.options.len(),
            d.multi,
            d.allows_text || d.kind == DialogKind::Text,
        ),
        TurnPrompt::Reply(r) => (r.choices.len(), false, true),
    };
    match answer {
        PromptAnswer::Choice(i) if *i < count => Ok(()),
        PromptAnswer::Choice(i) => bad(&format!("no option {i}: the prompt has {count}")),
        PromptAnswer::Choices(_) if !multi => bad("this prompt takes one option, not several"),
        PromptAnswer::Choices(list) => match list.iter().find(|i| **i >= count) {
            Some(i) => bad(&format!("no option {i}: the prompt has {count}")),
            None => Ok(()),
        },
        PromptAnswer::Text(_) if !text_ok => bad(ONLY_OPTIONS),
        PromptAnswer::Text(t) if t.trim().is_empty() => bad("the answer is empty"),
        PromptAnswer::Text(t) if t.len() > WRITE_MAX => bad(&format!(
            "the answer is {} bytes; at most {WRITE_MAX}",
            t.len()
        )),
        PromptAnswer::Text(_) => Ok(()),
    }
}

/// Answer the pending prompt `prompt_id` of session `sid`.
pub fn answer_prompt(
    terminals: &Terminals,
    run_id: &str,
    sid: &str,
    prompt_id: &str,
    answer: PromptAnswer,
) -> AppResult<()> {
    check_id("run id", run_id)?;
    check_sid(sid)?;
    let session = terminals.get(run_id, sid)?;
    let pending = session
        .pending()
        .filter(|p| p.prompt.id() == prompt_id)
        .ok_or_else(|| stale(prompt_id))?;
    check_answer(&pending.prompt, &answer)?;
    session.act(PromptAction::Answer {
        prompt_id: prompt_id.to_string(),
        answer,
    })
}

/// Mark the node waiting on a reply question done (status by deliverables).
pub fn continue_node(
    terminals: &Terminals,
    run_id: &str,
    sid: &str,
    node_id: &str,
) -> AppResult<()> {
    check_id("run id", run_id)?;
    check_sid(sid)?;
    check_id("node id", node_id)?;
    let session = terminals.get(run_id, sid)?;
    match session.pending() {
        Some(SessionPrompt {
            node_id: n,
            prompt: TurnPrompt::Reply(_),
            ..
        }) if n == node_id => session.act(PromptAction::Continue {
            node_id: node_id.to_string(),
        }),
        _ => Err(AppError::new(
            "invalid_input",
            format!("node {node_id} is not waiting on a question"),
        )),
    }
}

/// The pending prompts of a run's live sessions (for a reloaded window).
pub fn list_session_prompts(terminals: &Terminals, run_id: &str) -> AppResult<Vec<SessionPrompt>> {
    check_id("run id", run_id)?;
    Ok(terminals
        .all_of(run_id)
        .iter()
        .filter_map(|s| s.pending())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{
        DialogPrompt, PromptOption, PromptSource, ReplyChoice, ReplyQuestion, SessionStatus,
        TerminalSession,
    };
    use crate::terminal::api::SessionShared;
    use std::path::Path;

    fn dialog(multi: bool, text: bool) -> TurnPrompt {
        TurnPrompt::Dialog(DialogPrompt {
            id: "n-p1".into(),
            kind: DialogKind::Choice,
            question: "A or B?".into(),
            header: None,
            options: ["A", "B"]
                .iter()
                .enumerate()
                .map(|(index, l)| PromptOption {
                    index,
                    label: l.to_string(),
                    detail: None,
                })
                .collect(),
            selected: Some(0),
            multi,
            allows_text: text,
            source: PromptSource::Screen,
        })
    }

    fn reply() -> TurnPrompt {
        TurnPrompt::Reply(ReplyQuestion {
            id: "n-p2".into(),
            question: "Which?".into(),
            choices: vec![ReplyChoice {
                label: "vitest".into(),
                detail: None,
            }],
            reply_excerpt: "Which?".into(),
        })
    }

    #[test]
    fn answers_are_checked_against_the_prompt() {
        let d = dialog(false, false);
        assert!(check_answer(&d, &PromptAnswer::Choice(1)).is_ok());
        assert!(check_answer(&d, &PromptAnswer::Choice(2)).is_err());
        let err = check_answer(&d, &PromptAnswer::Text("x".into())).unwrap_err();
        assert_eq!(err.message, ONLY_OPTIONS);
        assert!(check_answer(&d, &PromptAnswer::Choices(vec![0])).is_err());
        let m = dialog(true, true);
        assert!(check_answer(&m, &PromptAnswer::Choices(vec![0, 1])).is_ok());
        assert!(check_answer(&m, &PromptAnswer::Choices(vec![5])).is_err());
        assert!(check_answer(&m, &PromptAnswer::Text("own".into())).is_ok());
        let r = reply();
        assert!(check_answer(&r, &PromptAnswer::Text("use jest".into())).is_ok());
        assert!(check_answer(&r, &PromptAnswer::Text("  ".into())).is_err());
        assert!(check_answer(&r, &PromptAnswer::Choice(0)).is_ok());
    }

    #[test]
    fn answer_and_continue_need_the_pending_prompt() {
        let terminals = Terminals::default();
        let info = TerminalSession {
            sid: "s1".into(),
            title: "T".into(),
            node_ids: vec!["n".into()],
            status: SessionStatus::Running,
            pid: None,
            current_node: Some("n".into()),
            harness: "claude".into(),
            pending_prompt: None,
        };
        let s = SessionShared::new("R1", Path::new("/g"), info);
        terminals.insert(s.clone());
        let mut rx = s.take_actions().unwrap();
        let err = answer_prompt(&terminals, "R1", "s1", "n-p1", PromptAnswer::Choice(0));
        assert_eq!(err.unwrap_err().code, "not_found");
        assert!(continue_node(&terminals, "R1", "s1", "n").is_err());
        s.set_pending(Some(SessionPrompt {
            run_id: "R1".into(),
            sid: "s1".into(),
            node_id: "n".into(),
            prompt: reply(),
        }));
        answer_prompt(&terminals, "R1", "s1", "n-p2", PromptAnswer::Choice(0)).unwrap();
        continue_node(&terminals, "R1", "s1", "n").unwrap();
        assert!(continue_node(&terminals, "R1", "s1", "other").is_err());
        assert!(answer_prompt(&terminals, "R1", "s9x", "n-p2", PromptAnswer::Choice(0)).is_err());
        assert!(matches!(
            rx.try_recv().unwrap(),
            PromptAction::Answer { .. }
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            PromptAction::Continue { .. }
        ));
        assert_eq!(list_session_prompts(&terminals, "R1").unwrap().len(), 1);
        // A Messages-tab message becomes the answer of a pending reply question.
        terminals
            .type_message(
                "R1",
                "n",
                "use jest",
                std::sync::Arc::new(crate::events::NullEmitter),
            )
            .unwrap();
        assert_eq!(
            rx.try_recv().unwrap(),
            PromptAction::Answer {
                prompt_id: "n-p2".into(),
                answer: PromptAnswer::Text("use jest".into())
            }
        );
    }
}
