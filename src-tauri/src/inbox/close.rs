//! Closing the session of an Inbox round that ended `done`: its window is
//! closed and its process ended right after the outcome and the results were
//! written (ordinary graph runs never come here). A round that ended
//! `question` or `failed` keeps its session (a failed one is ended, its
//! window closed, at the next scan, [`super::runner::Inbox::close_finished`]).
//! The run itself is not cancelled here: it succeeded and stays so.
//!
//! Closing never changes what was written: a failure is logged, and a
//! session that does not end in time keeps its entry, so the next scan's
//! `close_finished` ends it.

use std::time::{Duration, Instant};

use super::runner::Inbox;

/// Tries (one per scan) to record a run's ended sessions before giving up.
pub(crate) const UNRECORDED_TRIES: u32 = 20;

/// How long closing waits for the session process to end.
pub(crate) const CLOSE_WAIT: Duration = Duration::from_secs(20);

impl Inbox {
    /// Close the windows of graph run `run` (tip `id`) and end its session
    /// processes. Call only after the done outcome is in the tip.
    pub(crate) async fn close_done(&self, id: &str, run: &str) {
        let app = &self.env().app;
        // Only the sessions are ended (no `cancel_run`: the run succeeded and
        // must stay recorded so).
        app.terminals().kill_run(run);
        let alive = || {
            app.terminals()
                .all_of(run)
                .iter()
                .any(|s| s.is_task_alive())
        };
        let until = Instant::now() + CLOSE_WAIT;
        while alive() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.close_windows(id, run);
        if alive() {
            self.log(
                Some(id),
                format!(
                    "the done session (run {run}) did not end within {} s; \
                     the next scan closes it",
                    CLOSE_WAIT.as_secs()
                ),
            );
            return;
        }
        self.record_ended(id, run);
        {
            let mut s = self.lock();
            // Only this round's entry: a newer round may have replaced it.
            let ours = s
                .active
                .get(id)
                .is_some_and(|a| a.run.graph_run_id.as_deref() == Some(run));
            if ours {
                s.active.remove(id);
            }
        }
        self.log(Some(id), format!("closed the done session (run {run})"));
        self.emit();
    }

    /// Write the ended sessions of graph run `run` into its run.json (no
    /// session alive, no pid), the run's status untouched. Only once the
    /// run's own loop has ended: while it runs it records them itself.
    /// The folder is held through the registry for the write, so a run
    /// resumed meanwhile cannot interleave with it; when the write has to
    /// wait (the loop or a session still alive), it is retried at the next
    /// [`Inbox::close_finished`].
    pub(crate) fn record_ended(&self, id: &str, run: &str) {
        let app = &self.env().app;
        let sessions = app.terminals().all_of(run);
        let Some(dir) = sessions.first().map(|s| s.graph_dir.clone()) else {
            self.lock().unrecorded.remove(run);
            return;
        };
        let write = || {
            if sessions.iter().any(|s| s.is_task_alive()) {
                return false;
            }
            let infos: Vec<_> = sessions.iter().map(|s| s.info()).collect();
            if let Err(e) = crate::run_store::record_sessions(&dir, run, &infos) {
                self.log(
                    Some(id),
                    format!("recording the ended session of run {run}: {}", e.message),
                );
            }
            true
        };
        let done = app.with_dir_reserved(&dir, write).unwrap_or(false);
        let gave_up = {
            let mut s = self.lock();
            if done {
                s.unrecorded.remove(run);
                false
            } else {
                let tries = s.unrecorded.get(run).map_or(0, |(_, n)| *n) + 1;
                if tries >= UNRECORDED_TRIES {
                    s.unrecorded.remove(run);
                    true
                } else {
                    s.unrecorded
                        .insert(run.to_string(), (id.to_string(), tries));
                    false
                }
            }
        };
        if gave_up {
            self.log(
                Some(id),
                format!(
                    "gave up recording the ended session of run {run} after {UNRECORDED_TRIES} tries; its run.json may still say it is alive"
                ),
            );
        }
    }

    /// [`Self::record_ended`] again for the runs that had to wait.
    pub(crate) fn retry_unrecorded(&self) {
        let pending: Vec<(String, String)> = self
            .lock()
            .unrecorded
            .iter()
            .map(|(r, (t, _))| (r.clone(), t.clone()))
            .collect();
        for (run, id) in pending {
            self.record_ended(&id, &run);
        }
    }

    /// Close the session windows of graph run `run` (failures logged).
    pub(crate) fn close_windows(&self, id: &str, run: &str) {
        let env = self.env();
        for session in env.app.terminals().all_of(run) {
            let label = session.window().label;
            if let Err(e) = env.emitter.close_window(&label) {
                self.log(
                    Some(id),
                    format!("closing the window {label}: {}", e.message),
                );
            }
        }
    }
}
