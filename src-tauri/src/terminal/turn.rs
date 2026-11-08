//! The turn of one node after its message was submitted (docs/ARCHITECTURE.md
//! "Turn outcomes"): wait for the end of the reply and classify it as
//! `done` (the node ends, the chain goes on), `question` (the reply asks the
//! user something: the node is `waiting` until it is answered from the app
//! or in the window, or continued), or meanwhile `dialog` (an interactive
//! prompt on screen: `waiting` until it goes away). An answered question
//! starts another turn of the same node, classified again.

use std::time::{Duration, Instant, SystemTime};

use chrono::Utc;

use crate::contract::{
    AnswerVia, Node, NodeEventKind, NodeStatus, PromptAnswer, QuestionAnswer, QuestionRecord,
    ReplyQuestion, TurnPrompt,
};
use crate::harness::adapter::StopSignal;
use crate::run_store::{node_run_dir, RESULT_FILE};
use crate::store::{read_optional, write_atomic};
use crate::terminal::answer::{self, AnswerError, Answered};
use crate::terminal::api::PromptAction;
use crate::terminal::deliver::{deliver, Delivery, Halt, Outcome as Delivered, QUIET};
use crate::terminal::dialog;
use crate::terminal::hooks::{Hook, HookLine};
use crate::terminal::message::{one_line, Typed};
use crate::terminal::node::evaluate;
use crate::terminal::outcome::{classify, excerpt, OutcomeKind};
use crate::terminal::port::Tick;
use crate::terminal::prompts::{found, synthetic, Pending, PtyDialogIo};
use crate::terminal::reply::REPLY_FILE;
use crate::terminal::screen::normalize;
use crate::terminal::session::{End, NodeEnd, Session, CANCELLED, POLL};

/// A dialog is drawn (and opened) after this much quiet …
const OPEN_QUIET: Duration = QUIET;
/// … known from the transcript alone after this much …
const TRANSCRIPT_QUIET: Duration = Duration::from_secs(2);
/// … and gone when the screen shows none after this much.
const CLEAR_QUIET: Duration = Duration::from_millis(300);
/// An interrupted turn (no end signal) counts after this much quiet.
const INTERRUPT_QUIET: Duration = Duration::from_secs(2);
/// Hook-stop harnesses: a turn whose stop line never came ends after this
/// much quiet with an idle screen and no dialog.
pub const STOP_INFER_QUIET: Duration = Duration::from_secs(90);
/// Node event when a turn's end is inferred (no stop signal came).
pub const STOP_INFERRED: &str =
    "stop_inferred: no end-of-turn signal for 90 s and the window is idle; the turn counts as ended";
/// Node event when an answer reached the window but no dialog was drawn to
/// confirm it.
pub const ANSWER_UNCONFIRMED: &str =
    "answer_unconfirmed: the keys were sent but the window shows no dialog to confirm them; check the window";
/// Refusal when the dialog cannot be answered safely from the app.
pub const ANSWER_IN_WINDOW: &str = "answer this one in the window";
/// The question of an interrupted turn.
pub const INTERRUPTED_QUESTION: &str =
    "The turn was interrupted. What should the agent do instead?";
/// Node event when an answer did not reach the dialog.
pub const ANSWER_FAILED: &str =
    "dialog_answer_failed: the window did not take the answer; answer it in the window";

/// One message of the node's conversation and the reply's final text.
pub(super) struct Exchange {
    pub(super) user: String,
    pub(super) reply: Option<String>,
}

/// The state of a node's turn(s).
pub(super) struct Turn {
    node_id: String,
    /// Text of the latest submitted message (Codex tells its turns apart by it).
    expect: String,
    /// Output mark at the latest submit.
    mark: u64,
    /// Transcript offset where the node's first message began.
    from: u64,
    /// Claude Code transcript length at the latest submit (the turn's
    /// entries start there).
    tr_since: Option<u64>,
    started: SystemTime,
    submitted: Instant,
    convo: Vec<Exchange>,
    pending: Option<Pending>,
    /// Prompt answered from the app (id) and how, until it clears.
    answered: Option<(String, QuestionAnswer)>,
    failed_once: Option<String>,
    carried: Vec<HookLine>,
}

