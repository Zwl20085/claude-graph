//! Starting one queued tip: checks that need no write (folder busy, planted
//! Claude Code files, trust, agent-called mode), the claim, the generated
//! graph with the seen body, and the terminal run through the normal run API.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chrono::Utc;
use tokio::task::JoinHandle;

use crate::contract::{InboxPhase, InboxRun, RunState, RunTarget};
use crate::paths::abs;
use crate::run_loop::blocking;

use super::block::Outcome;
use super::eligible::{decide, Decision, Runner};
use super::env::Tap;
use super::folder::read;
use super::graphgen::{self, folder_name, PLANTED};
use super::ops::{Claimed, Known};
use super::runner::{Active, Inbox, Pending};
use super::tip::parse;
use super::watch::{self, Round};
use super::writeback::claim_file;

/// A started run.
pub(crate) struct Launched {
    pub round: Round,
    pub join: JoinHandle<RunState>,
    pub tap: Arc<Tap>,
}

pub(crate) enum Failure {
    /// Nothing was written; the tip stays as it is (next scan retries).
    NotStarted(String),
    /// Nothing was written; the graph folder is busy, so a re-scan is due soon.
    Busy(String),
    /// Claimed, but the run did not start: reported to the tip as failed.
    StartFailed(Box<Round>, String),
}

fn not_started(why: impl Into<String>) -> Failure {
    Failure::NotStarted(why.into())
}

/// Files an earlier agent round could have left in the graph folder that a
/// new Claude Code session there would load (instructions, MCP servers,
/// settings). `Some(reason)` names them; nothing is deleted.
pub fn planted_files(dir: &Path) -> Option<String> {
    let found: Vec<&str> = PLANTED
        .iter()
        .copied()
        .filter(|name| std::fs::symlink_metadata(dir.join(name)).is_ok())
        .collect();
    (!found.is_empty()).then(|| {
        format!(
            "{} contains {}, which a new session would load; check and remove {} by hand",
            dir.display(),
            found.join(", "),
            if found.len() == 1 { "it" } else { "them" }
        )
    })
}

/// Checks before the claim: the tip is still eligible, its graph folder is
/// free and carries no planted files, the generated graph passes the trust
/// gate as is, no agent-called run.
fn precheck(inbox: &Inbox, p: &Pending, run: &str, dir: &Path) -> Result<(), Failure> {
    let env = inbox.env();
    let (bytes, _) = read(&p.path).map_err(|e| not_started(format!("cannot read: {e}")))?;
    let text = String::from_utf8(bytes).map_err(|_| not_started("not UTF-8"))?;
    let tip = parse(&p.id, &text).map_err(|e| not_started(e.describe()))?;
    let (mut live, claimed) = {
        let s = inbox.lock();
        (s.live_runs(), s.claimed_set())
    };
    live.remove(run);
    let me = Runner {
        host: &env.host,
        live: &live,
        claimed: &claimed,
    };
    if let Decision::Skip(why) = decide(&tip, &me) {
        return Err(not_started(why));
    }
    if env.app.dir_busy(dir) {
        return Err(Failure::Busy("a run of its graph folder is active".into()));
    }
    if let Some(why) = planted_files(dir) {
        return Err(not_started(why));
    }
    let dir = graphgen::write(&inbox.graphs_root(), &p.id, &tip.title, &tip.body)
        .map_err(|e| not_started(format!("cannot write its graph: {}", e.message)))?;
    let status = env
        .app
        .trust_status(&abs(&dir))
        .map_err(|e| not_started(e.message))?;
    if !status.trusted {
        let items: Vec<String> = status.items.iter().map(|i| i.detail.clone()).collect();
        return Err(not_started(format!(
            "the generated graph is not trusted as is ({}); not bypassed",
            items.join("; ")
        )));
    }
    crate::agentmode::gui::refuse_if_agent_active(&abs(&dir)).map_err(|e| not_started(e.message))
}

