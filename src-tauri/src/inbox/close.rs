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