impl Turn {
    pub(super) fn new(
        node_id: &str,
        typed: &str,
        from: u64,
        mark: u64,
        started: SystemTime,
        carried: Vec<HookLine>,
    ) -> Self {
        Turn {
            node_id: node_id.to_string(),
            expect: typed.to_string(),
            mark,
            from,
            tr_since: None,
            started,
            submitted: Instant::now(),
            convo: vec![Exchange {
                user: typed.to_string(),
                reply: None,
            }],
            pending: None,
            answered: None,
            failed_once: None,
            carried,
        }
    }
}

/// reply.md from the conversation's final texts: each message quoted, then
/// its reply.
pub(super) fn compose(convo: &[Exchange]) -> String {
    let mut out = String::new();
    for e in convo {
        for line in e.user.lines() {
            out.push_str(&format!("> {line}\n"));
        }
        out.push('\n');
        out.push_str(e.reply.as_deref().unwrap_or("(no reply text)").trim());
        out.push_str("\n\n");
    }
    out.trim_end().to_string() + "\n"
}

/// reply.md without its first quoted line (the emulated form).
fn body_of(reply: &str) -> String {
    let mut lines = reply.lines();
    let first = lines.next().unwrap_or("");
    if first.starts_with("> ") {
        lines.collect::<Vec<_>>().join("\n")
    } else {
        reply.to_string()
    }
}

fn typed_for(text: &str) -> Typed {
    if text.trim().contains('\n') {
        let lines: Vec<String> = text.replace("\r\n", "\n").lines().map(one_line).collect();
        Typed::Paste(lines.join("\n"))
    } else {
        Typed::Line(one_line(text.trim()))
    }
}

impl Session {
    /// Wait for the end of the node's turn (see the module doc).
    pub(super) async fn turn(&mut self, node: &Node, mut t: Turn) -> NodeEnd {
        loop {
            let mut lines = std::mem::take(&mut t.carried);
            lines.extend(self.hooks.poll_lines().unwrap_or_else(|e| {
                log::warn!("reading hooks.log failed: {e}");
                Vec::new()
            }));
            if let Some(end) = self.on_hooks(node, &mut t, lines).await {
                return end;
            }
            // A pending prompt (question or dialog) means the turn is not over.
            let busy = t.pending.is_some();
            let quiet_stop = match self.behavior.stop {
                StopSignal::Quiet(d) => t.submitted.elapsed() >= d && self.quiet_for(d),
                StopSignal::Hook => self.stop_inferred(&t),
            };
            if !busy && quiet_stop {
                if self.behavior.stop == StopSignal::Hook {
                    self.event(NodeEventKind::Notification, STOP_INFERRED.to_string());
                }
                if let Some(end) = self.on_stop(node, &mut t, None).await {
                    return end;
                }
            }
            self.check_interrupted(&mut t).await;
            self.check_dialog(&mut t);
            if let Some(end) = self.on_actions(node, &mut t).await {
                return end;
            }
            match self.tick(POLL).await {
                Tick::Killed => return self.cut_short(&mut t, End::Cancelled),
                Tick::Exited(code) => return self.cut_short(&mut t, End::Exited(code)),
                Tick::Poll => {}
            }
        }
    }

    /// The session ends while the node waits or runs.
    fn cut_short(&mut self, t: &mut Turn, end: End) -> NodeEnd {
        if let Some(p) = t.pending.take() {
            self.clear_prompt(&t.node_id, &p, None, AnswerVia::Window, false);
        }
        let (status, reason) = match &end {
            End::Cancelled => (NodeStatus::Cancelled, CANCELLED.to_string()),
            End::Exited(code) => (
                NodeStatus::Failed,
                format!("the CLI exited (code {code}) before the node finished"),
            ),
            End::Failed(r) => (NodeStatus::Failed, r.clone()),
        };
        (status, Some(reason), Some(end))
    }

