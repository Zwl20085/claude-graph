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

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
        // No process lives between messages: the app's pid says "takes messages".
        self.report(|i| i.pid = Some(std::process::id()));
        Ok(())
    }

    fn show(&self, text: &str) {
        if let Some(c) = &self.chain {
            c.view.show(text);
        }
    }

    /// One node of a chained session (see the module doc).
    pub(super) async fn run_chained_node(
        &mut self,
        id: &str,
        started_at: DateTime<Utc>,
    ) -> NodeEnd {
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

    /// The node waits on `pending`: `Some(text)` to send as the answer,
    /// `None` for "continue".
    async fn wait_answer(&mut self, node: &Node, pending: Pending) -> Result<Option<String>, End> {
        self.open_prompt(&node.id, &pending);
        let Pending::Reply(q) = &pending else {
            return Ok(None);
        };
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
                        None => continue,
                    },
                    PromptAnswer::Choices(_) => continue,
                },
                PromptAction::Followup { node_id, text } if node_id == node.id => {
                    (Some(text), None, AnswerVia::App)
                }
                other => {
                    log::info!("chained session: ignored {other:?} while {} waits", node.id);
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
        loop {
            let Ok(action) = self.next_action().await else {
                return;
            };
            let PromptAction::Followup { node_id, text } = action else {
                continue;
            };
            self.show(&format!(
                "\r\n{BOLD}\u{25b6} message to {node_id}{RESET}\r\n"
            ));
            self.event_on(&node_id, NodeEventKind::UserMessage, text.clone());
            match self.dsh_turn(&node_id, &text).await {
                Ok(r) if r.code != 0 => self.event_on(
                    &node_id,
                    NodeEventKind::Stderr,
                    format!(
                        "dsh exited with code {}: {}",
                        r.code,
                        r.error.unwrap_or_default()
                    ),
                ),
                Ok(_) => {}
                Err(_) => return,
            }
        }
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
        for (k, v) in session_env(&run_id, &sid, chain.launch.env.clone()) {
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
        let _job = Job::assign(&child);
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
        if let Some(out) = child.stdout.take() {
            let mut lines = BufReader::new(out).lines();
            let mut kill = self.t.shared.kill_watch();
            loop {
                let line = tokio::select! {
                    l = lines.next_line() => l,
                    _ = async { kill.wait_for(|k| *k).await.map(|_| ()) } => {
                        if let Some(pid) = pid {
                            crate::runner::kill_tree(pid).await;
                        }
                        let _ = child.kill().await;
                        return Err(End::Cancelled);
                    }
                };
                let Ok(Some(line)) = line else { break };
                append_line(&events_path, &line);
                if let Some(ev) = parse_line(&line) {
                    self.on_dsh_event(node_id, &ev, &mut tools, &mut ran);
                }
            }
        }
        let status = child.wait().await;
        let tail = match stderr_task {
            Some(t) => t.await.unwrap_or_default(),
            None => Vec::new(),
        };
        ran.code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
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
    let mut lines = BufReader::new(err).lines();
    let mut tail: Vec<String> = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
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
