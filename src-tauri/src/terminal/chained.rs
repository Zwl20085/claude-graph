//! Chained-headless sessions (harness kind `deepseek-harness`, see
//! [`crate::harness::dsh`]): nothing is typed into a terminal. Each message
//! (a node's message, an answer, a follow-up) is one `dsh --json` process
//! with the text on stdin; the first process's `session` event gives the
//! session id that every later process adopts (`--session-id`). The session
//! window shows a rendering of the events and a "▶ node <title>" line per
//! node; the turn ends when the process exits. The final text goes into
//! reply.md and the question classifier, so a `question` outcome pauses the
//! session exactly as in a pty session (answer / continue). Dialogs cannot
//! occur: an approval headless dsh cannot ask for is reported as a
//! `permission_denied` node event.
//!
//! Messages for the session (typed in its window, or Messages-tab
//! follow-ups) that arrive while a node runs are follow-ups, never answers:
//! they are delivered, each as its own process, after the node ends.
//! Output is read as lossy UTF-8 in lines of at most [`LINE_MAX`] bytes;
//! once dsh exits, what is left of its process tree is killed and its pipes
//! get [`PIPE_DRAIN`] to close (a background process holding them cannot
//! hang the turn).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

use crate::contract::{
    AnswerVia, Node, NodeEventKind, NodeStatus, PromptAnswer, QuestionAnswer, QuestionRecord,
    ReplyQuestion, SessionStatus, TurnPrompt,
};
use crate::harness::adapter::Launch;
use crate::harness::dsh::{
    approval_denied, parse_line, render, tool_summary, DshEvent, MODE_LABEL,
};
use crate::job::Job;
use crate::prompt::resolve_working_dir;
use crate::run_store::{node_run_dir, EVENTS_FILE, RESULT_FILE};
use crate::store::write_atomic;
use crate::terminal::api::PromptAction;
use crate::terminal::node::{evaluate, prepare};
use crate::terminal::outcome::{classify, excerpt, OutcomeKind};
use crate::terminal::prompts::Pending;
use crate::terminal::reply::REPLY_FILE;
use crate::terminal::session::{End, NodeEnd, Session, CANCELLED};
use crate::terminal::startup::session_env;
use crate::terminal::turn::{compose, Exchange};
use crate::terminal::view::View;
use crate::terminal::{session_dir, window_label};

/// dsh's stderr of a session (diagnostics, `dsh: reasoning:` lines).
pub const DSH_STDERR: &str = "dsh-stderr.log";
/// Last stderr lines kept for a failure reason.
const STDERR_TAIL: usize = 3;
/// Longest output line kept; the rest of a longer line is dropped.
pub const LINE_MAX: usize = 4 * 1024 * 1024;
/// After dsh exits: how long its pipes may stay open before the turn ends.
const PIPE_DRAIN: Duration = Duration::from_secs(2);

/// Lines of a byte stream: lossy UTF-8, cut at `max` bytes, without the
/// line end. Cancel-safe: a partial line stays buffered.
pub struct LineReader<R> {
    inner: BufReader<R>,
    buf: Vec<u8>,
    cut: bool,
    max: usize,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(inner: R, max: usize) -> Self {
        Self {
            inner: BufReader::new(inner),
            buf: Vec::new(),
            cut: false,
            max,
        }
    }

    /// The next line; `None` at the end of the stream.
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            let avail = self.inner.fill_buf().await?;
            if avail.is_empty() {
                if self.buf.is_empty() && !self.cut {
                    return Ok(None);
                }
                return Ok(Some(self.take()));
            }
            let newline = avail.iter().position(|b| *b == b'\n');
            let chunk = &avail[..newline.unwrap_or(avail.len())];
            let room = self.max.saturating_sub(self.buf.len());
            if chunk.len() > room {
                self.cut = true;
            }
            self.buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
            let used = newline.map_or(avail.len(), |i| i + 1);
            self.inner.consume(used);
            if newline.is_some() {
                return Ok(Some(self.take()));
            }
        }
    }

    fn take(&mut self) -> String {
        if self.cut {
            log::warn!("an output line over {} bytes was cut", self.max);
        }
        let mut line = String::from_utf8_lossy(&self.buf).into_owned();
        if line.ends_with('\r') {
            line.pop();
        }
        self.buf.clear();
        self.cut = false;
        line
    }
}

