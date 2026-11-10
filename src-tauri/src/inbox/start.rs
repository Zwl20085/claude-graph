//! Starting one queued tip: checks that write nothing (tip still eligible,
//! no folder of the tip busy, the round's folders per [`super::round`],
//! trust), the claim, then the round folder made and the generated graph
//! with the seen body written, a last check of the folder, and the terminal
//! run through the normal run API.

use std::path::{Path, PathBuf};
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
use super::graphgen::{self, round_dir, PLANTED};
use super::ops::{Claimed, Known};
use super::round;
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
/// outside `top` is refused (its parents could not be checked).
pub fn planted_files(dir: &Path, top: &Path) -> Option<String> {
    if !dir.starts_with(top) {
        return Some(format!(
            "{} is outside the app data folder {}; not run",
            dir.display(),
            top.display()
        ));
    }
    let levels: Vec<&Path> = dir.ancestors().take_while(|a| a.starts_with(top)).collect();
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

/// One run per tip: no graph folder of the tip (an earlier round's, or the
/// single folder of earlier app versions) has a run in this app's registry
/// (in memory, [`crate::run_api::AppState::dir_busy`]). Each round runs in a
/// fresh folder, so the "one run per graph folder" rule alone would no
/// longer keep two rounds of a tip apart. On-disk run records (agent-called
/// runs' `run.json`) are not consulted: an agent could forge one in its own
/// round folder to block its tip forever, and no legitimate agent-called run
/// uses an Inbox round folder.
fn check_rounds_free(inbox: &Inbox, tip_id: &str) -> Result<(), Failure> {
    for d in graphgen::round_dirs(&inbox.graphs_root(), tip_id) {
        let d = abs(&d);
        if inbox.env().app.dir_busy(&d) {
            return Err(Failure::Busy(format!(
                "a run of its graph folder {} is active",
                d.display()
            )));
        }
    }
    Ok(())
}

/// The trust gate on the graph as it will be written, before anything is
/// written: the round folder does not exist yet, so nothing in it can add
/// elevated settings; [`crate::run_api::AppState::start_by_mode`] checks the
/// written folder again.
fn trust_check(inbox: &Inbox, dir: &Path, title: &str, body: &str) -> Result<(), Failure> {
    let graph = graphgen::build(title, body);
    let status = crate::trust::status(inbox.env().app.data_dir(), dir, &graph);
    if status.trusted {
        return Ok(());
    }
    let items: Vec<String> = status.items.iter().map(|i| i.detail.clone()).collect();
    Err(not_started(format!(
        "the generated graph is not trusted as is ({}); not bypassed",
        items.join("; ")
    )))
}

/// Checks before the claim, writing nothing: the tip is still eligible, no
/// folder of the tip is busy, the new round's folder does not exist, the
/// levels above it are plain folders without planted files, and the graph
/// passes the trust gate as is. Returns the round folder's path.
fn precheck(inbox: &Inbox, p: &Pending, run: &str, data: &Path) -> Result<PathBuf, Failure> {
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
    check_rounds_free(inbox, &p.id)?;
    let dir = round::check_before_claim(data, &p.id, run).map_err(not_started)?;
    trust_check(inbox, &dir, &tip.title, &tip.body)?;
    Ok(dir)
}

/// The round folder after the claim: created, the graph written, and the
/// folder checked once more as the last step before the run starts (S3:
/// nothing the Inbox does comes between this and `start_by_mode`).
fn make_round(data: &Path, p: &Pending, run: &str, title: &str, body: &str) -> Result<(), String> {
    let dir = round::create(data, &p.id, run)?;
    graphgen::write_into(&dir, title, body)
        .map_err(|e| format!("cannot write its graph: {}", e.message))?;
    round::recheck(data, &dir)
}

fn launch(inbox: &Inbox, p: &Pending, run: &str) -> Result<Launched, Failure> {
    let env = inbox.env();
    // The canonical data dir: every round path is built from it, so the
    // planted-files check and the session see the same folders.
    let data = abs(env.app.data_dir());
    let dir = precheck(inbox, p, run, &data)?;
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
        prompt: None,
    };
    inbox.refresh_tip(&p.id, &p.path);
    if let Err(why) = make_round(&data, p, run, &title, &seen_body) {
        return Err(Failure::StartFailed(
            Box::new(round),
            format!("not started: {why}"),
        ));
    }
    let tap = Tap::new(env.emitter.clone());
    match env.app.start_by_mode(&dir, RunTarget::All, tap.clone()) {
        Ok((graph_run, join)) => Ok(Launched {
            round: Round {
                graph_run: Some(graph_run),
                // The node prompt written into this round's graph.json just
                // above, which the run loaded and built the brief from.
                prompt: Some(graphgen::task_text(&title, &seen_body)),
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
            graph_dir: round_dir(root, tip_id, run).display().to_string(),
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

    /// S1 (d): a folder outside the data dir is refused, not checked alone.
    #[test]
    fn a_folder_outside_the_data_dir_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("claude-graph");
        let outside = tmp.path().join("elsewhere").join("round");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let why = planted_files(&outside, &data).expect("refused");
        assert!(why.contains("is outside the app data folder"), "{why}");
    }
}
