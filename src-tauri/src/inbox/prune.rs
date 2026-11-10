//! Deleting old round folders (owner decision 2026-11-10): a round folder
//! under `<data>/inbox/rounds/<tip>/` is deleted [`KEEP_FOR`] after its round
//! finished, at scan time.
//!
//! Only folders the app made are touched: a direct child of a tip folder
//! whose name is a runner run id (`YYYYMMDD-HHMMSS-xxxx`). Never a round that
//! is in use (its tip's entry is active, a run of the app is in it, or a
//! session in it is alive). Never through a link: a link or junction as a
//! tip or round folder, or anywhere inside a round folder, is refused and
//! reported, and nothing of that round is deleted. The 0.7.0 folders
//! (`<data>/inbox/<slug>-<hash>/`, one per tip) are never looked at.
//!
//! Age: from the round's finish time, which is the newest modification time
//! of its run records (`runs/*/run.json`, written by the app when the run's
//! state changes). An agent could touch those files while its session runs,
//! but only up to the moment the session ends; a time before the round's
//! start (from its folder name) or after now is not believed, and the start
//! is used instead, so the age can never be pushed into the future.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::NaiveDateTime;

use crate::run_api::AppState;
use crate::run_loop::blocking;

use super::fsguard::{is_link, plain_tree, require_plain_dir};
use super::graphgen::{inbox_root, tip_dir, ROUNDS_DIR};
use super::runner::Inbox;

/// How long a finished round's folder is kept.
pub const KEEP_FOR: Duration = Duration::from_secs(15 * 24 * 3600);

/// Whether a tip folder (a round of the tip is starting or open) or a round
/// folder is in use: never deleted then.
pub trait Live {
    fn in_use(&self, dir: &Path) -> bool;
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PruneReport {
    pub deleted: Vec<PathBuf>,
    /// `<path>: why` for everything left alone that should be looked at.
    pub refused: Vec<String>,
}

/// The start time of a round from its folder name (a runner run id).
fn started(name: &str) -> Option<NaiveDateTime> {
    let b = name.as_bytes();
    let shape = b.len() == 20
        && b[8] == b'-'
        && b[15] == b'-'
        && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit)
        && b[16..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c));
    if !shape {
        return None;
    }
    NaiveDateTime::parse_from_str(&name[..15], "%Y%m%d-%H%M%S").ok()
}

fn local(t: SystemTime) -> NaiveDateTime {
    chrono::DateTime::<chrono::Local>::from(t).naive_local()
}

/// The round's finish time (see the module doc).
fn finished(dir: &Path, start: NaiveDateTime, now: NaiveDateTime) -> NaiveDateTime {
    let newest = std::fs::read_dir(dir.join("runs"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| std::fs::symlink_metadata(e.path().join("run.json")).ok())
        .filter(|m| m.is_file() && !is_link(m))
        .filter_map(|m| m.modified().ok())
        .map(local)
        .max();
    newest.filter(|t| *t >= start && *t <= now).unwrap_or(start)
}

/// Prune one round folder `dir` (a direct child of a tip folder).
fn prune_round(
    dir: &Path,
    now: NaiveDateTime,
    keep: Duration,
    live: &dyn Live,
    r: &mut PruneReport,
) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return;
    };
    if is_link(&meta) {
        r.refused
            .push(format!("{} is a link or junction", dir.display()));
        return;
    }
    let name = dir.file_name().map(|n| n.to_string_lossy().to_string());
    let Some(start) = name.as_deref().and_then(started) else {
        return; // not a folder the app made
    };
    if !meta.is_dir() || live.in_use(dir) {
        return;
    }
    let age = now - finished(dir, start, now);
    if age.to_std().map_or(true, |a| a < keep) {
        return;
    }
    if let Err(why) = plain_tree(dir) {
        r.refused.push(why);
        return;
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => r.deleted.push(dir.to_path_buf()),
        Err(e) => r.refused.push(format!("{}: {e}", dir.display())),
    }
}

fn children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// Delete the round folders under `<data>/inbox/rounds/` that finished at
/// least `keep` before `now` (local time) and are not in use, then each tip
/// folder left empty. `data` is the canonical app data dir. Never fails:
/// what could not be done is in the report.
pub fn prune(data: &Path, now: NaiveDateTime, keep: Duration, live: &dyn Live) -> PruneReport {
    let mut r = PruneReport::default();
    let inbox = inbox_root(data);
    let rounds = inbox.join(ROUNDS_DIR);
    if std::fs::symlink_metadata(&rounds).is_err() {
        return r;
    }
    if let Err(why) = require_plain_dir(&inbox).and_then(|()| require_plain_dir(&rounds)) {
        r.refused.push(why);
        return r;
    }
    for tip in children(&rounds) {
        match std::fs::symlink_metadata(&tip) {
            Ok(m) if is_link(&m) => {
                r.refused
                    .push(format!("{} is a link or junction", tip.display()));
                continue;
            }
            Ok(m) if m.is_dir() => {}
            _ => continue,
        }
        // A tip with a round starting or open: its folder is left as it is
        // (a starting round creates its folder in it).
        if live.in_use(&tip) {
            continue;
        }
        for round in children(&tip) {
            prune_round(&round, now, keep, live, &mut r);
        }
        if children(&tip).is_empty() {
            if let Err(e) = std::fs::remove_dir(&tip) {
                r.refused.push(format!("{}: {e}", tip.display()));
            }
        }
    }
    r
}

/// In use for the app: the tip folders of tips with an Inbox entry (a round
/// starting, working, asking, or finished with its session not yet closed),
/// a folder a run of the app owns, a folder with a live session.
struct InboxLive {
    tips: HashSet<PathBuf>,
    app: AppState,
}

impl Live for InboxLive {
    fn in_use(&self, dir: &Path) -> bool {
        self.tips.contains(dir)
            || dir.parent().is_some_and(|p| self.tips.contains(p))
            || self.app.dir_busy(dir)
            || !self.app.terminals().live_runs_of(dir, "").is_empty()
    }
}

impl Inbox {
    /// Scan time: delete old round folders ([`prune`]); everything is
    /// logged, nothing fails the scan.
    pub(crate) async fn prune_rounds(&self) {
        let app = self.env().app.clone();
        let data = crate::paths::abs(app.data_dir());
        let root = inbox_root(&data);
        let tips = self
            .lock()
            .active
            .keys()
            .map(|id| tip_dir(&root, id))
            .collect();
        let now = (self.env().clock)();
        let live = InboxLive { tips, app };
        let report = blocking(move || Ok(prune(&data, now, KEEP_FOR, &live))).await;
        match report {
            Ok(r) => {
                for d in &r.deleted {
                    self.log(
                        None,
                        format!("deleted the old round folder {}", d.display()),
                    );
                }
                for why in &r.refused {
                    self.log(None, format!("round folder left alone: {why}"));
                }
            }
            Err(e) => self.log(
                None,
                format!("deleting old round folders failed: {}", e.message),
            ),
        }
    }
}

#[cfg(test)]
#[path = "prune_tests.rs"]
mod tests;
