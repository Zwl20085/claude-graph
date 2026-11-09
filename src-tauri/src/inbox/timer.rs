//! The in-app timer: a first scan about [`FIRST_DELAY`] after start when the
//! inbox is enabled, then one every `interval_hours` after the last scan
//! (a manual "Scan now" counts; so does a failed timed attempt, so a missing
//! folder is not retried every tick). Settings are re-read at every tick;
//! scans never overlap (the runner coalesces), and the next due time is
//! published for the UI.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::runner::Inbox;

pub const FIRST_DELAY: Duration = Duration::from_secs(30);
pub const TICK: Duration = Duration::from_secs(30);

/// When the next scan is due: `first` until this app run attempted one,
/// then the later of the last attempt and the last scan, plus the interval.
pub fn next_due(
    first: DateTime<Utc>,
    last_attempt: Option<DateTime<Utc>>,
    last_scan: Option<DateTime<Utc>>,
    interval: Duration,
) -> DateTime<Utc> {
    let interval = chrono::Duration::from_std(interval).unwrap_or(chrono::Duration::hours(12));
    match last_attempt {
        None => first,
        Some(a) => last_scan.map_or(a, |s| s.max(a)) + interval,
    }
}

/// Run until the app closes (spawned at app start).
pub async fn run(inbox: Inbox, first_delay: Duration, tick: Duration) {
    let first = Utc::now() + chrono::Duration::from_std(first_delay).unwrap_or_default();
    let mut last_attempt: Option<DateTime<Utc>> = None;
    loop {
        if inbox.is_stopping() {
            return;
        }
        let cfg = inbox.config();
        if cfg.enabled {
            let due = next_due(first, last_attempt, inbox.last_scan(), cfg.interval);
            inbox.set_next_scan(Some(due));
            if Utc::now() >= due {
                last_attempt = Some(Utc::now());
                if let Err(e) = inbox.scan_now().await {
                    log::warn!("inbox: timed scan failed: {}", e.message);
                }
                continue;
            }
        } else {
            inbox.set_next_scan(None);
        }
        tokio::time::sleep(tick).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_times() {
        let t0 = Utc::now();
        let h12 = Duration::from_secs(12 * 3600);
        let hours = chrono::Duration::hours;
        assert_eq!(next_due(t0, None, None, h12), t0);
        // A scan from an earlier app run does not delay the first scan.
        assert_eq!(next_due(t0, None, Some(t0 - hours(1)), h12), t0);
        let a = t0 + chrono::Duration::minutes(1);
        assert_eq!(next_due(t0, Some(a), None, h12), a + hours(12));
        // A later manual scan moves the next timed one.
        assert_eq!(
            next_due(t0, Some(a), Some(a + hours(2)), h12),
            a + hours(14)
        );
        assert_eq!(
            next_due(t0, Some(a), Some(a - hours(2)), h12),
            a + hours(12)
        );
    }
}
