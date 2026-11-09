//! Retries: outcomes whose write-back failed for a reason that may pass
//! (read-only tip, file kept changing, i/o error) are kept and written at the
//! next scan, and a tip whose earlier session or graph folder is still busy
//! gets a re-scan after [`super::InboxEnv::retry_delay`] (at most
//! [`RESCAN_MAX`] in a row) instead of waiting for the next timed scan.

use std::sync::atomic::Ordering;

use crate::run_loop::blocking;

use super::block::Outcome;
use super::runner::Inbox;
use super::watch::{write_round, Round, Written};

/// Most delayed re-scans in a row.
pub const RESCAN_MAX: u32 = 5;

/// A finished round whose outcome is not in the tip yet.
#[derive(Debug, Clone)]
pub(crate) struct Unwritten {
    pub round: Round,
    pub outcome: Outcome,
    /// Why the last write failed.
    pub reason: String,
}

impl Inbox {
    /// Keep `outcome` of `round` for the next scan.
    pub(crate) fn keep_unwritten(&self, round: &Round, outcome: &Outcome, reason: &str) {
        self.lock().unwritten.insert(
            round.tip_id.clone(),
            Unwritten {
                round: round.clone(),
                outcome: outcome.clone(),
                reason: reason.to_string(),
            },
        );
    }

    /// Try every kept outcome again (start of a scan).
    pub(crate) async fn retry_unwritten(&self) {
        let pending: Vec<Unwritten> = self.lock().unwritten.values().cloned().collect();
        for u in pending {
            let ib = self.clone();
            let item = u.clone();
            let result = blocking(move || Ok(write_round(&ib, &item.round, &item.outcome))).await;
            let id = &u.round.tip_id;
            match result {
                Ok(Written::Yes(_)) | Ok(Written::Dropped) => {
                    self.lock().unwritten.remove(id);
                }
                Ok(Written::Kept) => {}
                Err(e) => self.log(Some(id), format!("retrying the write-back: {}", e.message)),
            }
        }
    }

    /// A re-scan after the retry delay, unless one is pending or the budget
    /// of [`RESCAN_MAX`] in a row is used up (then the next timed scan).
    pub(crate) fn schedule_rescan(&self) {
        let inner = &self.0;
        if inner.retry_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let n = inner.retries.fetch_add(1, Ordering::SeqCst) + 1;
        if n > RESCAN_MAX {
            inner.retry_pending.store(false, Ordering::SeqCst);
            self.log(
                None,
                "busy graph folders: retries used up; waiting for the next scan",
            );
            return;
        }
        let ib = self.clone();
        let delay = self.env().retry_delay;
        self.log(
            None,
            format!(
                "re-scan in {} s (retry {n} of {RESCAN_MAX})",
                delay.as_secs()
            ),
        );
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            ib.0.retry_pending.store(false, Ordering::SeqCst);
            if ib.is_stopping() {
                return;
            }
            if let Err(e) = ib.scan_now().await {
                ib.log(None, format!("re-scan failed: {}", e.message));
            }
        });
    }

    /// A scan that needed no retry resets the budget.
    pub(crate) fn reset_retries(&self) {
        self.0.retries.store(0, Ordering::SeqCst);
    }
}
