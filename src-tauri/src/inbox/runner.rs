//! The inbox runner: scans, the wait queue, starting runs and the shared
//! state the UI shows. One scan at a time (a request during a scan is
//! coalesced into one more pass); at most `max_parallel` sessions working;
//! queued tips are claimed only when they actually start. Persistence (last
//! scan, recorded run ids) is in [`super::persist`]; retries (delayed
//! re-scans, outcomes not yet written) in [`super::retry`].

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::contract::{
    AppError, AppResult, InboxLogLine, InboxPhase, InboxRun, InboxSkipped, InboxStatus, InboxTip,
};
use crate::run_loop::blocking;

use super::eligible::Runner;
use super::env::{InboxEnv, RunConfig};
use super::graphgen::inbox_root;
use super::host::new_inbox_run_id;
use super::persist::{self, Persisted};
use super::retry::Unwritten;
use super::scan::{scan_folder, see};

/// Most log lines kept.
const LOG_MAX: usize = 200;
/// How long superseding waits for the earlier run to end.
const SUPERSEDE_WAIT: Duration = Duration::from_secs(20);

/// A tip waiting for a slot.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub id: String,
    pub path: PathBuf,
    /// The tips folder of the settings it was queued under.
    pub folder: PathBuf,
}

/// A claimed tip (starting, working, waiting for the user, or finished
/// with its session left open). A starting tip already carries its run id.
pub(crate) struct Active {
    pub run: InboxRun,
    pub superseded: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct Shared {
    pub tips: Vec<InboxTip>,
    pub skipped: Vec<InboxSkipped>,
    pub other_tips: u32,
    pub last_scan: Option<DateTime<Utc>>,
    pub next_scan: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub queue: VecDeque<Pending>,
    pub active: BTreeMap<String, Active>,
    /// Why a tip did not start (not claimed), by tip id, until the next scan.
    pub not_started: BTreeMap<String, String>,
    /// Outcomes whose write-back failed for a reason that may pass.
    pub unwritten: BTreeMap<String, Unwritten>,
    /// Graph runs whose ended sessions are not in their run.json yet (the
    /// run's loop or a session was still alive): run id -> (tip id, tries).
    pub unrecorded: BTreeMap<String, (String, u32)>,
    /// Run ids this machine claimed (oldest first, bounded, persisted).
    pub claimed: VecDeque<String>,
    pub log: VecDeque<InboxLogLine>,
}

impl Shared {
    pub fn working(&self) -> usize {
        self.active
            .values()
            .filter(|a| a.run.phase == InboxPhase::Working)
            .count()
    }

    /// Protocol run ids whose session this app still drives (a starting tip
    /// included: its id is set before the claim is written).
    pub fn live_runs(&self) -> HashSet<String> {
        self.active
            .values()
            .filter(|a| matches!(a.run.phase, InboxPhase::Working | InboxPhase::Question))
            .map(|a| a.run.run.clone())
            .filter(|r| !r.is_empty())
            .collect()
    }

