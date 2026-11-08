//! The prompts of a running node (docs/ARCHITECTURE.md "Turn outcomes"):
//! building a [`DialogPrompt`] from the screen and the transcript, opening
//! and clearing a prompt (status `waiting`, events, questions.jsonl), and
//! the dialog keys on the live pty.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use tokio::sync::watch;

use crate::approve::wire::summary;
use crate::contract::{
    AnswerVia, DialogKind, DialogPrompt, NodeEventKind, NodeStatus, PromptOption, PromptSource,
    QuestionAnswer, QuestionRecord, ReplyQuestion, SessionPrompt, SessionPromptCleared, TurnPrompt,
};
use crate::run_store::node_run_dir;
use crate::terminal::answer::DialogIo;
use crate::terminal::deliver::Halt;
use crate::terminal::dialog::{self, NavRow, ScreenDialog};
use crate::terminal::port::{tick, Tick};
use crate::terminal::pty::Pty;
use crate::terminal::questions;
use crate::terminal::session::Session;
use crate::terminal::transcript::{PendingTool, ASK_TOOL};
use crate::terminal::TermMsg;

/// The free-text option AskUserQuestion adds after the given options.
const TYPE_SOMETHING: &str = "Type something.";

/// What the node waits on.
#[derive(Debug, Clone)]
pub(super) enum Pending {
    Dialog {
        prompt: DialogPrompt,
        /// The dialog as last parsed from the screen.
        screen: Option<Box<ScreenDialog>>,
        sig: String,
        /// The transcript's tool call behind the dialog, when known.
        tool_id: Option<String>,
        /// Questions of the AskUserQuestion call (1 for other dialogs).
        questions: usize,
    },
    Reply(ReplyQuestion),
}

impl Pending {
    pub(super) fn id(&self) -> &str {
        match self {
            Pending::Dialog { prompt, .. } => &prompt.id,
            Pending::Reply(q) => &q.id,
        }
    }

    pub(super) fn prompt(&self) -> TurnPrompt {
        match self {
            Pending::Dialog { prompt, .. } => TurnPrompt::Dialog(prompt.clone()),
            Pending::Reply(q) => TurnPrompt::Reply(q.clone()),
        }
    }

    pub(super) fn is_reply(&self) -> bool {
        matches!(self, Pending::Reply(_))
    }
}

/// A dialog found now: its prompt (id not set yet), screen and signature.
pub(super) struct Found {
    pub prompt: DialogPrompt,
    pub screen: Option<ScreenDialog>,
    pub sig: String,
    /// The transcript's tool call behind the dialog, when known.
    pub tool_id: Option<String>,
    /// Questions of the AskUserQuestion call (1 for other dialogs).
    pub questions: usize,
}

impl Found {
    /// `old` (a pending dialog) shows the same dialog: same question and
    /// labels, and not another tool call (unknown on either side matches).
    pub(super) fn same_as(&self, old: &Pending) -> bool {
        match old {
            Pending::Dialog { sig, tool_id, .. } => {
                *sig == self.sig
                    && match (tool_id, &self.tool_id) {
                        (Some(a), Some(b)) => a == b,
                        _ => true,
                    }
            }
            Pending::Reply(_) => false,
        }
    }
}

fn ask_questions(input: &Value) -> usize {
    input
        .get("questions")
        .and_then(Value::as_array)
        .map_or(1, |q| q.len().max(1))
}

