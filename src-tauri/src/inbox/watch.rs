//! Following one started run until its outcome is written to the tip.
//!
//! The node's status (from the run's own `run:state` events) decides:
//! succeeded -> `done`; failed / cancelled / skipped / blocked -> `failed`;
//! waiting -> `question` (a reply question, or a dialog in the window) while
//! the session stays open; if the owner then answers in the window and the
//! node finishes, that outcome is written as one more block. Working time
//! over the timeout cancels the run (`failed`). Nothing is written once the
//! tip was superseded by a newer run or the app is closing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::contract::{InboxPhase, NodeStatus, RunState, SessionPrompt, TurnPrompt};
use crate::run_loop::blocking;
use crate::run_store::{node_run_dir, read_node_result, RESULT_FILE};
use crate::store::deliverables_dir;
use crate::terminal::prompt_api::list_session_prompts;

use super::block::Outcome;
use super::graphgen::NODE_ID;
use super::ops::Claimant;
use super::results::{copy_round, results_dir};
use super::runner::Inbox;
use super::start::Launched;
use super::writeback::{claim_holds, finish_file};

/// A waiting node with no prompt listed after this long is reported anyway.
const PROMPT_GRACE: Duration = Duration::from_secs(10);

/// One claimed round of a tip.
#[derive(Debug, Clone)]
pub(crate) struct Round {
    pub tip_id: String,
    pub path: PathBuf,
    pub folder: PathBuf,
    /// Protocol run id (`agent.run`).
    pub run: String,
    pub graph_dir: PathBuf,
    pub graph_run: Option<String>,
    /// The seen body, then the body as this run last left it.
    pub expected_body: String,
}

impl Inbox {
    pub(crate) fn set_phase(&self, tip: &str, phase: InboxPhase) {
        if let Some(a) = self.lock().active.get_mut(tip) {
            a.run.phase = phase;
        }
        self.emit();
    }

    fn set_note(&self, tip: &str, note: String) {
        if let Some(a) = self.lock().active.get_mut(tip) {
            a.run.note = Some(note);
        }
    }

    fn set_sid(&self, tip: &str, state: &RunState) {
        let sid = state.sessions.first().map(|s| s.sid.clone());
        if let Some(a) = self.lock().active.get_mut(tip) {
            if a.run.sid.is_none() && sid.is_some() {
                a.run.sid = sid;
            }
        }
    }
}

fn reply_of(round: &Round) -> String {
    let Some(run) = round.graph_run.as_deref() else {
        return String::new();
    };
    let text = read_node_result(&round.graph_dir, run, NODE_ID)
        .ok()
        .and_then(|r| r.result_md)
        .unwrap_or_default();
    without_echo(&text).to_string()
}

/// The reply without the echoed pointer line the TUI shows first (`> Node
/// "Board task" (task): read the brief at …`, up to the first blank line):
/// it only names local paths of this machine.
pub(crate) fn without_echo(reply: &str) -> &str {
    let trimmed = reply.trim_start_matches(['\r', '\n']);
    let first = trimmed.lines().next().unwrap_or("");
    if !(first.starts_with("> ") && first.contains("read the brief at")) {
        return reply;
    }
    match trimmed.find("\n\n").or_else(|| trimmed.find("\r\n\r\n")) {
        Some(i) => trimmed[i..].trim_start_matches(['\r', '\n']),
        None => "",
    }
}

fn question_text(p: &SessionPrompt) -> Outcome {
    match &p.prompt {
        TurnPrompt::Reply(r) => {
            let excerpt = r.reply_excerpt.trim();
            let text = if excerpt.is_empty() {
                let choices: Vec<String> = r
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {}", i + 1, c.label))
                    .collect();
                [r.question.clone()]
                    .into_iter()
                    .chain(choices)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else if excerpt.chars().count() >= crate::terminal::outcome::EXCERPT_MAX {
                // Cut at the front: start at a whole line.
                excerpt
                    .split_once('\n')
                    .map_or(excerpt, |(_, rest)| rest)
                    .to_string()
            } else {
                excerpt.to_string()
            };
            Outcome::Question { text }
        }
        TurnPrompt::Dialog(d) => {
            let options: Vec<&str> = d.options.iter().map(|o| o.label.as_str()).collect();
            let head = d
                .header
                .as_deref()
                .map(|h| format!("{h}: "))
                .unwrap_or_default();
            let opts = if options.is_empty() {
                String::new()
            } else {
                format!(" ({})", options.join(" / "))
            };
            Outcome::Dialog {
                text: format!("{head}{}{opts}", d.question.trim()),
            }
        }
    }
}

/// What [`write_round`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Written {
    /// In the tip; the body as left.
    Yes(String),
    /// Not written for a reason that may pass (read-only, locked, kept
    /// changing): a final outcome is kept and retried at the next scan.
    Kept,
    /// Not written and never will be (gone, not ours, not an agent tip).
    Dropped,
}

