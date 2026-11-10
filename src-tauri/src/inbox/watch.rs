//! Following one started run until its outcome is written to the tip.
//!
//! The node's status (from the run's own `run:state` events) decides:
//! succeeded -> `done`; failed / cancelled / skipped / blocked -> `failed`;
//! waiting -> `question` (a reply question, or a dialog in the window) while
//! the session stays open; queued with a Claude Code start-up dialog on
//! screen ([`super::startup`]) -> `question` too, once per dialog kind; if
//! the owner then answers in the window and the node finishes, that outcome
//! is written as one more block. Working time over the timeout cancels the
//! run (`failed`). Nothing is written once the tip was superseded by a newer
//! run or the app is closing. A `done` that was written closes the round's
//! session and window ([`super::close`]).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use crate::contract::{InboxPhase, NodeStatus, RunState, SessionPrompt, TurnPrompt};
use crate::run_loop::blocking;
use crate::run_store::{node_run_dir, read_node_result, RESULT_FILE};
use crate::store::deliverables_dir;
use crate::terminal::api::Terminals;
use crate::terminal::prompt_api::list_session_prompts;

use super::block::Outcome;
use super::env::Tap;
use super::graphgen::NODE_ID;
use super::ops::Claimant;
use super::results::{copy_round, results_dir};
use super::runner::Inbox;
use super::start::Launched;
use super::startup::{startup_dialog, StartupDialog};
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
    /// The node prompt the round ran with (set once the run started),
    /// kept as `prompt.md` with the results.
    pub prompt: Option<String>,
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
                round.prompt.as_deref(),
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

/// Prefix of the key of a start-up dialog (prompt ids never start so).
const STARTUP_KEY: &str = "startup: ";

/// How long a dialog may stay "still being drawn" (output arriving) before
/// it stops holding off the timeout.
const DRAWING_MAX: Duration = Duration::from_secs(30);

/// Whether the session waits for the owner now.
enum Ask {
    /// It waits: the key of what it waits on (written once) and the outcome.
    Now(String, Outcome),
    /// Not decided yet (no prompt listed yet, a dialog still being drawn).
    Later,
    /// It does not wait.
    No,
}

/// The watcher's state for one round.
struct Follow {
    timeout: Duration,
    deadline: Instant,
    /// Key of the prompt last written (cleared once answered).
    written_prompt: Option<String>,
    /// Start-up dialogs reported in this round: each kind once.
    startup_written: HashSet<String>,
    waiting_since: Option<Instant>,
    drawing_since: Option<Instant>,
    /// The tip shows a question for what the session waits on now.
    asking: bool,
}

impl Follow {
    fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            deadline: Instant::now() + timeout,
            written_prompt: None,
            startup_written: HashSet::new(),
            waiting_since: None,
            drawing_since: None,
            asking: false,
        }
    }

    /// Not written yet in this round (a start-up dialog kind: ever; a turn
    /// prompt: since the last answer).
    fn fresh(&self, key: &str) -> bool {
        self.written_prompt.as_deref() != Some(key) && !self.startup_written.contains(key)
    }

    fn written(&mut self, key: String) {
        if key.starts_with(STARTUP_KEY) {
            self.startup_written.insert(key.clone());
        }
        self.written_prompt = Some(key);
    }

    /// Answered in the window: working again, with a fresh timeout.
    fn answered(&mut self) {
        self.asking = false;
        self.written_prompt = None;
        self.deadline = Instant::now() + self.timeout;
    }
}

/// A start-up dialog per [`startup_dialog`]; one still being drawn holds
/// off the timeout for at most [`DRAWING_MAX`].
fn ask_startup(terminals: &Terminals, run: &str, f: &mut Follow) -> Ask {
    let seen = startup_dialog(terminals, run);
    if seen != StartupDialog::Drawing {
        f.drawing_since = None;
    }
    match seen {
        StartupDialog::Open(text) => Ask::Now(
            format!("{STARTUP_KEY}{text}"),
            Outcome::Dialog { text: text.into() },
        ),
        StartupDialog::Drawing => {
            let since = *f.drawing_since.get_or_insert_with(Instant::now);
            if since.elapsed() < DRAWING_MAX {
                Ask::Later
            } else {
                Ask::No
            }
        }
        StartupDialog::Closed => Ask::No,
    }
}