/// The AskUserQuestion entry for the screen's `question`; with `None` (no
/// screen) the first. A screen question the call does not hold: `None`.
fn ask_question<'a>(input: &'a Value, question: Option<&str>) -> Option<&'a Value> {
    let qs = input.get("questions")?.as_array()?;
    match question {
        Some(q) => qs
            .iter()
            .find(|x| x.get("question").and_then(Value::as_str) == Some(q)),
        None => qs.first(),
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The dialog on screen (`screen`), with what the transcript says about the
/// waiting tool call (`tool`); or, with no dialog parsed but a pending
/// AskUserQuestion and `transcript_only`, the transcript's question alone.
pub(super) fn found(
    screen: Option<ScreenDialog>,
    tool: Option<&PendingTool>,
    transcript_only: bool,
) -> Option<Found> {
    let Some(sd) = screen else {
        let tool = tool.filter(|t| t.name == ASK_TOOL && transcript_only)?;
        let q = ask_question(&tool.input, None)?;
        let mut labels: Vec<(String, Option<String>)> = q
            .get("options")?
            .as_array()?
            .iter()
            .filter_map(|o| {
                Some((
                    str_of(o, "label")?.to_string(),
                    str_of(o, "description").map(str::to_string),
                ))
            })
            .collect();
        labels.push((TYPE_SOMETHING.to_string(), None));
        let options: Vec<PromptOption> = labels
            .into_iter()
            .enumerate()
            .map(|(index, (label, detail))| PromptOption {
                index,
                label,
                detail,
            })
            .collect();
        let question = str_of(q, "question").unwrap_or("").to_string();
        let sig = format!("transcript\u{1f}{}\u{1f}{}", tool.id, question);
        let questions = ask_questions(&tool.input);
        return Some(Found {
            prompt: DialogPrompt {
                id: String::new(),
                kind: DialogKind::Choice,
                question,
                header: str_of(q, "header").map(str::to_string),
                options,
                selected: None,
                multi: q
                    .get("multiSelect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                allows_text: true,
                source: PromptSource::Transcript,
            },
            screen: None,
            sig,
            tool_id: Some(tool.id.clone()),
            questions,
        });
    };
    let mut prompt = DialogPrompt {
        id: String::new(),
        kind: sd.kind,
        question: sd.question.clone(),
        header: sd.header.clone(),
        options: sd.options.clone(),
        selected: sd.selected(),
        multi: sd.multi,
        allows_text: sd.text_option.is_some(),
        source: PromptSource::Screen,
    };
    let mut questions = 1;
    match tool {
        Some(t) if t.name == ASK_TOOL => {
            questions = ask_questions(&t.input);
            if let Some(q) = ask_question(&t.input, Some(&sd.question)) {
                prompt.kind = DialogKind::Choice;
                prompt.source = PromptSource::Transcript;
                if prompt.header.is_none() {
                    prompt.header = str_of(q, "header").map(str::to_string);
                }
                let described: Vec<(&str, &str)> = q
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|o| {
                        o.iter()
                            .filter_map(|o| Some((str_of(o, "label")?, str_of(o, "description")?)))
                            .collect()
                    })
                    .unwrap_or_default();
                for o in prompt.options.iter_mut().filter(|o| o.detail.is_none()) {
                    if let Some((_, d)) = described.iter().find(|(l, _)| *l == o.label) {
                        o.detail = Some(d.to_string());
                    }
                }
            }
        }
        Some(t) => {
            prompt.source = PromptSource::Transcript;
            prompt.header = Some(format!("{}: {}", t.name, summary(&t.name, &t.input)));
        }
        None => {}
    }
    let sig = sd.signature();
    Some(Found {
        prompt,
        screen: Some(sd),
        sig,
        tool_id: tool.map(|t| t.id.clone()),
        questions,
    })
}

/// A screen dialog for a prompt known only from the transcript.
pub(super) fn synthetic(p: &DialogPrompt) -> ScreenDialog {
    let text_option = p.options.iter().position(|o| {
        let l = o.label.to_lowercase();
        dialog::TEXT_OPTIONS.contains(&l.trim_end_matches('.'))
    });
    ScreenDialog {
        kind: p.kind,
        question: p.question.clone(),
        header: p.header.clone(),
        options: p.options.clone(),
        checked: vec![false; p.options.len()],
        cursor: Some(NavRow::Option(p.selected.unwrap_or(0))),
        nav: (0..p.options.len()).map(NavRow::Option).collect(),
        multi: p.multi,
        text_option,
        hinted: true,
    }
}

/// One-line description of a prompt for node events.
pub(super) fn describe(p: &TurnPrompt) -> String {
    let (question, labels): (&str, Vec<&str>) = match p {
        TurnPrompt::Dialog(d) => (
            &d.question,
            d.options.iter().map(|o| o.label.as_str()).collect(),
        ),
        TurnPrompt::Reply(r) => (
            &r.question,
            r.choices.iter().map(|c| c.label.as_str()).collect(),
        ),
    };
    if labels.is_empty() {
        return question.to_string();
    }
    let list: Vec<String> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{}. {l}", i + 1))
        .collect();
    format!("{question} — {}", list.join(" / "))
}

/// The dialog keys on a session's live pty.
pub(super) struct PtyDialogIo {
    pub pty: Arc<Pty>,
    pub kill: watch::Receiver<bool>,
}

impl DialogIo for PtyDialogIo {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Halt> {
        self.pty
            .write(bytes)
            .map_err(|_| Halt::Exited(self.pty.exit_code().unwrap_or(u32::MAX)))
    }

    async fn wait(&mut self, d: Duration) -> Result<(), Halt> {
        match tick(&mut self.kill, Some(self.pty.exit_watch()), d).await {
            Tick::Poll => Ok(()),
            Tick::Killed => Err(Halt::Killed),
            Tick::Exited(code) => Err(Halt::Exited(code)),
        }
    }

    fn dialog(&self) -> Option<ScreenDialog> {
        let m = self.pty.output.markers();
        dialog::parse(&self.pty.output.rows(), &m.idle, &m.style)
    }
}

impl Session {
    /// A node's status changes without starting or ending (waiting ↔ running).
    pub(super) fn node_status(&self, node_id: &str, status: NodeStatus) {
        let _ = self.t.tx.send(TermMsg::Node {
            node_id: node_id.to_string(),
            status,
            reason: None,
            started_at: None,
            finished_at: None,
        });
    }

    /// The next prompt id of `node_id` (`<node>-<sid>-p<n>`: unique in the
    /// run even when the node runs again in another session).
    pub(super) fn next_prompt_id(&mut self, node_id: &str) -> String {
        self.prompt_seq += 1;
        format!("{node_id}-{}-p{}", self.t.plan.sid, self.prompt_seq)
    }

    /// The pending prompt changed (more detail known): publish it again
    /// without a new record or event.
    pub(super) fn republish_prompt(&mut self, node_id: &str, pending: &Pending) {
        let sp = SessionPrompt {
            run_id: self.t.ctx.run_id.clone(),
            sid: self.t.plan.sid.clone(),
            node_id: node_id.to_string(),
            prompt: pending.prompt(),
        };
        self.t.shared.set_pending(Some(sp.clone()));
        self.t.ctx.emitter.session_prompt(&sp);
    }

    /// The node waits for the user: publish `pending`, record it.
    pub(super) fn open_prompt(&mut self, node_id: &str, pending: &Pending) {
        let prompt = pending.prompt();
        let sp = SessionPrompt {
            run_id: self.t.ctx.run_id.clone(),
            sid: self.t.plan.sid.clone(),
            node_id: node_id.to_string(),
            prompt: prompt.clone(),
        };
        self.t.shared.set_pending(Some(sp.clone()));
        let id = prompt.id().to_string();
        self.report(|i| i.pending_prompt = Some(id));
        self.t.ctx.emitter.session_prompt(&sp);
        self.record(node_id, QuestionRecord::of(&prompt, Utc::now()));
        let kind = if pending.is_reply() {
            NodeEventKind::Question
        } else {
            NodeEventKind::Dialog
        };
        self.event(kind, describe(&prompt));
        self.node_status(node_id, NodeStatus::Waiting);
        self.stats.questions += 1;
        self.stats.waited = true;
    }

    pub(super) fn record(&self, node_id: &str, record: QuestionRecord) {
        let ctx = &self.t.ctx;
        let dir = node_run_dir(&ctx.graph_dir, &ctx.run_id, node_id);
        if let Err(e) = questions::add(&dir, record) {
            log::warn!("writing questions.jsonl of {node_id} failed: {e}");
        }
    }

    /// The prompt `pending` is resolved `via`; `answer` (if any) goes into
    /// its record; `running`: the node goes back to `running`.
    pub(super) fn clear_prompt(
        &mut self,
        node_id: &str,
        pending: &Pending,
        answer: Option<QuestionAnswer>,
        via: AnswerVia,
        running: bool,
    ) {
        let id = pending.id().to_string();
        self.t.shared.set_pending(None);
        self.report(|i| i.pending_prompt = None);
        self.t
            .ctx
            .emitter
            .session_prompt_cleared(&SessionPromptCleared {
                run_id: self.t.ctx.run_id.clone(),
                sid: self.t.plan.sid.clone(),
                node_id: node_id.to_string(),
                id: id.clone(),
                via,
            });
        let answer = answer.unwrap_or(QuestionAnswer {
            ts: Utc::now(),
            via,
            text: None,
            index: None,
        });
        let text = match (&answer.via, &answer.text, answer.index) {
            (AnswerVia::Continue, _, _) => "continued without answering".to_string(),
            (AnswerVia::Window, None, _) => "answered in the window".to_string(),
            (AnswerVia::Window, Some(t), _) => format!("answered in the window: {t}"),
            (AnswerVia::App, Some(t), _) => format!("answered from the app: {t}"),
            (AnswerVia::App, None, Some(i)) => format!("answered from the app: option {}", i + 1),
            (AnswerVia::App, None, None) => "answered from the app".to_string(),
        };
        let ctx = &self.t.ctx;
        let dir = node_run_dir(&ctx.graph_dir, &ctx.run_id, node_id);
        if let Err(e) = questions::answer(&dir, &id, answer) {
            log::warn!("writing the answer of {id} failed: {e}");
        }
        self.event(NodeEventKind::Answer, text);
        if running {
            self.node_status(node_id, NodeStatus::Running);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, input: Value) -> PendingTool {
        PendingTool {
            id: "t1".into(),
            name: name.into(),
            input,
        }
    }

    fn ask_input() -> Value {
        json!({"questions": [{"question": "A or B?", "header": "AB", "multiSelect": false,
            "options": [{"label": "A", "description": "first"}, {"label": "B", "description": "second"}]}]})
    }

    fn screen(labels: &[&str], kind: DialogKind) -> ScreenDialog {
        let p = DialogPrompt {
            id: String::new(),
            kind,
            question: "A or B?".into(),
            header: None,
            options: labels
                .iter()
                .enumerate()
                .map(|(index, l)| PromptOption {
                    index,
                    label: l.to_string(),
                    detail: None,
                })
                .collect(),
            selected: Some(0),
            multi: false,
            allows_text: false,
            source: PromptSource::Screen,
        };
        synthetic(&p)
    }

    #[test]
    fn transcript_enriches_the_screen_dialog() {
        let t = tool(ASK_TOOL, ask_input());
        let f = found(
            Some(screen(&["A", "B", "Type something."], DialogKind::Choice)),
            Some(&t),
            false,
        )
        .unwrap();
        assert_eq!(f.prompt.source, PromptSource::Transcript);
        assert_eq!(f.prompt.header.as_deref(), Some("AB"));
        assert_eq!(f.prompt.options[1].detail.as_deref(), Some("second"));
        assert!(f.prompt.allows_text);
        let bash = tool("Bash", json!({"command": "mkdir x"}));
        let f = found(
            Some(screen(&["Yes", "No"], DialogKind::Permission)),
            Some(&bash),
            false,
        )
        .unwrap();
        assert_eq!(f.prompt.header.as_deref(), Some("Bash: mkdir x"));
        assert_eq!(f.prompt.kind, DialogKind::Permission);
    }

    #[test]
    fn transcript_alone_gives_the_ask_dialog_only_when_allowed() {
        let t = tool(ASK_TOOL, ask_input());
        assert!(found(None, Some(&t), false).is_none());
        let f = found(None, Some(&t), true).unwrap();
        let labels: Vec<&str> = f.prompt.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["A", "B", TYPE_SOMETHING]);
        assert_eq!(synthetic(&f.prompt).text_option, Some(2));
        let bash = tool("Bash", json!({"command": "ls"}));
        assert!(found(None, Some(&bash), true).is_none());
        assert!(found(None, None, true).is_none());
    }

    #[test]
    fn another_tool_call_with_the_same_labels_is_another_dialog() {
        let old_for = |tool_id: Option<&str>| {
            let f = found(
                Some(screen(&["Yes", "No"], DialogKind::Permission)),
                None,
                false,
            )
            .unwrap();
            Pending::Dialog {
                prompt: f.prompt,
                screen: None,
                sig: f.sig,
                tool_id: tool_id.map(str::to_string),
                questions: 1,
            }
        };
        let push = tool("Bash", json!({"command": "git push --force"}));
        let mut now = found(
            Some(screen(&["Yes", "No"], DialogKind::Permission)),
            Some(&push),
            false,
        )
        .unwrap();
        now.tool_id = Some("t2".into());
        assert!(!now.same_as(&old_for(Some("t1"))), "git status vs git push");
        assert!(now.same_as(&old_for(Some("t2"))));
        assert!(now.same_as(&old_for(None)), "the transcript caught up");
        let other = found(
            Some(screen(&["Yes", "No", "Maybe"], DialogKind::Permission)),
            None,
            false,
        )
        .unwrap();
        assert!(!other.same_as(&old_for(None)));
    }

    #[test]
    fn a_screen_question_the_call_does_not_hold_gets_no_details() {
        let two = json!({"questions": [
            {"question": "First?", "options": [{"label": "A", "description": "a"}]},
            {"question": "A or B?", "options": [{"label": "A", "description": "second q"}]}]});
        let t = tool(ASK_TOOL, two);
        let f = found(
            Some(screen(&["A", "B"], DialogKind::Choice)),
            Some(&t),
            false,
        )
        .unwrap();
        assert_eq!(f.questions, 2);
        assert_eq!(f.prompt.options[0].detail.as_deref(), Some("second q"));
        let other = json!({"questions": [{"question": "Else?", "options": [{"label": "A", "description": "x"}]}]});
        let t = tool(ASK_TOOL, other);
        let f = found(
            Some(screen(&["A", "B"], DialogKind::Choice)),
            Some(&t),
            false,
        )
        .unwrap();
        assert_eq!(f.prompt.options[0].detail, None, "not the first question's");
    }

    #[test]
    fn descriptions_list_the_options() {
        let f = found(None, Some(&tool(ASK_TOOL, ask_input())), true).unwrap();
        let d = describe(&TurnPrompt::Dialog(f.prompt));
        assert_eq!(d, "A or B? — 1. A / 2. B / 3. Type something.");
    }
}