impl Written {
    pub fn body(self) -> Option<String> {
        match self {
            Written::Yes(b) => Some(b),
            Written::Kept | Written::Dropped => None,
        }
    }
}

/// The reply and the deliverables to `<tips>/agent/<id>/` (protocol 7.3).
fn copy_results(inbox: &Inbox, round: &Round) {
    let id = &round.tip_id;
    match results_dir(&round.folder, id) {
        Ok(dest) => {
            let result_md = round
                .graph_run
                .as_deref()
                .map(|r| node_run_dir(&round.graph_dir, r, NODE_ID).join(RESULT_FILE));
            let report = copy_round(
                &dest,
                result_md.as_deref(),
                &deliverables_dir(&round.graph_dir, NODE_ID),
            );
            for r in &report.refused {
                inbox.log(Some(id), format!("not copied: {r}"));
            }
        }
        Err(e) => inbox.log(Some(id), format!("results folder refused: {e}")),
    }
}

/// Copy the round's files (unless the claim is lost), then append the block and set `agent` (protocol
/// 7.2 and 7.3). A final outcome (done / failed) that could not be written
/// for a passing reason is kept for the next scan.
pub(crate) fn write_round(inbox: &Inbox, round: &Round, outcome: &Outcome) -> Written {
    let env = inbox.env();
    let id = &round.tip_id;
    let who = Claimant {
        host: &env.host,
        run: &round.run,
        at: (env.clock)(),
    };
    // A read-only look at the frontmatter first: when the claim is no longer
    // this host's run (or the tip is gone or no longer an agent tip), the
    // results folder belongs to whoever holds it now and is not touched.
    match claim_holds(&round.path, id, &who) {
        Err(d) if d.is_lost() => {
            inbox.log(Some(id), format!("results not copied: {}", d.describe()));
        }
        _ => copy_results(inbox, round),
    }
    let state = outcome.state().as_str();
    let written = finish_file(&round.path, id, &who, &round.expected_body, outcome);
    inbox.refresh_tip(id, &round.path);
    match written {
        Ok(f) => {
            let note = if f.edited_meanwhile {
                format!("{state} written; the tip was edited meanwhile, it runs again")
            } else {
                format!("{state} written")
            };
            inbox.log(Some(id), note.clone());
            inbox.set_note(id, note);
            inbox.lock().unwritten.remove(id);
            Written::Yes(f.body)
        }
        Err(d) => {
            let keep = d.is_retryable() && outcome.state() != super::tip::AgentState::Question;
            let note = if keep {
                format!(
                    "{state} not written yet: {}; retried at the next scan",
                    d.describe()
                )
            } else {
                format!("{state} not written: {}", d.describe())
            };
            inbox.log(Some(id), note.clone());
            inbox.set_note(id, note);
            if keep {
                inbox.keep_unwritten(round, outcome, &d.describe());
                Written::Kept
            } else {
                inbox.lock().unwritten.remove(id);
                Written::Dropped
            }
        }
    }
}

fn halted(inbox: &Inbox, superseded: &AtomicBool) -> bool {
    superseded.load(Ordering::SeqCst) || inbox.is_stopping()
}

/// Write `outcome` unless halted; `None` when nothing was written.
async fn write(
    inbox: &Inbox,
    round: &Round,
    outcome: Outcome,
    flag: &AtomicBool,
) -> Option<String> {
    if halted(inbox, flag) {
        return None;
    }
    let ib = inbox.clone();
    let r = round.clone();
    match blocking(move || Ok(write_round(&ib, &r, &outcome))).await {
        Ok(w) => w.body(),
        Err(e) => {
            inbox.log(
                Some(&round.tip_id),
                format!("write-back task failed: {}", e.message),
            );
            None
        }
    }
}

