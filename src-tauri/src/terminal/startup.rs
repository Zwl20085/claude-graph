//! The start of a terminal session: folder, hooks.json, window, the pty
//! process of the session's harness and waiting for it to start (see
//! [`super::session`] for the rules). Readiness for typing is
//! [`super::deliver::gate`]'s job.

use std::sync::Arc;
use std::time::Duration;

use crate::claude_bin::ResolveEnv;
use crate::contract::{PermissionMode, SessionStatus};
use crate::harness::adapter::{spawn_spec, Driver, Launch, SpawnInput};
use crate::prompt::{display_dir, resolve_working_dir};
use crate::runner::permission_flag;
use crate::store::write_json;
use crate::terminal::deliver::QUIET;
use crate::terminal::hooks::{hooks_json, Hook, HOOKS_FILE, HOOKS_LOG};
use crate::terminal::port::Tick;
use crate::terminal::pty::{Pty, PtySpec};
use crate::terminal::session::{End, Session, POLL};
use crate::terminal::{session_dir, window_label, DEFAULT_COLS, DEFAULT_ROWS, TRANSCRIPT_FILE};

/// Markers of an enclosing Claude Code session: removed so the session
/// window is a normal top-level Claude Code (transcripts saved, no nesting).
pub const STRIPPED_ENV: [&str; 2] = ["CLAUDE_CODE_CHILD_SESSION", "CLAUDECODE"];

/// Environment changes for a session's process: `Some` sets, `None` removes.
/// `profile_env` (the harness profile's, expanded) goes on top of the
/// stripped environment; the session ids win over it.
pub fn session_env(
    run_id: &str,
    sid: &str,
    profile_env: Vec<(String, Option<String>)>,
) -> Vec<(String, Option<String>)> {
    let removed = STRIPPED_ENV.map(|k| (k.to_string(), None));
    let set = [("CLAUDE_GRAPH_RUN", run_id), ("CLAUDE_GRAPH_SESSION", sid)]
        .map(|(k, v)| (k.to_string(), Some(v.to_string())));
    removed.into_iter().chain(profile_env).chain(set).collect()
}

const START_TIMEOUT: Duration = Duration::from_secs(60);
const QUIET_START: Duration = Duration::from_secs(3);

impl Session {
    pub(super) async fn start(&mut self) -> Result<(), End> {
        if self.behavior.driver == Driver::Chained {
            return self.start_chained();
        }
        self.spawn()?;
        self.wait_started().await
    }

    /// Program, args and env of the session's harness (no process yet).
    pub(super) fn launch_spec(&self, cwd: &std::path::Path) -> Result<Launch, End> {
        let ctx = &self.t.ctx;
        let profile = ctx
            .harness(&self.t.plan.harness)
            .map_err(|e| End::Failed(e.message))?;
        let dir = session_dir(&ctx.graph_dir, &ctx.run_id, &self.t.plan.sid);
        let mode = ctx.graph.settings.default_permission_mode;
        let resolve = ResolveEnv::from_process(None);
        spawn_spec(&SpawnInput {
            profile,
            claude: Some(&ctx.claude),
            graph_dir: &ctx.graph_dir,
            cwd,
            session_dir: &dir,
            notify_exe: &ctx.notify_exe,
            permission: permission_flag(PermissionMode::Inherit, mode),
            lookup: &|name| std::env::var(name).ok(),
            resolve: &resolve,
        })
        .map_err(|e| End::Failed(e.message))
    }

    fn spawn(&mut self) -> Result<(), End> {
        let ctx = self.t.ctx.clone();
        let (run_id, sid) = (ctx.run_id.clone(), self.t.plan.sid.clone());
        let dir = session_dir(&ctx.graph_dir, &run_id, &sid);
        let fail = |what: String| End::Failed(what);
        std::fs::create_dir_all(&dir).map_err(|e| fail(format!("io: {}: {e}", dir.display())))?;
        let hooks = dir.join(HOOKS_FILE);
        write_json(
            &hooks,
            &hooks_json(&dir.join(HOOKS_LOG), Some(&ctx.notify_exe)),
        )
        .map_err(|e| fail(e.message))?;
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
            return Err(fail(format!(
                "working directory {} does not exist",
                cwd.display()
            )));
        }
        // Before the window and the process: a missing ${VAR} or program
        // fails the session with nothing spawned.
        let launch = self.launch_spec(&cwd)?;
        if let Err(e) = ctx.emitter.open_window(&self.t.shared.window()) {
            log::warn!("opening the window of session {sid} failed: {e}");
        }
        let spec = PtySpec {
            program: launch.program,
            args: launch.args,
            cwd,
            env: session_env(&run_id, &sid, launch.env),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            transcript: dir.join(TRANSCRIPT_FILE),
            label: window_label(&run_id, &sid),
            run_id,
            sid,
            markers: self.behavior.markers.clone(),
        };
        log::info!(
            "session {} ({}): {} in {}",
            spec.sid,
            self.t.plan.harness,
            launch.display,
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

    /// Started: the `start` hook line (harnesses that have one), or the
    /// input box on screen after 1 s of quiet, or 3 s of quiet. A dialog
    /// pauses the timeout.
    async fn wait_started(&mut self) -> Result<(), End> {
        let label = self.behavior.label.clone();
        let mut waited = Duration::ZERO;
        loop {
            match self.tick(POLL).await {
                Tick::Killed => return Err(End::Cancelled),
                Tick::Exited(code) => {
                    let tail = self.tail();
                    return Err(End::Failed(format!(
                        "{label} exited (code {code}) before it started: {tail}"
                    )));
                }
                Tick::Poll => {}
            }
            if self.behavior.start_hook && self.poll_hooks().contains(&Hook::Start) {
                return Ok(());
            }
            if self.dialog_open() {
                continue;
            }
            waited += POLL;
            if waited >= START_TIMEOUT {
                let tail = self.tail();
                return Err(End::Failed(format!("{label} did not start: {tail}")));
            }
            let idle = self.pty.as_ref().is_some_and(|p| p.output.screen().idle());
            if (idle && self.quiet_for(QUIET)) || self.quiet_for(QUIET_START) {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_env_sets_ids_and_strips_parent_session_markers() {
        let env = session_env("R1", "s2", Vec::new());
        assert!(env.contains(&("CLAUDE_GRAPH_RUN".into(), Some("R1".into()))));
        assert!(env.contains(&("CLAUDE_GRAPH_SESSION".into(), Some("s2".into()))));
        assert!(env.contains(&("CLAUDE_CODE_CHILD_SESSION".into(), None)));
        assert!(env.contains(&("CLAUDECODE".into(), None)));
        assert_eq!(env.len(), 4);
    }

    #[test]
    fn profile_env_sits_between_the_stripped_markers_and_the_ids() {
        let profile = vec![
            ("ANTHROPIC_BASE_URL".to_string(), Some("u".to_string())),
            ("CLAUDE_GRAPH_RUN".to_string(), Some("spoof".to_string())),
        ];
        let env = session_env("R1", "s1", profile);
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            [
                "CLAUDE_CODE_CHILD_SESSION",
                "CLAUDECODE",
                "ANTHROPIC_BASE_URL",
                "CLAUDE_GRAPH_RUN",
                "CLAUDE_GRAPH_RUN",
                "CLAUDE_GRAPH_SESSION"
            ]
        );
        assert_eq!(env[4].1.as_deref(), Some("R1"), "the run id is set last");
    }
}