    async fn on_hooks(
        &mut self,
        node: &Node,
        t: &mut Turn,
        lines: Vec<HookLine>,
    ) -> Option<NodeEnd> {
        for line in lines {
            self.note_transcript(&line);
            match line.hook {
                Hook::Submit => {
                    t.mark = self.output_mark();
                    t.submitted = Instant::now();
                    t.tr_since = self.transcript_len();
                    let text = line.field("prompt");
                    if let Some(p) = t.pending.take_if(|p| p.is_reply()) {
                        let answer = QuestionAnswer {
                            ts: Utc::now(),
                            via: AnswerVia::Window,
                            text: text.clone(),
                            index: None,
                        };
                        self.clear_prompt(&t.node_id, &p, Some(answer), AnswerVia::Window, true);
                        t.convo.push(Exchange {
                            user: text
                                .clone()
                                .unwrap_or_else(|| "(typed in the window)".into()),
                            reply: None,
                        });
                    }
                    if let Some(text) = text {
                        t.expect = text;
                    }
                }
                Hook::Notify => {
                    if t.pending.is_none() {
                        let (label, title) = (&self.behavior.label, &self.t.plan.title);
                        self.event(
                            NodeEventKind::Notification,
                            format!("{label} is waiting for you in window {title}"),
                        );
                    }
                }
                Hook::Stop => {
                    let replying = t.pending.as_ref().is_some_and(Pending::is_reply);
                    if !replying && !line.stops_turn(&t.expect) {
                        continue;
                    }
                    if let Some(p) = t.pending.take_if(|p| p.is_reply()) {
                        // Answered in a window that fires no submit hook (Codex).
                        self.clear_prompt(&t.node_id, &p, None, AnswerVia::Window, true);
                        t.convo.push(Exchange {
                            user: "(typed in the window)".into(),
                            reply: None,
                        });
                    }
                    if let Some(end) = self.on_stop(node, t, line.final_text()).await {
                        return Some(end);
                    }
                }
                Hook::Start => {}
            }
        }
        None
    }

    fn output_mark(&self) -> u64 {
        self.pty.as_ref().map_or(0, |p| p.output.mark())
    }

    /// Safety net for hook-stop harnesses: [`STOP_INFER_QUIET`] of quiet
    /// since the submit, the input box idle and no dialog drawn.
    fn stop_inferred(&self, t: &Turn) -> bool {
        if t.submitted.elapsed() < STOP_INFER_QUIET || !self.quiet_for(STOP_INFER_QUIET) {
            return false;
        }
        let Some(pty) = self.pty.as_ref() else {
            return false;
        };
        let markers = pty.output.markers();
        pty.output.screen().idle()
            && dialog::parse(&pty.output.rows(), &markers.idle, &markers.style).is_none()
    }

    fn transcript_len(&self) -> Option<u64> {
        self.transcript.as_ref().map(|tr| tr.file_len())
    }