fn launch(inbox: &Inbox, p: &Pending, run: &str) -> Result<Launched, Failure> {
    let env = inbox.env();
    let dir = abs(&inbox.graphs_root().join(folder_name(&p.id)));
    precheck(inbox, p, run, &dir)?;
    let who = super::ops::Claimant {
        host: &env.host,
        run,
        at: (env.clock)(),
    };
    // Recorded before the claim is written: a crash right after it leaves a
    // claim this machine knows (stale-claim rule, protocol section 9).
    inbox.record_claim(run);
    let (mut live, claimed) = {
        let s = inbox.lock();
        (s.live_runs(), s.claimed_set())
    };
    live.remove(run);
    let known = Known {
        live: &live,
        claimed: &claimed,
    };
    let Claimed {
        seen_body, title, ..
    } = claim_file(&p.path, &p.id, &who, &known).map_err(|d| not_started(d.describe()))?;
    let round = Round {
        tip_id: p.id.clone(),
        path: p.path.clone(),
        folder: p.folder.clone(),
        run: run.to_string(),
        graph_dir: dir.clone(),
        graph_run: None,
        expected_body: seen_body.clone(),
    };
    if let Err(e) = graphgen::write(&inbox.graphs_root(), &p.id, &title, &seen_body) {
        return Err(Failure::StartFailed(
            Box::new(round),
            format!("cannot write its graph: {}", e.message),
        ));
    }
    inbox.refresh_tip(&p.id, &p.path);
    let tap = Tap::new(env.emitter.clone());
    match env.app.start_by_mode(&dir, RunTarget::All, tap.clone()) {
        Ok((graph_run, join)) => Ok(Launched {
            round: Round {
                graph_run: Some(graph_run),
                ..round
            },
            join,
            tap,
        }),
        Err(e) => Err(Failure::StartFailed(
            Box::new(round),
            format!("the run did not start: {}", e.message),
        )),
    }
}

/// The slot of a tip that is starting: it carries the run id the claim will
/// use, so the tip counts as live (never re-claimed as stale) meanwhile.
pub(crate) fn placeholder(root: &Path, tip_id: &str, run: &str) -> Active {
    Active {
        run: InboxRun {
            run: run.to_string(),
            phase: InboxPhase::Working,
            graph_dir: root.join(folder_name(tip_id)).display().to_string(),
            graph_run_id: None,
            sid: None,
            started_at: Some(Utc::now()),
            note: Some("starting".into()),
        },
        superseded: Arc::new(AtomicBool::new(false)),
    }
}

/// Start `p` (its slot is already reserved by [`placeholder`]). Boxed: the
/// watcher it spawns awaits the queue pump, which awaits this.
pub(crate) fn start(
    inbox: &Inbox,
    p: Pending,
    run: String,
    superseded: Arc<AtomicBool>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
    Box::pin(start_inner(inbox, p, run, superseded))
}

async fn start_inner(inbox: &Inbox, p: Pending, run: String, superseded: Arc<AtomicBool>) {
    let ib = inbox.clone();
    let pending = p.clone();
    let r = run.clone();
    let result = blocking(move || Ok(launch(&ib, &pending, &r))).await;
    let result = result.unwrap_or_else(|e| Err(not_started(e.message)));
    let dropped = |why: &str| {
        inbox.lock().active.remove(&p.id);
        inbox.not_started(&p.id, why);
    };
    match result {
        Ok(l) => {
            {
                let mut s = inbox.lock();
                if let Some(a) = s.active.get_mut(&p.id) {
                    a.run.graph_dir = l.round.graph_dir.display().to_string();
                    a.run.graph_run_id = l.round.graph_run.clone();
                    a.run.note = Some("working".into());
                }
            }
            inbox.reset_retries();
            inbox.log(
                Some(&p.id),
                format!(
                    "started run {run} (graph run {})",
                    l.round.graph_run.as_deref().unwrap_or("-")
                ),
            );
            tokio::spawn(watch::watch(inbox.clone(), l, superseded));
        }
        Err(Failure::NotStarted(why)) => dropped(&why),
        Err(Failure::Busy(why)) => {
            dropped(&format!("{why}; retrying soon"));
            inbox.schedule_rescan();
        }
        Err(Failure::StartFailed(round, why)) => {
            let ib = inbox.clone();
            let outcome = Outcome::Failed { reason: why };
            let r = blocking(move || Ok(watch::write_round(&ib, &round, &outcome))).await;
            if let Err(e) = r {
                inbox.log(Some(&p.id), format!("write-back failed: {}", e.message));
            }
            inbox.set_phase(&p.id, InboxPhase::Finished);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planted_claude_files_are_named() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert_eq!(planted_files(d), None);
        std::fs::write(d.join("graph.json"), "{}").unwrap();
        std::fs::create_dir_all(d.join("nodes")).unwrap();
        assert_eq!(planted_files(d), None);
        for name in PLANTED {
            let sub = tempfile::tempdir().unwrap();
            if name == ".claude" {
                std::fs::create_dir(sub.path().join(name)).unwrap();
            } else {
                std::fs::write(sub.path().join(name), "x").unwrap();
            }
            let why = planted_files(sub.path()).expect(name);
            assert!(
                why.contains(name) && why.contains(&sub.path().display().to_string()),
                "{why}"
            );
        }
        std::fs::write(d.join("CLAUDE.md"), "x").unwrap();
        std::fs::write(d.join(".mcp.json"), "{}").unwrap();
        assert!(planted_files(d).unwrap().contains("CLAUDE.md, .mcp.json"));
    }
}