fn node_outcome(round: &Round, state: &RunState) -> Option<Outcome> {
    let node = state.nodes.get(NODE_ID)?;
    match node.status {
        NodeStatus::Succeeded => Some(Outcome::Done {
            reply: reply_of(round),
        }),
        s if s.is_terminal() => Some(Outcome::Failed {
            reason: format!(
                "the session ended {}{}",
                s.as_str(),
                node.reason
                    .as_deref()
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            ),
        }),
        _ => None,
    }
}

/// Follow `l` until its outcome is written (or the tip is superseded).
pub(crate) async fn watch(inbox: Inbox, l: Launched, superseded: Arc<AtomicBool>) {
    let Launched {
        mut round,
        join,
        tap,
    } = l;
    let timeout = inbox.config().timeout;
    let poll = inbox.env().poll;
    let id = round.tip_id.clone();
    let run = round.graph_run.clone().unwrap_or_default();
    let mut deadline = Instant::now() + timeout;
    let mut written_prompt: Option<String> = None;
    let mut waiting_since: Option<Instant> = None;
    let mut asking = false;
    let mut join = Some(join);
    loop {
        tokio::time::sleep(poll).await;
        if halted(&inbox, &superseded) {
            return;
        }
        let finished_now = join.as_ref().is_some_and(|j| j.is_finished());
        let final_state = match join.take_if(|_| finished_now) {
            Some(j) => match j.await {
                Ok(s) => Some(s),
                Err(e) => {
                    inbox.log(Some(&id), format!("the run task ended abnormally: {e}"));
                    None
                }
            },
            None => None,
        };
        let ended = join.is_none();
        let state = final_state.or_else(|| tap.latest());
        if let Some(s) = &state {
            inbox.set_sid(&id, s);
        }
        let status = state
            .as_ref()
            .and_then(|s| s.nodes.get(NODE_ID))
            .map(|n| n.status);
        if let Some(outcome) = state.as_ref().and_then(|s| node_outcome(&round, s)) {
            write(&inbox, &round, outcome, &superseded).await;
            inbox.set_phase(&id, InboxPhase::Finished);
            inbox.pump().await;
            return;
        }
        if status == Some(NodeStatus::Waiting) {
            let since = *waiting_since.get_or_insert_with(Instant::now);
            let prompt = list_session_prompts(inbox.env().app.terminals(), &run)
                .ok()
                .and_then(|v| v.into_iter().find(|p| p.node_id == NODE_ID));
            let (key, outcome) = match prompt {
                Some(p) => (p.prompt.id().to_string(), question_text(&p)),
                None if since.elapsed() >= PROMPT_GRACE => (
                    "unlisted".to_string(),
                    Outcome::Dialog {
                        text: "the session waits for input".into(),
                    },
                ),
                None => continue,
            };
            if written_prompt.as_deref() != Some(key.as_str()) {
                if let Some(body) = write(&inbox, &round, outcome, &superseded).await {
                    round.expected_body = body;
                }
                written_prompt = Some(key);
                asking = true;
                inbox.set_phase(&id, InboxPhase::Question);
                inbox.pump().await;
            }
            continue;
        }
        waiting_since = None;
        if asking {
            // Answered in the window: working again, with a fresh timeout.
            asking = false;
            written_prompt = None;
            deadline = Instant::now() + timeout;
            inbox.set_phase(&id, InboxPhase::Working);
        }
        if Instant::now() >= deadline {
            if let Err(e) = inbox.env().app.cancel_run(&run) {
                inbox.log(Some(&id), format!("cancel after timeout: {}", e.message));
            }
            let minutes = timeout.as_secs() / 60;
            let reason = if minutes > 0 {
                format!("timed out after {minutes} minutes; the run was cancelled")
            } else {
                format!(
                    "timed out after {} s; the run was cancelled",
                    timeout.as_secs()
                )
            };
            write(&inbox, &round, Outcome::Failed { reason }, &superseded).await;
            inbox.set_phase(&id, InboxPhase::Finished);
            inbox.pump().await;
            return;
        }
        if ended {
            let reason = "the run ended without a result for the node".to_string();
            write(&inbox, &round, Outcome::Failed { reason }, &superseded).await;
            inbox.set_phase(&id, InboxPhase::Finished);
            inbox.pump().await;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{DialogKind, DialogPrompt, PromptOption, PromptSource, ReplyQuestion};

    #[test]
    fn the_echoed_pointer_line_is_dropped() {
        let r =
            "> Node \"Board task\" (task): read the brief at C:/x/brief.md and do it\n\nFound 3.\n";
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
}