    /// The end of a reply: classify its final text.
    async fn on_stop(
        &mut self,
        node: &Node,
        t: &mut Turn,
        text: Option<String>,
    ) -> Option<NodeEnd> {
        if let Some(p) = t.pending.take() {
            let via = self.via(t, &p);
            let answer = t.answered.take().map(|(_, a)| a);
            self.clear_prompt(&t.node_id, &p, answer, via, true);
        }
        self.settle().await;
        let since = t.tr_since;
        let text = text.or_else(|| {
            let tr = self.transcript.as_mut()?;
            tr.refresh();
            tr.last_assistant_text_since(since)
        });
        if let Some(last) = t.convo.last_mut() {
            last.reply = text.clone();
        }
        let body = self.write_turn_reply(t).await;
        let outcome = classify(&body);
        let (status, reason) = evaluate(&self.t.ctx, node, t.started);
        if outcome.kind == OutcomeKind::Done {
            return Some((status, reason, None));
        }
        let id = self.next_prompt_id(&t.node_id);
        let question = ReplyQuestion {
            id,
            question: outcome.question.unwrap_or_default(),
            choices: outcome.choices,
            reply_excerpt: excerpt(&body),
        };
        let offer = !node.deliverables.is_empty() && status == NodeStatus::Succeeded;
        if offer {
            let prompt = TurnPrompt::Reply(question);
            self.record(
                &t.node_id,
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
            return Some((status, reason, None));
        }
        let pending = Pending::Reply(question);
        self.open_prompt(&t.node_id, &pending);
        t.pending = Some(pending);
        None
    }

    /// reply.md / result.md now; returns the text to classify.
    async fn write_turn_reply(&mut self, t: &Turn) -> String {
        let ctx = self.t.ctx.clone();
        let dir = node_run_dir(&ctx.graph_dir, &ctx.run_id, &t.node_id);
        let last = t.convo.last().and_then(|e| e.reply.clone());
        if let Some(last) = last {
            let text = compose(&t.convo);
            for f in [REPLY_FILE, RESULT_FILE] {
                if let Err(e) = write_atomic(&dir.join(f), text.as_bytes()) {
                    log::warn!("writing {f} of {} failed: {e}", t.node_id);
                }
            }
            let to = self
                .pty
                .as_ref()
                .map_or(t.from, |p| p.output.transcript_len());
            self.span = Some((t.from, to));
            return last;
        }
        let first = t.convo.first().map_or("", |e| e.user.as_str()).to_string();
        self.span = self.save_reply(&t.node_id, &first, t.from).await;
        let reply = read_optional(&dir.join(REPLY_FILE))
            .ok()
            .flatten()
            .unwrap_or_default();
        body_of(&reply)
    }

    /// Claude Code drew "Interrupted · What should Claude do instead?" (Esc
    /// at a dialog) and fires no Stop: the node waits on that question.
    async fn check_interrupted(&mut self, t: &mut Turn) {
        if t.pending.is_some() || !self.quiet_for(INTERRUPT_QUIET) {
            return;
        }
        let Some(pty) = self.pty.clone() else { return };
        let markers = pty.output.markers();
        if markers.style.interrupted.is_empty() || !pty.output.screen().idle() {
            return;
        }
        let since = normalize(&pty.output.since(t.mark));
        if !dialog::interrupted(&since, &markers.style) {
            return;
        }
        t.mark = pty.output.mark();
        if let Some(last) = t.convo.last_mut() {
            last.reply = None;
        }
        let body = self.write_turn_reply(t).await;
        let id = self.next_prompt_id(&t.node_id);
        let pending = Pending::Reply(ReplyQuestion {
            id,
            question: INTERRUPTED_QUESTION.to_string(),
            choices: Vec::new(),
            reply_excerpt: excerpt(&body),
        });
        self.open_prompt(&t.node_id, &pending);
        t.pending = Some(pending);
    }

    fn via(&self, t: &Turn, p: &Pending) -> AnswerVia {
        match &t.answered {
            Some((id, _)) if id == p.id() => AnswerVia::App,
            _ => AnswerVia::Window,
        }
    }

    /// Open, update or clear the dialog per the screen and the transcript.
    fn check_dialog(&mut self, t: &mut Turn) {
        if t.pending.as_ref().is_some_and(Pending::is_reply) {
            return;
        }
        let Some(pty) = self.pty.clone() else { return };
        let quiet = pty
            .output
            .last_output()
            .map_or(Duration::ZERO, |l| l.elapsed());
        if quiet < CLEAR_QUIET {
            return;
        }
        let markers = pty.output.markers();
        let screen = dialog::parse(&pty.output.rows(), &markers.idle, &markers.style);
        if let Some(tr) = self.transcript.as_mut() {
            tr.refresh();
        }
        let tool = self.transcript.as_ref().and_then(|tr| tr.pending_tool());
        let screen = screen.filter(|_| quiet >= OPEN_QUIET || t.pending.is_some());
        let now = found(screen, tool.as_ref(), quiet >= TRANSCRIPT_QUIET);
        match (t.pending.take(), now) {
            (None, Some(f)) => self.open_dialog(t, f),
            (Some(old), Some(f)) if f.same_as(&old) && self.answered_here(t, &old) => {
                // The same dialog again after an answer from the app: a new
                // prompt (another call with the same labels, or the answer
                // did not take), never the old one with its old header.
                if quiet >= OPEN_QUIET {
                    self.close_dialog(t, &old);
                    self.open_dialog(t, f);
                } else {
                    t.pending = Some(old);
                }
            }
            (Some(old), Some(f)) if f.same_as(&old) => self.update_dialog(t, old, f),
            (Some(old), Some(f)) => {
                self.close_dialog(t, &old);
                if quiet >= OPEN_QUIET {
                    self.open_dialog(t, f);
                }
            }
            (Some(old), None) => {
                let gone = quiet >= OPEN_QUIET || pty.output.screen().idle();
                if old.is_reply() || !gone {
                    t.pending = Some(old);
                } else {
                    self.close_dialog(t, &old);
                }
            }
            (None, None) => {}
        }
    }

    fn open_dialog(&mut self, t: &mut Turn, f: crate::terminal::prompts::Found) {
        let id = self.next_prompt_id(&t.node_id);
        let prompt = crate::contract::DialogPrompt { id, ..f.prompt };
        let pending = Pending::Dialog {
            prompt,
            screen: f.screen.map(Box::new),
            sig: f.sig,
            tool_id: f.tool_id,
            questions: f.questions,
        };
        self.open_prompt(&t.node_id, &pending);
        t.pending = Some(pending);
    }

    /// `old` was answered from the app (its answer is waiting to clear).
    fn answered_here(&self, t: &Turn, old: &Pending) -> bool {
        t.answered.as_ref().is_some_and(|(id, _)| id == old.id())
    }

    /// The same dialog redrawn: the highlighted row, and the tool details
    /// once the transcript names the call (published again).
    fn update_dialog(&mut self, t: &mut Turn, old: Pending, f: crate::terminal::prompts::Found) {
        let Pending::Dialog {
            prompt,
            sig,
            tool_id,
            questions,
            ..
        } = old
        else {
            t.pending = Some(old);
            return;
        };
        let learned = tool_id.is_none() && f.tool_id.is_some();
        let prompt = if learned {
            crate::contract::DialogPrompt {
                id: prompt.id,
                ..f.prompt
            }
        } else {
            crate::contract::DialogPrompt {
                selected: f.prompt.selected,
                ..prompt
            }
        };
        let pending = Pending::Dialog {
            prompt,
            screen: f.screen.map(Box::new),
            sig,
            tool_id: tool_id.or(f.tool_id),
            questions: if learned { f.questions } else { questions },
        };
        if learned {
            self.republish_prompt(&t.node_id, &pending);
        }
        t.pending = Some(pending);
    }

    fn close_dialog(&mut self, t: &mut Turn, old: &Pending) {
        let via = self.via(t, old);
        let answer = t
            .answered
            .take()
            .filter(|(id, _)| id == old.id())
            .map(|(_, a)| a);
        self.clear_prompt(&t.node_id, old, answer, via, true);
    }

    async fn on_actions(&mut self, node: &Node, t: &mut Turn) -> Option<NodeEnd> {
        loop {
            let action = self.actions.as_mut()?.try_recv().ok()?;
            match action {
                PromptAction::Continue { node_id } => {
                    let Some(p) = t.pending.take_if(|p| p.is_reply() && node_id == t.node_id)
                    else {
                        continue;
                    };
                    let answer = QuestionAnswer {
                        ts: Utc::now(),
                        via: AnswerVia::Continue,
                        text: None,
                        index: None,
                    };
                    self.clear_prompt(&t.node_id, &p, Some(answer), AnswerVia::Continue, false);
                    let (status, reason) = evaluate(&self.t.ctx, node, t.started);
                    return Some((status, reason, None));
                }
                PromptAction::Followup { .. } => {
                    log::info!("pty session: a follow-up goes through the typing path");
                }
                PromptAction::Answer { prompt_id, answer } => {
                    let Some(p) = t.pending.clone().filter(|p| p.id() == prompt_id) else {
                        self.event(
                            NodeEventKind::Stderr,
                            format!("prompt {prompt_id} was already answered"),
                        );
                        continue;
                    };
                    let halted = match p {
                        Pending::Reply(q) => self.answer_reply(t, &q, &answer).await,
                        Pending::Dialog {
                            screen: None,
                            questions,
                            ..
                        } if questions > 1 => {
                            // Known from the transcript only: which of the
                            // call's questions the window shows is unknown.
                            self.event(NodeEventKind::Stderr, ANSWER_IN_WINDOW.to_string());
                            None
                        }
                        Pending::Dialog { prompt, screen, .. } => {
                            let blind = screen.is_none();
                            let screen = screen.map(|b| *b).unwrap_or_else(|| synthetic(&prompt));
                            self.answer_dialog(t, &prompt, &screen, &answer, blind)
                                .await
                        }
                    };
                    if let Some(end) = halted {
                        return Some(self.cut_short(t, end));
                    }
                }
            }
        }
    }

    /// Type the answer to a reply question as the next message.
    async fn answer_reply(
        &mut self,
        t: &mut Turn,
        q: &ReplyQuestion,
        a: &PromptAnswer,
    ) -> Option<End> {
        let refuse = |s: &mut Self, why: String| {
            s.event(NodeEventKind::Stderr, why);
            None
        };
        let (text, index) = match a {
            PromptAnswer::Text(s) => (s.clone(), None),
            PromptAnswer::Choice(i) => match q.choices.get(*i) {
                Some(c) if crate::terminal::prompt_api::unsafe_to_type(&c.label) => {
                    return refuse(self, crate::terminal::prompt_api::choice_refused(&c.label));
                }
                Some(c) => (c.label.clone(), Some(*i)),
                None => return refuse(self, format!("no option {i}: answer not sent")),
            },
            PromptAnswer::Choices(_) => {
                return refuse(self, "this question takes one answer, not several".into())
            }
        };
        let typed = typed_for(&text);
        let shared = self.t.shared.clone();
        let _typing = shared.typing().await;
        let title = self.t.plan.title.clone();
        let (expect, ready) = (self.behavior.expect.clone(), self.behavior.ready_quiet);
        let Some(mut port) = self.port(&t.node_id) else {
            return refuse(
                self,
                "the window is not ready for typing; answer in the window".into(),
            );
        };
        let delivery = Delivery::prompt_with(&typed, &title, expect, ready);
        let result = deliver(&mut port, &delivery).await;
        let carried = std::mem::take(&mut port.after_submit);
        let mark = port.pty.output.mark();
        drop(port);
        match result {
            Ok(Delivered::Submitted) => {
                t.carried.extend(carried);
                let p = t.pending.take()?;
                let answer = QuestionAnswer {
                    ts: Utc::now(),
                    via: AnswerVia::App,
                    text: Some(text.clone()),
                    index,
                };
                self.clear_prompt(&t.node_id, &p, Some(answer), AnswerVia::App, true);
                t.convo.push(Exchange {
                    user: typed.text().to_string(),
                    reply: None,
                });
                t.expect = typed.text().to_string();
                t.mark = mark;
                t.submitted = Instant::now();
                None
            }
            Ok(Delivered::NotSubmitted) => {
                self.event(
                    NodeEventKind::Stderr,
                    "could not deliver the answer; type it in the window".to_string(),
                );
                None
            }
            Err(Halt::Killed) => Some(End::Cancelled),
            Err(Halt::Exited(code)) => Some(End::Exited(code)),
        }
    }

    /// Send the keys for `a` to the dialog on screen.
    async fn answer_dialog(
        &mut self,
        t: &mut Turn,
        prompt: &crate::contract::DialogPrompt,
        screen: &dialog::ScreenDialog,
        a: &PromptAnswer,
        blind: bool,
    ) -> Option<End> {
        let Some(pty) = self.pty.clone() else {
            self.event(NodeEventKind::Stderr, ANSWER_IN_WINDOW.to_string());
            return None;
        };
        let style = pty.output.markers().style.clone();
        let shared = self.t.shared.clone();
        let _typing = shared.typing().await;
        let mut io = PtyDialogIo {
            pty,
            kill: self.kill.clone(),
        };
        // The dialog on screen must still be the one the user answered.
        let on_screen = answer::DialogIo::dialog(&io);
        if let Some(now) = &on_screen {
            if !blind && now.signature() != screen.signature() {
                self.event(
                    NodeEventKind::Stderr,
                    format!("the dialog in the window changed; {ANSWER_IN_WINDOW}"),
                );
                return None;
            }
        }
        let plain = prompt.kind == crate::contract::DialogKind::Text;
        let result = answer::answer(&mut io, screen, a, &style, plain).await;
        let (text, index) = match a {
            PromptAnswer::Text(s) => (Some(s.clone()), None),
            PromptAnswer::Choice(i) => (prompt.options.get(*i).map(|o| o.label.clone()), Some(*i)),
            PromptAnswer::Choices(list) => (
                Some(
                    list.iter()
                        .filter_map(|i| prompt.options.get(*i).map(|o| o.label.clone()))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                None,
            ),
        };
        match result {
            Ok(done @ (Answered::Changed | Answered::Unknown)) => {
                let answer = QuestionAnswer {
                    ts: Utc::now(),
                    via: AnswerVia::App,
                    text,
                    index,
                };
                t.answered = Some((prompt.id.clone(), answer));
                if done == Answered::Unknown {
                    self.event(NodeEventKind::Stderr, ANSWER_UNCONFIRMED.to_string());
                }
                None
            }
            Ok(Answered::Unchanged) => {
                if t.failed_once.as_deref() != Some(prompt.id.as_str()) {
                    t.failed_once = Some(prompt.id.clone());
                    self.event(NodeEventKind::Stderr, ANSWER_FAILED.to_string());
                }
                None
            }
            Err(AnswerError::Invalid(e)) => {
                self.event(NodeEventKind::Stderr, e.message);
                None
            }
            Err(AnswerError::Halt(Halt::Killed)) => Some(End::Cancelled),
            Err(AnswerError::Halt(Halt::Exited(c))) => Some(End::Exited(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_quotes_each_message_before_its_reply() {
        let convo = vec![
            Exchange {
                user: "Set up tests".into(),
                reply: Some("Which framework?\n1. vitest".into()),
            },
            Exchange {
                user: "vitest".into(),
                reply: Some("Done.".into()),
            },
        ];
        assert_eq!(
            compose(&convo),
            "> Set up tests\n\nWhich framework?\n1. vitest\n\n> vitest\n\nDone.\n"
        );
        assert_eq!(body_of("> prompt\nline 1\nline 2"), "line 1\nline 2");
        assert_eq!(body_of("no quote"), "no quote");
    }

    #[test]
    fn answers_with_lines_are_pasted() {
        assert!(matches!(typed_for("one line "), Typed::Line(s) if s == "one line"));
        assert!(matches!(typed_for("a\r\nb"), Typed::Paste(s) if s == "a\nb"));
    }
}
