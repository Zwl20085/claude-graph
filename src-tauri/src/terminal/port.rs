//! The live side of [`super::deliver`]: a [`Port`] on a session's pty and
//! hooks.log, the kill/exit-aware sleep shared with the session task, and
//! follow-up messages (Messages tab) typed with the same procedure.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;

use crate::contract::{AppResult, NodeEvent, NodeEventKind};
use crate::events::Emitter;
use crate::terminal::api::{session_closed, SessionShared};
use crate::terminal::deliver::{deliver, Delivery, Halt, Outcome, Port, NOT_SUBMITTED};
use crate::terminal::hooks::{Hook, HookLog, HOOKS_LOG};
use crate::terminal::live::{composer_text, Composer};
use crate::terminal::message::{one_line, Typed};
use crate::terminal::pty::Pty;
use crate::terminal::screen::Screen;
use crate::terminal::session_dir;

pub(super) enum Tick {
    Poll,
    Killed,
    Exited(u32),
}

/// Sleep `wait` unless `kill` turns true or the process exits first.
pub(super) async fn tick(
    kill: &mut watch::Receiver<bool>,
    exit: Option<watch::Receiver<Option<u32>>>,
    wait: Duration,
) -> Tick {
    let exited = async move {
        match exit {
            Some(mut rx) => match rx.wait_for(Option::is_some).await.map(|c| *c) {
                Ok(code) => code.unwrap_or(u32::MAX),
                Err(_) => std::future::pending().await,
            },
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = tokio::time::sleep(wait) => Tick::Poll,
        Ok(_) = kill.wait_for(|k| *k) => Tick::Killed,
        code = exited => Tick::Exited(code),
    }
}

/// Delivery on a live session. Hooks that follow the `submit` line in the
/// same poll are kept in `after_submit` for the caller.
pub(super) struct PtyPort<'a> {
    pub pty: Arc<Pty>,
    pub hooks: &'a mut HookLog,
    pub after_submit: Vec<Hook>,
    /// The session's dialog announcement mark ([`Port::announced`]).
    pub announced: &'a mut Option<u64>,
    pub kill: watch::Receiver<bool>,
    pub emitter: Arc<dyn Emitter>,
    pub run_id: String,
    pub node_id: String,
}

impl Port for PtyPort<'_> {
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

    fn quiet_for(&self) -> Duration {
        self.pty
            .output
            .last_output()
            .map_or(Duration::ZERO, |t| t.elapsed())
    }

    fn screen(&self) -> Screen {
        self.pty.output.screen()
    }

    fn mark(&self) -> u64 {
        self.pty.output.mark()
    }

    fn since(&self, mark: u64) -> Screen {
        self.pty.output.screen_since(mark)
    }

    fn composer(&self, c: &Composer) -> Screen {
        Screen::of_all(&composer_text(&self.pty.output.rows(), c))
    }

    fn submitted(&mut self) -> bool {
        let hooks = self.hooks.poll().unwrap_or_else(|e| {
            log::warn!("reading {HOOKS_LOG} failed: {e}");
            Vec::new()
        });
        match hooks.iter().position(|h| *h == Hook::Submit) {
            Some(i) => {
                self.after_submit.extend_from_slice(&hooks[i + 1..]);
                true
            }
            None => false,
        }
    }

    fn event(&mut self, kind: NodeEventKind, text: String) {
        self.emitter.node_event(&NodeEvent {
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            kind,
            text,
            ts: Utc::now(),
        });
    }

    fn announced(&self) -> Option<u64> {
        *self.announced
    }

    fn set_announced(&mut self, mark: Option<u64>) {
        *self.announced = mark;
    }
}

/// Type a follow-up message to `node_id` into `session` in the background,
/// with the delivery procedure (after any delivery already in progress).
/// Progress and failure are node events; the node's status does not change.
pub fn spawn_followup(
    session: Arc<SessionShared>,
    emitter: Arc<dyn Emitter>,
    node_id: &str,
    text: &str,
) -> AppResult<()> {
    let pty = session.pty().ok_or_else(session_closed)?;
    if pty.exit_code().is_some() {
        return Err(session_closed());
    }
    let typed = Typed::Line(one_line(text));
    let node_id = node_id.to_string();
    tokio::spawn(async move {
        let _typing = session.typing().await;
        let info = session.info();
        let dir = session_dir(&session.graph_dir, &session.run_id, &info.sid);
        let mut hooks = HookLog::at_end(dir.join(HOOKS_LOG));
        let mut announced = None;
        let mut port = PtyPort {
            pty,
            hooks: &mut hooks,
            after_submit: Vec::new(),
            announced: &mut announced,
            kill: session.kill_watch(),
            emitter,
            run_id: session.run_id.clone(),
            node_id,
        };
        let (expect, ready) = session.delivery();
        let delivery = Delivery::prompt_with(&typed, &info.title, expect, ready);
        match deliver(&mut port, &delivery).await {
            Ok(Outcome::Submitted) => {}
            Ok(Outcome::NotSubmitted) => {
                port.event(NodeEventKind::Stderr, NOT_SUBMITTED.to_string());
            }
            Err(halt) => log::info!("follow-up message not delivered: {halt:?}"),
        }
    });
    Ok(())
}
