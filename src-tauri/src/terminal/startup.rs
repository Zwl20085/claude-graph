//! The start of a terminal session: folder, hooks.json, window, the pty
//! process, waiting for Claude Code to start, and settling before the first
//! message (see [`super::session`] for the rules).

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use crate::contract::{PermissionMode, SessionStatus};
use crate::prompt::{display_dir, resolve_working_dir};
use crate::runner::permission_flag;
use crate::store::write_json;
use crate::terminal::hooks::{hooks_json, Hook, HOOKS_FILE, HOOKS_LOG};
use crate::terminal::pty::{Pty, PtySpec};
use crate::terminal::session::{End, Session, Tick, POLL};
use crate::terminal::{session_dir, window_label, DEFAULT_COLS, DEFAULT_ROWS, TRANSCRIPT_FILE};

/// Markers of an enclosing Claude Code session: removed so the session
/// window is a normal top-level Claude Code (transcripts saved, no nesting).
pub const STRIPPED_ENV: [&str; 2] = ["CLAUDE_CODE_CHILD_SESSION", "CLAUDECODE"];

/// Environment changes for a session's process: `Some` sets, `None` removes.
pub fn session_env(run_id: &str, sid: &str) -> Vec<(String, Option<String>)> {
    let set = [("CLAUDE_GRAPH_RUN", run_id), ("CLAUDE_GRAPH_SESSION", sid)]
        .map(|(k, v)| (k.to_string(), Some(v.to_string())));
    let removed = STRIPPED_ENV.map(|k| (k.to_string(), None));
    set.into_iter().chain(removed).collect()
}

const START_TIMEOUT: Duration = Duration::from_secs(60);
const QUIET_START: Duration = Duration::from_secs(3);
const SETTLE_QUIET: Duration = Duration::from_secs(1);
const SETTLE_MAX: Duration = Duration::from_secs(5);

impl Session {
    pub(super) async fn start(&mut self) -> Result<(), End> {
        self.spawn()?;
        self.wait_started().await?;
        self.settle().await
    }

    fn spawn(&mut self) -> Result<(), End> {
        let ctx = self.t.ctx.clone();
        let (run_id, sid) = (ctx.run_id.clone(), self.t.plan.sid.clone());
        let dir = session_dir(&ctx.graph_dir, &run_id, &sid);
        let fail = |what: String| End::Failed(what);
        std::fs::create_dir_all(&dir).map_err(|e| fail(format!("io: {}: {e}", dir.display())))?;
        let hooks = dir.join(HOOKS_FILE);
        write_json(&hooks, &hooks_json(&dir.join(HOOKS_LOG))).map_err(|e| fail(e.message))?;
        self.report(|i| i.status = SessionStatus::Starting);
        if let Err(e) = ctx.emitter.open_window(&self.t.shared.window()) {
            log::warn!("opening the window of session {sid} failed: {e}");
        }
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
            return Err(fail(format!(
                "working directory {} does not exist",
                cwd.display()
            )));
        }
        let mut args: Vec<OsString> = ctx.claude.prefix_args.clone();
        args.extend(["--settings".into(), hooks.into_os_string()]);
        args.extend(["--add-dir".into(), ctx.graph_dir.clone().into_os_string()]);
        let mode = ctx.graph.settings.default_permission_mode;
        if let Some(p) = permission_flag(PermissionMode::Inherit, mode) {
            args.extend(["--permission-mode".into(), p.into()]);
        }
        let spec = PtySpec {
            program: ctx.claude.program.clone(),
            args,
            cwd,
            env: session_env(&run_id, &sid),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            transcript: dir.join(TRANSCRIPT_FILE),
            label: window_label(&run_id, &sid),
            run_id,
            sid,
        };
        log::info!(
            "session {}: {} in {}",
            spec.sid,
            ctx.claude.display,
            display_dir(&spec.cwd)
        );
        let pty = Arc::new(Pty::spawn(spec, ctx.emitter.clone()).map_err(|e| fail(e.message))?);
        self.exit = Some(pty.exit_watch());
        self.t.shared.set_pty(pty.clone());
        let pid = pty.pid();
        self.pty = Some(pty);
        self.report(|i| i.pid = pid);
        Ok(())
    }

    async fn wait_started(&mut self) -> Result<(), End> {
        let mut waited = Duration::ZERO;
        loop {
            match self.tick(POLL).await {
                Tick::Killed => return Err(End::Cancelled),
                Tick::Exited(code) => {
                    let tail = self.tail();
                    return Err(End::Failed(format!(
                        "Claude Code exited (code {code}) before it started: {tail}"
                    )));
                }
                Tick::Poll => {}
            }
            if self.poll_hooks().contains(&Hook::Start) {
                return Ok(());
            }
            if self.dialog_open() {
                continue;
            }
            waited += POLL;
            if waited >= START_TIMEOUT {
                let tail = self.tail();
                return Err(End::Failed(format!("Claude Code did not start: {tail}")));
            }
            if self.quiet_for(QUIET_START) {
                return Ok(());
            }
        }
    }

    /// Wait until no dialog is open and the screen was quiet for a moment.
    async fn settle(&mut self) -> Result<(), End> {
        let mut waited = Duration::ZERO;
        loop {
            if !self.dialog_open() && (self.quiet_for(SETTLE_QUIET) || waited >= SETTLE_MAX) {
                return Ok(());
            }
            match self.tick(POLL).await {
                Tick::Killed => return Err(End::Cancelled),
                Tick::Exited(code) => return Err(End::Exited(code)),
                Tick::Poll => waited += POLL,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_env_sets_ids_and_strips_parent_session_markers() {
        let env = session_env("R1", "s2");
        assert!(env.contains(&("CLAUDE_GRAPH_RUN".into(), Some("R1".into()))));
        assert!(env.contains(&("CLAUDE_GRAPH_SESSION".into(), Some("s2".into()))));
        assert!(env.contains(&("CLAUDE_CODE_CHILD_SESSION".into(), None)));
        assert!(env.contains(&("CLAUDECODE".into(), None)));
        assert_eq!(env.len(), 4);
    }
}