/// What the node waits on: a turn prompt while it is `waiting`, or a
/// Claude Code start-up dialog (trust, MCP) while its message was not taken
/// yet (`pending` / `queued`). Nothing is typed into either.
fn ask(inbox: &Inbox, run: &str, status: Option<NodeStatus>, f: &mut Follow) -> Ask {
    let terminals = inbox.env().app.terminals();
    if status != Some(NodeStatus::Waiting) {
        f.waiting_since = None;
    }
    match status {
        Some(NodeStatus::Waiting) => {
            let since = *f.waiting_since.get_or_insert_with(Instant::now);
            let prompt = list_session_prompts(terminals, run)
                .ok()
                .and_then(|v| v.into_iter().find(|p| p.node_id == NODE_ID));
            match prompt {
                Some(p) => Ask::Now(p.prompt.id().to_string(), question_text(&p)),
                None if since.elapsed() >= PROMPT_GRACE => Ask::Now(
                    "unlisted".to_string(),
                    Outcome::Dialog {
                        text: "the session waits for input".into(),
                    },
                ),
                None => Ask::Later,
            }
        }
        None | Some(NodeStatus::Pending | NodeStatus::Queued) => ask_startup(terminals, run, f),
        Some(_) => Ask::No,
    }
}

/// The round ended with `outcome`: written, the phase set, and a written
/// `done` closes its session in the background (so the freed slot is used
/// at once).
async fn finish(inbox: &Inbox, round: &Round, outcome: Outcome, flag: &AtomicBool, run: &str) {
    let done = matches!(outcome, Outcome::Done { .. });
    let written = write(inbox, round, outcome, flag).await;
    inbox.set_phase(&round.tip_id, InboxPhase::Finished);
    if done && written.is_some() {
        // Outcome and results are in the tip folder: close it now.
        let (ib, id, run) = (inbox.clone(), round.tip_id.clone(), run.to_string());
        tokio::spawn(async move { ib.close_done(&id, &run).await });
    }
    inbox.pump().await;
}

fn timeout_reason(timeout: Duration) -> String {
    let minutes = timeout.as_secs() / 60;
    if minutes > 0 {
        format!("timed out after {minutes} minutes; the run was cancelled")
    } else {
        format!(
            "timed out after {} s; the run was cancelled",
            timeout.as_secs()
        )
    }
}

/// The run's latest state, and whether its task has ended.
async fn latest(
    inbox: &Inbox,
    id: &str,
    join: &mut Option<JoinHandle<RunState>>,
    tap: &Tap,
) -> (Option<RunState>, bool) {
    let finished_now = join.as_ref().is_some_and(|j| j.is_finished());
    let final_state = match join.take_if(|_| finished_now) {
        Some(j) => match j.await {
            Ok(s) => Some(s),
            Err(e) => {
                inbox.log(Some(id), format!("the run task ended abnormally: {e}"));
                None
            }
        },
        None => None,
    };
    (final_state.or_else(|| tap.latest()), join.is_none())
}

/// Follow `l` until its outcome is written (or the tip is superseded).
pub(crate) async fn watch(inbox: Inbox, l: Launched, superseded: Arc<AtomicBool>) {
    let Launched {
        mut round,
        join,
        tap,
    } = l;
    let poll = inbox.env().poll;
    let id = round.tip_id.clone();
    let run = round.graph_run.clone().unwrap_or_default();
    let mut f = Follow::new(inbox.config().timeout);
    let mut join = Some(join);
    loop {
        tokio::time::sleep(poll).await;
        if halted(&inbox, &superseded) {
            return;
        }
        let (state, ended) = latest(&inbox, &id, &mut join, &tap).await;
        if let Some(s) = &state {
            inbox.set_sid(&id, s);
        }
        let status = state
            .as_ref()
            .and_then(|s| s.nodes.get(NODE_ID))
            .map(|n| n.status);
        if let Some(outcome) = state.as_ref().and_then(|s| node_outcome(&round, s)) {
            return finish(&inbox, &round, outcome, &superseded, &run).await;
        }
        // Before anything that waits: a run that ended never comes back.
        if ended {
            let reason = "the run ended without a result for the node".to_string();
            return finish(
                &inbox,
                &round,
                Outcome::Failed { reason },
                &superseded,
                &run,
            )
            .await;
        }
        match ask(&inbox, &run, status, &mut f) {
            Ask::Now(key, outcome) => {
                if f.fresh(&key) {
                    if let Some(body) = write(&inbox, &round, outcome, &superseded).await {
                        round.expected_body = body;
                    }
                    f.written(key);
                    f.asking = false;
                }
                if !f.asking {
                    // Written now, or a kind already reported this round:
                    // either way the session waits for the owner.
                    f.asking = true;
                    inbox.set_phase(&id, InboxPhase::Question);
                    inbox.pump().await;
                }
                continue;
            }
            Ask::Later => continue,
            Ask::No => {}
        }
        if f.asking {
            f.answered();
            inbox.set_phase(&id, InboxPhase::Working);
        }
        if Instant::now() >= f.deadline {
            if let Err(e) = inbox.env().app.cancel_run(&run) {
                inbox.log(Some(&id), format!("cancel after timeout: {}", e.message));
            }
            let reason = timeout_reason(f.timeout);
            return finish(
                &inbox,
                &round,
                Outcome::Failed { reason },
                &superseded,
                &run,
            )
            .await;
        }
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