    pub fn claimed_set(&self) -> HashSet<String> {
        self.claimed.iter().cloned().collect()
    }
}

pub(crate) struct Inner {
    pub env: InboxEnv,
    pub shared: Mutex<Shared>,
    scanning: AtomicBool,
    rescan: AtomicBool,
    pub stopping: AtomicBool,
    /// Delayed re-scans scheduled in a row (bounded, see [`super::retry`]).
    pub retries: AtomicU32,
    pub retry_pending: AtomicBool,
}

/// Tauri-managed inbox (cheap to clone).
#[derive(Clone)]
pub struct Inbox(pub(crate) Arc<Inner>);

/// Clears the scanning flag when a scan ends, also by panic.
struct ScanGuard<'a>(&'a AtomicBool);

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Inbox {
    pub fn new(env: InboxEnv) -> Self {
        let p = persist::load(env.app.data_dir());
        Self(Arc::new(Inner {
            env,
            shared: Mutex::new(Shared {
                last_scan: p.last_scan,
                claimed: p.claimed.into_iter().collect(),
                ..Shared::default()
            }),
            scanning: AtomicBool::new(false),
            rescan: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            retries: AtomicU32::new(0),
            retry_pending: AtomicBool::new(false),
        }))
    }

    pub(crate) fn env(&self) -> &InboxEnv {
        &self.0.env
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Shared> {
        self.0.shared.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn config(&self) -> RunConfig {
        (self.0.env.config)()
    }

    pub fn host(&self) -> &str {
        &self.0.env.host
    }

    /// `<data dir>/inbox`, where the generated graphs live.
    pub fn graphs_root(&self) -> PathBuf {
        inbox_root(self.0.env.app.data_dir())
    }

    pub fn is_stopping(&self) -> bool {
        self.0.stopping.load(Ordering::SeqCst)
    }

    /// App exit: no more starts or write-backs. Tips left `running` are
    /// picked up by the stale-claim rule on the next scan.
    pub fn shutdown(&self) {
        self.0.stopping.store(true, Ordering::SeqCst);
        self.lock().queue.clear();
    }

    pub fn last_scan(&self) -> Option<DateTime<Utc>> {
        self.lock().last_scan
    }

    pub fn set_next_scan(&self, at: Option<DateTime<Utc>>) {
        let changed = {
            let mut s = self.lock();
            let changed = s.next_scan != at;
            s.next_scan = at;
            changed
        };
        if changed {
            self.emit();
        }
    }

    pub(crate) fn log(&self, tip: Option<&str>, text: impl Into<String>) {
        let text = text.into();
        log::info!(
            "inbox: {}{text}",
            tip.map(|t| format!("{t}: ")).unwrap_or_default()
        );
        let mut s = self.lock();
        s.log.push_back(InboxLogLine {
            ts: Utc::now(),
            tip: tip.map(str::to_string),
            text,
        });
        while s.log.len() > LOG_MAX {
            s.log.pop_front();
        }
    }

    /// Record a run id this machine is about to claim with, and save it.
    pub(crate) fn record_claim(&self, run: &str) {
        let p = {
            let mut s = self.lock();
            persist::remember(&mut s.claimed, run);
            Persisted {
                last_scan: s.last_scan,
                claimed: s.claimed.iter().cloned().collect(),
            }
        };
        persist::save(self.0.env.app.data_dir(), &p);
    }

    fn persist(&self) {
        let p = {
            let s = self.lock();
            Persisted {
                last_scan: s.last_scan,
                claimed: s.claimed.iter().cloned().collect(),
            }
        };
        persist::save(self.0.env.app.data_dir(), &p);
    }

    pub fn status(&self) -> InboxStatus {
        let cfg = self.config();
        let s = self.lock();
        let tips = s
            .tips
            .iter()
            .map(|t| {
                let run = s.active.get(&t.id).map(|a| a.run.clone()).or_else(|| {
                    s.queue.iter().find(|p| p.id == t.id).map(|_| InboxRun {
                        run: String::new(),
                        phase: InboxPhase::Waiting,
                        graph_dir: String::new(),
                        graph_run_id: None,
                        sid: None,
                        started_at: None,
                        note: None,
                    })
                });
                let decision = if let Some(u) = s.unwritten.get(&t.id) {
                    format!("outcome not written yet: {}", u.reason)
                } else if let Some(why) = s.not_started.get(&t.id) {
                    format!("not started: {why}")
                } else {
                    t.decision.clone()
                };
                InboxTip {
                    run,
                    decision,
                    ..t.clone()
                }
            })
            .collect();
        let folder = cfg.folder.display().to_string();
        InboxStatus {
            enabled: cfg.enabled,
            suggested_folder: if folder.is_empty() {
                (self.0.env.suggest)()
            } else {
                None
            },
            folder,
            host: self.0.env.host.clone(),
            scanning: self.0.scanning.load(Ordering::SeqCst),
            last_scan: s.last_scan,
            next_scan: s.next_scan,
            last_error: s.last_error.clone(),
            tips,
            skipped: s.skipped.clone(),
            other_tips: s.other_tips,
            log: s.log.iter().cloned().collect(),
        }
    }

    pub(crate) fn emit(&self) {
        let status = self.status();
        self.0.env.sink.update(&status);
    }

    /// Publish the status (after a settings change).
    pub fn emit_status(&self) {
        self.emit();
    }

    /// Re-read one tip after this app wrote it, so the list shows its new state.
    pub(crate) fn refresh_tip(&self, id: &str, path: &Path) {
        let (live, claimed) = {
            let s = self.lock();
            (s.live_runs(), s.claimed_set())
        };
        let me = Runner {
            host: &self.0.env.host,
            live: &live,
            claimed: &claimed,
        };
        match see(id, path, &me) {
            Ok(Some(seen)) => {
                let mut s = self.lock();
                match s.tips.iter_mut().find(|t| t.id == id) {
                    Some(t) => *t = seen.view,
                    None => s.tips.push(seen.view),
                }
            }
            Ok(None) => self.log(Some(id), "re-read: no longer an agent tip"),
            Err(skipped) => self.log(Some(id), format!("re-read failed: {}", skipped.reason)),
        }
    }

    /// Scan now (the button, the timer). `invalid_input` when the inbox is off.
    pub async fn scan_now(&self) -> AppResult<InboxStatus> {
        let cfg = self.config();
        if !cfg.enabled {
            return Err(AppError::new(
                "invalid_input",
                "the inbox is off; enable it and choose the tips folder first",
            ));
        }
        if self.is_stopping() {
            return Err(AppError::new("cancelled", "the app is closing"));
        }
        if self.0.scanning.swap(true, Ordering::SeqCst) {
            self.0.rescan.store(true, Ordering::SeqCst);
            return Ok(self.status());
        }
        let result = {
            let _guard = ScanGuard(&self.0.scanning);
            self.emit();
            loop {
                let r = self.scan_once(&cfg).await;
                if !self.0.rescan.swap(false, Ordering::SeqCst) {
                    break r;
                }
            }
        };
        if let Err(e) = &result {
            self.lock().last_error = Some(e.message.clone());
            self.log(None, format!("scan failed: {}", e.message));
        }
        self.pump().await;
        self.emit();
        result.map(|_| self.status())
    }

    async fn scan_once(&self, cfg: &RunConfig) -> AppResult<()> {
        let folder = cfg.folder.clone();
        self.close_finished().await;
        self.prune_rounds().await;
        check_layout(&folder, &self.graphs_root())?;
        self.retry_unwritten().await;
        let host = self.0.env.host.clone();
        let (live, claimed) = {
            let s = self.lock();
            (s.live_runs(), s.claimed_set())
        };
        let view = blocking(move || {
            scan_folder(
                &folder,
                &Runner {
                    host: &host,
                    live: &live,
                    claimed: &claimed,
                },
            )
        })
        .await?;
        let mut supersede = Vec::new();
        {
            let mut s = self.lock();
            s.not_started.clear();
            for seen in view.tips.iter().filter(|t| t.view.eligible) {
                let id = &seen.view.id;
                if s.queue.iter().any(|p| &p.id == id) || s.unwritten.contains_key(id) {
                    continue;
                }
                if let Some(a) = s.active.get(id) {
                    if a.run.phase == InboxPhase::Working {
                        continue;
                    }
                    supersede.push(id.clone());
                }
                s.queue.push_back(Pending {
                    id: id.clone(),
                    path: seen.path.clone(),
                    folder: cfg.folder.clone(),
                });
            }
            s.tips = view.tips.iter().map(|t| t.view.clone()).collect();
            s.skipped = view.skipped.clone();
            s.other_tips = view.other_tips;
            s.last_scan = Some(Utc::now());
            s.last_error = None;
        }
        for id in supersede {
            if !self.supersede(&id).await {
                self.lock().queue.retain(|p| p.id != id);
                self.not_started(&id, "the earlier session is still closing; retrying soon");
                self.schedule_rescan();
            }
        }
        self.persist();
        let queued = self.lock().queue.len();
        self.log(
            None,
            format!(
                "scanned: {} agent tip(s), {} to run, {} unreadable",
                view.tips.len(),
                queued,
                view.skipped.len()
            ),
        );
        Ok(())
    }

    /// Start of a scan: every session whose round ended (done or failed)
    /// before this scan began is killed and its entry dropped. It was left
    /// open after its write-back so the owner could look at it; closing it
    /// now keeps one idle `claude` process per handled tip from piling up.
    /// Rounds whose outcome is still waiting to be written are kept until it
    /// is; sessions waiting at a question are never touched here. Waits (at
    /// most [`SUPERSEDE_WAIT`] in all) for the sessions to end, so a tip that
    /// runs again in this scan finds its graph folder free. Returns the tips
    /// closed.
    pub(crate) async fn close_finished(&self) -> Vec<String> {
        self.retry_unrecorded();
        let closed: Vec<(String, Active)> = {
            let mut s = self.lock();
            let ids: Vec<String> = s
                .active
                .iter()
                .filter(|(id, a)| {
                    a.run.phase == InboxPhase::Finished && !s.unwritten.contains_key(*id)
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| s.active.remove(&id).map(|a| (id, a)))
                .collect()
        };
        let app = &self.0.env.app;
        for (id, a) in &closed {
            a.superseded.store(true, Ordering::SeqCst);
            let Some(run) = a.run.graph_run_id.as_deref() else {
                continue;
            };
            if let Err(e) = app.cancel_run(run) {
                if e.code != "not_found" {
                    self.log(Some(id), format!("closing run {run}: {}", e.message));
                }
            }
            app.terminals().kill_run(run);
            self.close_windows(id, run);
            self.log(Some(id), format!("closed the finished session (run {run})"));
        }
        let busy = || {
            closed.iter().any(|(_, a)| {
                app.dir_busy(Path::new(&a.run.graph_dir))
                    || a.run.graph_run_id.as_deref().is_some_and(|run| {
                        app.terminals()
                            .all_of(run)
                            .iter()
                            .any(|s| s.is_task_alive())
                    })
            })
        };
        let until = Instant::now() + SUPERSEDE_WAIT;
        while busy() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for (id, a) in &closed {
            if let Some(run) = a.run.graph_run_id.as_deref() {
                self.record_ended(id, run);
            }
        }
        closed.into_iter().map(|(id, _)| id).collect()
    }

    pub(crate) fn not_started(&self, id: &str, why: &str) {
        self.lock()
            .not_started
            .insert(id.to_string(), why.to_string());
        self.log(Some(id), format!("not started: {why}"));
    }

    /// The tip's earlier run (waiting for the user, or finished with its
    /// session open) is cancelled and closed before a fresh run starts.
    /// `false` when it is still busy after [`SUPERSEDE_WAIT`].
    ///
    /// The tip's [`Active`] entry stays in place until the session is closed
    /// (so the queued fresh run cannot be taken meanwhile, see
    /// [`Self::take_next`]); when it does not close, the entry is kept, so a
    /// later scan closes it again instead of losing track of it.
    async fn supersede(&self, id: &str) -> bool {
        let Some((flag, graph_run, graph_dir)) = self.lock().active.get(id).map(|a| {
            (
                a.superseded.clone(),
                a.run.graph_run_id.clone(),
                a.run.graph_dir.clone(),
            )
        }) else {
            return true;
        };
        flag.store(true, Ordering::SeqCst);
        let app = self.0.env.app.clone();
        let Some(run) = graph_run.as_deref() else {
            self.lock().active.remove(id);
            return true;
        };
        if let Err(e) = app.cancel_run(run) {
            if e.code != "not_found" {
                self.log(Some(id), format!("cancelling run {run}: {}", e.message));
            }
        }
        app.terminals().kill_run(run);
        let dir = PathBuf::from(&graph_dir);
        let until = Instant::now() + SUPERSEDE_WAIT;
        let busy = || {
            app.dir_busy(&dir)
                || app
                    .terminals()
                    .all_of(run)
                    .iter()
                    .any(|s| s.is_task_alive())
        };
        while busy() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let closed = !busy();
        if closed {
            self.record_ended(id, run);
            self.lock().active.remove(id);
        }
        self.log(
            Some(id),
            if closed {
                format!("closed the earlier session (run {run})")
            } else {
                format!("the earlier session (run {run}) is still closing")
            },
        );
        closed
    }

    /// Take the next pending tip and reserve its slot in one step: its
    /// placeholder (with the run id it will claim with) is inserted under the
    /// same lock, so concurrent pumps never exceed `max_parallel`. Tips queued
    /// under another tips folder than the current one are dropped. A tip that
    /// still has an [`Active`] entry (its earlier session is being
    /// superseded) is never taken: it stays queued until [`Self::supersede`]
    /// has closed that session and removed the entry.
    fn take_next(&self, cfg: &RunConfig) -> Option<(Pending, String, Arc<AtomicBool>)> {
        let mut dropped = Vec::new();
        let next = {
            let mut s = self.lock();
            loop {
                if !cfg.enabled {
                    // Turned off meanwhile: queued tips (never claimed) are dropped.
                    s.queue.clear();
                    break None;
                }
                if s.working() >= cfg.max_parallel {
                    break None;
                }
                // A tip whose earlier session is still tracked (being
                // superseded, or not closed yet) waits in the queue: taking it
                // would replace that session's bookkeeping, and the session
                // would never be closed.
                let Some(at) = s.queue.iter().position(|p| !s.active.contains_key(&p.id)) else {
                    break None;
                };
                let Some(p) = s.queue.remove(at) else {
                    break None;
                };
                if p.folder != cfg.folder {
                    dropped.push(p.id);
                    continue;
                }
                let run = new_inbox_run_id((self.0.env.clock)());
                let active = super::start::placeholder(&self.graphs_root(), &p.id, &run);
                let flag = active.superseded.clone();
                s.active.insert(p.id.clone(), active);
                break Some((p, run, flag));
            }
        };
        for id in dropped {
            self.log(
                Some(&id),
                "dropped from the queue: the tips folder setting changed",
            );
        }
        next
    }

    /// Start queued tips while slots are free.
    pub(crate) async fn pump(&self) {
        loop {
            if self.is_stopping() {
                return;
            }
            let cfg = self.config();
            let Some((p, run, flag)) = self.take_next(&cfg) else {
                return;
            };
            super::start::start(self, p, run, flag).await;
            self.emit();
        }
    }

    /// Tests: wait until nothing is queued or working.
    pub async fn wait_idle(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        loop {
            let busy = {
                let s = self.lock();
                !s.queue.is_empty() || s.working() > 0
            } || self.0.scanning.load(Ordering::SeqCst);
            if !busy {
                return true;
            }
            if Instant::now() >= until {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// The generated graphs must not live in (or contain) the tips folder.
fn check_layout(folder: &Path, graphs: &Path) -> AppResult<()> {
    let folder = crate::paths::abs(folder);
    let graphs = crate::paths::abs(graphs);
    if graphs.starts_with(&folder) || folder.starts_with(&graphs) {
        return Err(AppError::new(
            "invalid_input",
            format!(
                "the tips folder {} and the inbox graphs {} overlap",
                folder.display(),
                graphs.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