/// The next line of `r`, or never when there is no reader.
async fn next_or_never<R: AsyncRead + Unpin>(
    r: Option<&mut LineReader<R>>,
) -> std::io::Result<Option<String>> {
    match r {
        Some(r) => r.next_line().await,
        None => std::future::pending().await,
    }
}

/// What a chained session runs, set at its start.
pub(super) struct Chain {
    launch: Launch,
    cwd: PathBuf,
    /// The CLI's session id, from the first process's `session` event.
    session_id: Option<String>,
    view: Arc<View>,
}

/// How one process ended.
struct Ran {
    code: i32,
    final_text: Option<String>,
    error: Option<String>,
}

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[90m";
const RESET: &str = "\x1b[0m";

impl Session {
    /// The start of a chained session: folder, window, program; no process.
    pub(super) fn start_chained(&mut self) -> Result<(), End> {
        let ctx = self.t.ctx.clone();
        let (run_id, sid) = (ctx.run_id.clone(), self.t.plan.sid.clone());
        let dir = session_dir(&ctx.graph_dir, &run_id, &sid);
        std::fs::create_dir_all(&dir)
            .map_err(|e| End::Failed(format!("io: {}: {e}", dir.display())))?;
        self.report(|i| i.status = SessionStatus::Starting);
        let first = self
            .t
            .plan
            .to_send()
            .first()
            .and_then(|id| ctx.node(id).ok());
        let cwd = first.map_or(ctx.graph_dir.clone(), |n| {
            resolve_working_dir(&ctx.graph_dir, n)
        });
        if !cwd.is_dir() {
            return Err(End::Failed(format!(
                "working directory {} does not exist",
                cwd.display()
            )));
        }
        self.prepare_hooks_dir()?;
        let launch = self.launch_spec(&cwd)?;
        let view = Arc::new(View::new(
            ctx.emitter.clone(),
            &window_label(&run_id, &sid),
            &run_id,
            &sid,
        ));
        self.t.shared.set_view(view.clone());
        if let Err(e) = ctx.emitter.open_window(&self.t.shared.window()) {
            log::warn!("opening the window of session {sid} failed: {e}");
        }
        view.show(&format!(
            "{BOLD}{} \u{2014} {MODE_LABEL}{RESET}\r\n{DIM}{} in {}{RESET}\r\n",
            self.behavior.label,
            launch.display,
            cwd.display()
        ));
        log::info!(
            "session {sid} ({}): {} chained",
            self.t.plan.harness,
            launch.display
        );
        self.chain = Some(Chain {
            launch,
            cwd,
            session_id: None,
            view,
        });
        // No process lives between messages: no pid, but the session takes
        // messages while its task runs.
        self.report(|i| {
            i.pid = None;
            i.alive = true;
        });
        Ok(())
    }

    fn show(&self, text: &str) {
        if let Some(c) = &self.chain {
            c.view.show(text);
        }
    }

    /// One node of a chained session (see the module doc), then the
    /// follow-ups that arrived meanwhile.
    pub(super) async fn run_chained_node(
        &mut self,
        id: &str,
        started_at: DateTime<Utc>,
    ) -> NodeEnd {
        let (status, reason, end) = self.chained_node_turns(id, started_at).await;
        if end.is_some() {
            return (status, reason, end);
        }
        self.defer_queued(id);
        match self.deliver_deferred().await {
            Ok(()) => (status, reason, None),
            Err(end) => (status, reason, Some(end)),
        }
    }

    async fn chained_node_turns(&mut self, id: &str, started_at: DateTime<Utc>) -> NodeEnd {
        self.report(|i| i.current_node = Some(id.to_string()));
        let ctx = self.t.ctx.clone();
        let Ok(node) = ctx.node(id) else {
            return (NodeStatus::Failed, Some(format!("no node {id}")), None);
        };
        let typed = match prepare(&ctx, node, &self.done, &self.t.plan.node_ids) {
            Ok(t) => t,
            Err(e) => {
                let reason = format!("could not prepare the node: {}", e.message);
                return (NodeStatus::Failed, Some(reason), None);
            }
        };
        self.node_msg(id, NodeStatus::Running, None, true);
        self.show(&format!(
            "\r\n{BOLD}\u{25b6} node {}{RESET}\r\n",
            node.title
        ));
        let started = SystemTime::from(started_at);
        let mut convo: Vec<Exchange> = Vec::new();
        let mut text = typed.text().to_string();
        loop {
            convo.push(Exchange {
                user: text.clone(),
                reply: None,
            });
            let ran = match self.dsh_turn(id, &text).await {
                Ok(r) => r,
                Err(end) => return ended(end),
            };
            if let Some(last) = convo.last_mut() {
                last.reply = ran.final_text.clone();
            }
            self.write_chained_reply(id, &convo);
            if ran.code != 0 {
                let why = ran.error.unwrap_or_else(|| "see the session window".into());
                return (
                    NodeStatus::Failed,
                    Some(format!("dsh exited with code {}: {why}", ran.code)),
                    None,
                );
            }
            let body = ran.final_text.unwrap_or_default();
            let outcome = classify(&body);
            let (status, reason) = evaluate(&ctx, node, started);
            if outcome.kind == OutcomeKind::Done {
                return (status, reason, None);
            }
            let question = ReplyQuestion {
                id: self.next_prompt_id(id),
                question: outcome.question.unwrap_or_default(),
                choices: outcome.choices,
                reply_excerpt: excerpt(&body),
            };
            if !node.deliverables.is_empty() && status == NodeStatus::Succeeded {
                self.record_offer(id, question);
                return (status, reason, None);
            }
            match self.wait_answer(node, Pending::Reply(question)).await {
                Ok(Some(answer)) => text = answer,
                Ok(None) => {
                    let (status, reason) = evaluate(&ctx, node, started);
                    return (status, reason, None);
                }
                Err(end) => return ended(end),
            }
        }
    }

    fn record_offer(&mut self, id: &str, question: ReplyQuestion) {
        let prompt = TurnPrompt::Reply(question);
        self.record(
            id,
            QuestionRecord {
                offer: true,
                ..QuestionRecord::of(&prompt, Utc::now())
            },
        );
        self.stats.questions += 1;
        self.event(
            NodeEventKind::Question,
            format!(
                "{} (deliverables written: the node is done; reply in Messages)",
                prompt.question()
            ),
        );
    }

    /// Messages already queued (sent while `node_id` ran, before any
    /// question): follow-ups are kept for after the node, stale answers are
    /// dropped with a node event.
    fn defer_queued(&mut self, node_id: &str) {
        let Some(rx) = self.actions.as_mut() else {
            return;
        };
        let mut stale = Vec::new();
        while let Ok(action) = rx.try_recv() {
            match action {
                PromptAction::Followup { node_id, text } => self.deferred.push((node_id, text)),
                other => stale.push(other),
            }
        }
        for other in stale {
            self.event_on(node_id, NodeEventKind::Stderr, dropped(&other));
        }
    }

    /// Deliver the follow-ups kept by [`Self::defer_queued`], in order.
    async fn deliver_deferred(&mut self) -> Result<(), End> {
        while !self.deferred.is_empty() {
            let (node_id, text) = self.deferred.remove(0);
            self.followup_turn(&node_id, &text).await?;
        }
        Ok(())
    }

    /// The node waits on `pending`: `Some(text)` to send as the answer,
    /// `None` for "continue".
    async fn wait_answer(&mut self, node: &Node, pending: Pending) -> Result<Option<String>, End> {
        // Messages sent while the node ran are follow-ups, not this answer.
        self.defer_queued(&node.id);
        let Pending::Reply(q) = &pending else {
            // dsh draws no dialogs; nothing waits on one.
            return Ok(None);
        };
        self.open_prompt(&node.id, &pending);
        loop {
            let action = self.next_action().await?;
            let (text, index, via) = match action {
                PromptAction::Continue { node_id } if node_id == node.id => {
                    (None, None, AnswerVia::Continue)
                }
                PromptAction::Answer { prompt_id, answer } if prompt_id == q.id => match answer {
                    PromptAnswer::Text(t) => (Some(t), None, AnswerVia::App),
                    PromptAnswer::Choice(i) => match q.choices.get(i) {
                        Some(c) => (Some(c.label.clone()), Some(i), AnswerVia::App),
                        None => {
                            let why = format!("no option {i}: answer not sent");
                            self.event_on(&node.id, NodeEventKind::Stderr, why);
                            continue;
                        }
                    },
                    PromptAnswer::Choices(_) => {
                        let why = "this question takes one answer, not several".to_string();
                        self.event_on(&node.id, NodeEventKind::Stderr, why);
                        continue;
                    }
                },
                PromptAction::Followup { node_id, text } => {
                    // Sent before the question showed: a follow-up.
                    self.deferred.push((node_id, text));
                    continue;
                }
                other => {
                    self.event_on(&node.id, NodeEventKind::Stderr, dropped(&other));
                    continue;
                }
            };
            let answer = QuestionAnswer {
                ts: Utc::now(),
                via,
                text: text.clone(),
                index,
            };
            let running = text.is_some();
            self.clear_prompt(&node.id, &pending, Some(answer), via, running);
            return Ok(text);
        }
    }

    /// The next action from the app (kill: `End::Cancelled`).
    async fn next_action(&mut self) -> Result<PromptAction, End> {
        let Some(rx) = self.actions.as_mut() else {
            return Err(End::Failed("the session lost its message channel".into()));
        };
        let mut kill = self.t.shared.kill_watch();
        tokio::select! {
            a = rx.recv() => a.ok_or(End::Cancelled),
            _ = async { kill.wait_for(|k| *k).await.map(|_| ()) } => Err(End::Cancelled),
        }
    }

    /// After the node list: follow-up messages until the session is killed.
    pub(super) async fn serve_followups(&mut self) {
        if self.deliver_deferred().await.is_err() {
            return;
        }
        loop {
            let Ok(action) = self.next_action().await else {
                return;
            };
            let node_id = self.t.plan.node_ids.last().cloned().unwrap_or_default();
            let (node_id, text) = match action {
                PromptAction::Followup { node_id, text } => (node_id, text),
                other => {
                    self.event_on(&node_id, NodeEventKind::Stderr, dropped(&other));
                    continue;
                }
            };
            if self.followup_turn(&node_id, &text).await.is_err() {
                return;
            }
        }
    }

    /// One follow-up message to `node_id` (its own dsh process).
    async fn followup_turn(&mut self, node_id: &str, text: &str) -> Result<(), End> {
        self.show(&format!(
            "\r\n{BOLD}\u{25b6} message to {node_id}{RESET}\r\n"
        ));
        self.event_on(node_id, NodeEventKind::UserMessage, text.to_string());
        let r = self.dsh_turn(node_id, text).await?;
        if r.code != 0 {
            self.event_on(
                node_id,
                NodeEventKind::Stderr,
                format!(
                    "dsh exited with code {}: {}",
                    r.code,
                    r.error.unwrap_or_default()
                ),
            );
        }
        Ok(())
    }

    fn event_on(&self, node_id: &str, kind: NodeEventKind, text: String) {
        self.t.ctx.emitter.node_event(&crate::contract::NodeEvent {
            run_id: self.t.ctx.run_id.clone(),
            node_id: node_id.to_string(),
            kind,
            text,
            ts: Utc::now(),
        });
    }

    fn write_chained_reply(&self, id: &str, convo: &[Exchange]) {
        let ctx = &self.t.ctx;
        let dir = node_run_dir(&ctx.graph_dir, &ctx.run_id, id);
        let text = compose(convo);
        for f in [REPLY_FILE, RESULT_FILE] {
            if let Err(e) = write_atomic(&dir.join(f), text.as_bytes()) {
                log::warn!("writing {f} of {id} failed: {e}");
            }
        }
    }

    /// One dsh process for `text` (see the module doc).
    async fn dsh_turn(&mut self, node_id: &str, text: &str) -> Result<Ran, End> {
        let Some(chain) = self.chain.as_ref() else {
            return Err(End::Failed("the dsh session did not start".into()));
        };
        let ctx = self.t.ctx.clone();
        let (run_id, sid) = (ctx.run_id.clone(), self.t.plan.sid.clone());
        let mut cmd = crate::claude_bin::ClaudeCommand {
            program: chain.launch.program.clone(),
            prefix_args: Vec::new(),
            display: chain.launch.display.clone(),
        }
        .command();
        cmd.args(&chain.launch.args);
        if let Some(id) = &chain.session_id {
            cmd.args(["--session-id", id]);
        }
        cmd.arg("-")
            .current_dir(&chain.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in session_env(&run_id, &sid, &self.hooks_dir(), chain.launch.env.clone()) {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        let quoted: String = text
            .lines()
            .map(|l| format!("{DIM}> {l}{RESET}\r\n"))
            .collect();
        chain.view.show(&quoted);
        let mut child = cmd
            .spawn()
            .map_err(|e| End::Failed(format!("could not start {}: {e}", chain.launch.display)))?;
        let mut job = Job::assign(&child);
        let pid = child.id();
        if let Some(mut stdin) = child.stdin.take() {
            let input = text.to_string();
            tokio::spawn(async move {
                if let Err(e) = stdin.write_all(input.as_bytes()).await {
                    log::warn!("writing the message to dsh failed: {e}");
                }
                let _ = stdin.shutdown().await;
            });
        }
        let stderr_task = child.stderr.take().map(|err| {
            let path = session_dir(&ctx.graph_dir, &run_id, &sid).join(DSH_STDERR);
            tokio::spawn(read_stderr(err, path))
        });
        let events_path = node_run_dir(&ctx.graph_dir, &run_id, node_id).join(EVENTS_FILE);
        let mut ran = Ran {
            code: 0,
            final_text: None,
            error: None,
        };
        let mut tools: HashMap<String, String> = HashMap::new();
        let mut out = child.stdout.take().map(|o| LineReader::new(o, LINE_MAX));
        let mut kill = self.t.shared.kill_watch();
        let mut exited: Option<std::io::Result<std::process::ExitStatus>> = None;
        let mut drain_until: Option<tokio::time::Instant> = None;
        loop {
            let deadline = drain_until;
            tokio::select! {
                l = next_or_never(out.as_mut()) => match l {
                    Ok(Some(line)) => {
                        append_line(&events_path, &line);
                        if let Some(ev) = parse_line(&line) {
                            self.on_dsh_event(node_id, &ev, &mut tools, &mut ran);
                        }
                    }
                    Ok(None) => {
                        out = None;
                        if exited.is_some() {
                            break;
                        }
                    }
                    Err(e) => {
                        log::warn!("reading dsh output of {node_id} failed: {e}");
                        self.event_on(node_id, NodeEventKind::Stderr, format!("reading dsh output failed: {e}"));
                        out = None;
                        if exited.is_some() {
                            break;
                        }
                    }
                },
                s = child.wait(), if exited.is_none() => {
                    exited = Some(s);
                    // Kill what is left of the tree: a background process
                    // that inherited the pipes must not hold the turn open.
                    drop(job.take());
                    if let Some(pid) = pid {
                        crate::runner::kill_tree(pid).await;
                    }
                    drain_until = Some(tokio::time::Instant::now() + PIPE_DRAIN);
                    if out.is_none() {
                        break;
                    }
                }
                _ = tokio::time::sleep_until(deadline.unwrap_or_else(tokio::time::Instant::now)), if deadline.is_some() => {
                    log::warn!("dsh output of {node_id} still open {PIPE_DRAIN:?} after it exited");
                    break;
                }
                _ = async { kill.wait_for(|k| *k).await.map(|_| ()) } => {
                    if let Some(pid) = pid {
                        crate::runner::kill_tree(pid).await;
                    }
                    let _ = child.kill().await;
                    return Err(End::Cancelled);
                }
            }
        }
        drop(job);
        let tail = match stderr_task {
            Some(mut task) => match tokio::time::timeout(PIPE_DRAIN, &mut task).await {
                Ok(t) => t.unwrap_or_default(),
                Err(_) => {
                    task.abort();
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        ran.code = exited
            .and_then(Result::ok)
            .and_then(|s| s.code())
            .unwrap_or(-1);
        if ran.code != 0 && ran.error.is_none() && !tail.is_empty() {
            ran.error = Some(tail.join(" / "));
        }
        Ok(ran)
    }

    fn on_dsh_event(
        &mut self,
        node_id: &str,
        ev: &DshEvent,
        tools: &mut HashMap<String, String>,
        ran: &mut Ran,
    ) {
        let shown = render(ev, |id| tools.get(id).cloned());
        match ev {
            DshEvent::Session { id } => {
                let Some(chain) = self.chain.as_mut() else {
                    return;
                };
                if chain.session_id.as_deref() == Some(id.as_str()) {
                    return; // the same session, adopted again: not shown again
                }
                if chain.session_id.is_none() && !id.is_empty() {
                    chain.session_id = Some(id.clone());
                }
            }
            DshEvent::Text(t) if !t.trim().is_empty() => {
                self.event_on(node_id, NodeEventKind::Text, t.clone());
            }
            DshEvent::ToolCall { id, tool, input } => {
                tools.insert(id.clone(), tool.clone());
                self.event_on(
                    node_id,
                    NodeEventKind::ToolUse,
                    format!("{tool} {}", tool_summary(tool, input)),
                );
            }
            DshEvent::ToolResult { id, ok, result } => {
                if approval_denied(*ok, result) {
                    let tool = tools.get(id).cloned().unwrap_or_default();
                    self.event_on(
                        node_id,
                        NodeEventKind::PermissionDenied,
                        format!("{tool}: needs approval; headless dsh has no approval channel"),
                    );
                }
            }
            DshEvent::Final(t) => ran.final_text = Some(t.clone()),
            DshEvent::Error(m) => {
                ran.error = Some(m.clone());
                self.event_on(node_id, NodeEventKind::Stderr, m.clone());
            }
            _ => {}
        }
        if let Some(text) = shown {
            self.show(&text);
        }
    }
}

/// The node event for an action that could not be used.
fn dropped(action: &PromptAction) -> String {
    match action {
        PromptAction::Answer { prompt_id, .. } => {
            format!("the answer to {prompt_id} came after the question was gone; it was not sent")
        }
        PromptAction::Continue { node_id } => {
            format!("node {node_id} was not waiting on a question; continue ignored")
        }
        PromptAction::Followup { node_id, .. } => {
            format!("a message for {node_id} could not be delivered")
        }
    }
}

/// The node's own status when the session ends while it runs.
fn ended(end: End) -> NodeEnd {
    let (status, reason) = match &end {
        End::Cancelled => (NodeStatus::Cancelled, CANCELLED.to_string()),
        End::Exited(code) => (NodeStatus::Failed, format!("dsh exited (code {code})")),
        End::Failed(r) => (NodeStatus::Failed, r.clone()),
    };
    (status, Some(reason), Some(end))
}

fn append_line(path: &std::path::Path, line: &str) {
    use std::io::Write as _;
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = written {
        log::warn!("appending to {} failed: {e}", path.display());
    }
}

/// Append stderr to `path`; returns its last lines (without the reasoning
/// stream).
async fn read_stderr(err: tokio::process::ChildStderr, path: PathBuf) -> Vec<String> {
    let mut lines = LineReader::new(err, LINE_MAX);
    let mut tail: Vec<String> = Vec::new();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                log::warn!("reading dsh stderr failed: {e}");
                break;
            }
        };
        append_line(&path, &line);
        if !line.trim().is_empty() && !line.starts_with("dsh: reasoning") {
            tail.push(line);
            if tail.len() > STDERR_TAIL {
                tail.remove(0);
            }
        }
    }
    tail
}

#[cfg(test)]
mod line_tests {
    use super::*;

    #[tokio::test]
    async fn lines_are_lossy_and_capped() {
        let data: &[u8] = b"ok\r\nbad \xff byte\n0123456789\nlast";
        let mut r = LineReader::new(data, 6);
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("ok"));
        assert_eq!(
            r.next_line().await.unwrap().as_deref(),
            Some("bad \u{fffd} ")
        );
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("012345"));
        assert_eq!(r.next_line().await.unwrap().as_deref(), Some("last"));
        assert_eq!(r.next_line().await.unwrap(), None);
    }
}
