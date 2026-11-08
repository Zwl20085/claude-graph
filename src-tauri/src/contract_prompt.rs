//! Turn outcomes in terminal mode (docs/ARCHITECTURE.md "Turn outcomes"):
//! the questions and dialogs a session waits on, their answers, and the
//! records in `runs/<run>/<node>/questions.jsonl`. Re-exported by
//! [`crate::contract`]; mirrored in `src/types/contract.ts`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Event name: a [`SessionPrompt`] (a node waits for the user).
pub const EVENT_SESSION_PROMPT: &str = "session_prompt";
/// Event name: a [`SessionPromptCleared`] (answered, continued or gone).
pub const EVENT_SESSION_PROMPT_CLEARED: &str = "session_prompt_cleared";

/// What kind of interactive prompt is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogKind {
    /// A list of choices (Claude Code's AskUserQuestion, a numbered menu).
    Choice,
    /// A tool call waiting for approval ("Do you want to proceed?").
    Permission,
    /// A plain text or `(y/n)` prompt.
    Text,
    Unknown,
}

/// Where a prompt was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSource {
    /// The CLI's transcript JSONL (Claude Code).
    Transcript,
    /// The live terminal screen.
    Screen,
    /// The final text of the reply.
    Reply,
    /// The run events of a headless harness (dsh).
    Events,
}

/// How a prompt was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerVia {
    /// From the app (Answer / a choice button / the Messages tab).
    App,
    /// Directly in the session window (or the dialog went away).
    Window,
    /// "Continue without answering".
    Continue,
}

/// One option of a dialog; `index` is its 0-based position in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOption {
    pub index: usize,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// An interactive prompt shown while the turn is still running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogPrompt {
    pub id: String,
    pub kind: DialogKind,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<PromptOption>,
    /// Index of the highlighted option, when the screen shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
    /// Several options may be picked (checkboxes).
    pub multi: bool,
    /// A free-text answer is possible ("Type something", a text prompt).
    pub allows_text: bool,
    pub source: PromptSource,
}

/// A choice offered in the reply text (a numbered or lettered list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyChoice {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The turn ended and its reply asks the user something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyQuestion {
    pub id: String,
    pub question: String,
    pub choices: Vec<ReplyChoice>,
    /// The end of the reply (at most 1200 chars).
    pub reply_excerpt: String,
}

/// A dialog (turn running) or a reply question (turn ended).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnPrompt {
    Dialog(DialogPrompt),
    Reply(ReplyQuestion),
}

impl TurnPrompt {
    pub fn id(&self) -> &str {
        match self {
            TurnPrompt::Dialog(d) => &d.id,
            TurnPrompt::Reply(r) => &r.id,
        }
    }

    pub fn question(&self) -> &str {
        match self {
            TurnPrompt::Dialog(d) => &d.question,
            TurnPrompt::Reply(r) => &r.question,
        }
    }
}

/// Payload of `session_prompt`; entry of `list_session_prompts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPrompt {
    pub run_id: String,
    pub sid: String,
    pub node_id: String,
    pub prompt: TurnPrompt,
}

/// Payload of `session_prompt_cleared`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPromptCleared {
    pub run_id: String,
    pub sid: String,
    pub node_id: String,
    pub id: String,
    pub via: AnswerVia,
}

/// The `answer` of `answer_prompt`: `{"choice": 1}`, `{"choices": [0, 2]}`
/// (multi-select) or `{"text": "…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAnswer {
    Choice(usize),
    Choices(Vec<usize>),
    Text(String),
}

/// How a recorded question was answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub ts: DateTime<Utc>,
    pub via: AnswerVia,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// Kind of a questions.jsonl record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Question,
    Dialog,
}

/// One line of `runs/<run>/<node>/questions.jsonl` (the file is rewritten
/// when an answer is added to its record).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRecord {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub kind: QuestionKind,
    pub source: PromptSource,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<ReplyChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
    /// A reply question that did not pause the node (its deliverables were
    /// all written: the question counts as an offer).
    #[serde(default)]
    pub offer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<QuestionAnswer>,
}

impl QuestionRecord {
    /// A new record of `prompt` (no answer yet).
    pub fn of(prompt: &TurnPrompt, ts: DateTime<Utc>) -> Self {
        match prompt {
            TurnPrompt::Dialog(d) => QuestionRecord {
                id: d.id.clone(),
                ts,
                kind: QuestionKind::Dialog,
                source: d.source,
                question: d.question.clone(),
                header: d.header.clone(),
                options: d
                    .options
                    .iter()
                    .map(|o| ReplyChoice {
                        label: o.label.clone(),
                        detail: o.detail.clone(),
                    })
                    .collect(),
                selected: d.selected,
                offer: false,
                answer: None,
            },
            TurnPrompt::Reply(r) => QuestionRecord {
                id: r.id.clone(),
                ts,
                kind: QuestionKind::Question,
                source: PromptSource::Reply,
                question: r.question.clone(),
                header: None,
                options: r.choices.clone(),
                selected: None,
                offer: false,
                answer: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Shared with the frontend (`src/types/fixtures/session_prompt.json`).
    const FIXTURE: &str = include_str!("../../src/types/fixtures/session_prompt.json");

    #[test]
    fn session_prompt_matches_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        let dialog: SessionPrompt = serde_json::from_value(fixture["dialog"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&dialog).unwrap(), fixture["dialog"]);
        let TurnPrompt::Dialog(d) = &dialog.prompt else {
            panic!("dialog expected")
        };
        assert_eq!(
            (d.kind, d.options.len(), d.selected),
            (DialogKind::Choice, 3, Some(0))
        );
        let reply: SessionPrompt = serde_json::from_value(fixture["reply"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&reply).unwrap(), fixture["reply"]);
        assert_eq!(reply.prompt.question(), "Which framework should I use?");
        let cleared: SessionPromptCleared =
            serde_json::from_value(fixture["cleared"].clone()).unwrap();
        assert_eq!(cleared.via, AnswerVia::Window);
        for a in fixture["answers"].as_array().unwrap() {
            let parsed: PromptAnswer = serde_json::from_value(a.clone()).unwrap();
            assert_eq!(&serde_json::to_value(&parsed).unwrap(), a);
        }
    }

    #[test]
    fn answers_are_externally_tagged() {
        assert_eq!(
            serde_json::to_value(PromptAnswer::Choice(2)).unwrap(),
            json!({"choice": 2})
        );
        assert!(serde_json::from_value::<PromptAnswer>(json!({"pick": 1})).is_err());
    }

    #[test]
    fn records_take_the_prompt_fields() {
        let r = ReplyQuestion {
            id: "q1".into(),
            question: "Which?".into(),
            choices: vec![ReplyChoice {
                label: "vitest".into(),
                detail: None,
            }],
            reply_excerpt: "Which?".into(),
        };
        let rec = QuestionRecord::of(&TurnPrompt::Reply(r), Utc::now());
        assert_eq!(rec.kind, QuestionKind::Question);
        assert_eq!(rec.source, PromptSource::Reply);
        let line = serde_json::to_string(&rec).unwrap();
        assert!(!line.contains("answer") && line.contains("\"kind\":\"question\""));
    }
}
