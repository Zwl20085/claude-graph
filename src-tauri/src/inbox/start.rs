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
use super::writeback::{claim_file, is_agent_file};

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

/// Files an earlier agent round could have left that a new Claude Code
/// session in `dir` would load (instructions, MCP servers, settings, a git
/// root that carries them): looked for in `dir` and every parent of it up to
/// and including `top` (the app's data dir), never above it. `Some(reason)`
/// names each folder and what it contains; nothing is deleted. A `dir`
/// outside `top` is checked on its own.
pub fn planted_files(dir: &Path, top: &Path) -> Option<String> {
    let levels: Vec<&Path> = if dir.starts_with(top) {
        dir.ancestors().take_while(|a| a.starts_with(top)).collect()
    } else {
        vec![dir]
    };
    let mut count = 0;
    let found: Vec<String> = levels
        .iter()
        .filter_map(|d| {
            let names: Vec<&str> = PLANTED
                .iter()
                .copied()
                .filter(|name| std::fs::symlink_metadata(d.join(name)).is_ok())
                .collect();
            count += names.len();
            (!names.is_empty()).then(|| format!("{} contains {}", d.display(), names.join(", ")))
        })
        .collect();
    (!found.is_empty()).then(|| {
        format!(
            "{}, which a new session would load; check and remove {} by hand",
            found.join("; "),
            if count == 1 { "it" } else { "them" }
        )
    })
}

/// Checks before the claim: the tip is still eligible, its graph folder is
/// free and carries no planted files, the generated graph passes the trust
/// gate as is, no agent-called run.
fn precheck(inbox: &Inbox, p: &Pending, run: &str, dir: &Path) -> Result<(), Failure> {
    let env = inbox.env();
    // `kind` from the bounded frontmatter read first: a tip that stopped
    // being an agent tip is not read further (protocol section 9).
    is_agent_file(&p.path).map_err(|d| not_started(d.describe()))?;
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
    if let Some(why) = planted_files(dir, &abs(env.app.data_dir())) {
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
        assert_eq!(planted_files(d, d), None);
        std::fs::write(d.join("graph.json"), "{}").unwrap();
        std::fs::create_dir_all(d.join("nodes")).unwrap();
        assert_eq!(planted_files(d, d), None);
        for name in PLANTED {
            let sub = tempfile::tempdir().unwrap();
            if name == ".claude" || name == ".git" {
                std::fs::create_dir(sub.path().join(name)).unwrap();
            } else {
                std::fs::write(sub.path().join(name), "x").unwrap();
            }
            let why = planted_files(sub.path(), sub.path()).expect(name);
            assert!(
                why.contains(name) && why.contains(&sub.path().display().to_string()),
                "{why}"
            );
        }
        std::fs::write(d.join("CLAUDE.md"), "x").unwrap();
        std::fs::write(d.join(".mcp.json"), "{}").unwrap();
        assert!(planted_files(d, d)
            .unwrap()
            .contains("CLAUDE.md, .mcp.json"));
    }

    /// Review finding 3: `<data>/inbox/<graph>/` is checked at every level
    /// up to the data dir, never above it.
    #[test]
    fn planted_files_in_parent_folders_up_to_the_data_dir_are_named() {
        let tmp = tempfile::tempdir().unwrap();
        let above = tmp.path();
        let data = above.join("claude-graph");
        let inbox = data.join("inbox");
        let graph = inbox.join("tip-0123abcd");
        std::fs::create_dir_all(&graph).unwrap();
        // Above the data dir: never looked at.
        std::fs::write(above.join("CLAUDE.md"), "x").unwrap();
        std::fs::create_dir(above.join(".git")).unwrap();
        assert_eq!(planted_files(&graph, &data), None);
        for (level, name) in [
            (&graph, ".git"),
            (&inbox, "CLAUDE.md"),
            (&inbox, ".git"),
            (&data, "CLAUDE.local.md"),
            (&data, ".claude"),
            (&data, ".mcp.json"),
        ] {
            let path = level.join(name);
            if name.starts_with(".g") || name == ".claude" {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, "x").unwrap();
            }
            let why = planted_files(&graph, &data).expect(name);
            assert!(
                why.contains(&format!("{} contains {name}", level.display())),
                "{why}"
            );
            assert!(
                !why.contains(&format!("{} contains", above.display())),
                "{why}"
            );
            if path.is_dir() {
                std::fs::remove_dir(&path).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
        }
        // Several levels at once are all named.
        std::fs::write(graph.join("CLAUDE.md"), "x").unwrap();
        std::fs::write(data.join("CLAUDE.md"), "x").unwrap();
        let why = planted_files(&graph, &data).unwrap();
        assert!(why.contains(&format!("{} contains CLAUDE.md", graph.display())));
        assert!(why.contains(&format!("{} contains CLAUDE.md", data.display())));
        assert!(why.ends_with("check and remove them by hand"), "{why}");
    }
}
