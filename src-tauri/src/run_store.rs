//! Run folders (docs/ARCHITECTURE.md §2): `runs/<run-id>/run.json`, per-node
//! prompt.md / events.jsonl / result.md / summary.json / messages.jsonl /
//! stderr.log, and the append-only GRAPH_LOG.md.

use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::contract::{
    AppError, AppResult, NodeMessage, NodeResult, NodeStatus, NodeSummary, RunState, RunStatus,
    RunSummary, StatusCounts, TerminalSession,
};
use crate::graph_util::is_valid_node_id;
use crate::store::{append, read_json, read_optional, write_json};

pub const RUN_FILE: &str = "run.json";
pub const PROMPT_FILE: &str = "prompt.md";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const RESULT_FILE: &str = "result.md";
pub const SUMMARY_FILE: &str = "summary.json";
pub const MESSAGES_FILE: &str = "messages.jsonl";
pub const STDERR_FILE: &str = "stderr.log";
pub const LOG_FILE: &str = "GRAPH_LOG.md";
pub const LOG_HEADER: &str = "run_id\tnode_id\tstatus\tcost_usd\tduration_ms\n";
const EVENTS_TAIL: usize = 200;
/// At most this much of events.jsonl is read (from the end) for the tail.
const EVENTS_TAIL_BYTES: u64 = 1024 * 1024;
/// result.md is read up to this size (briefs cut further to 4000 chars).
pub const RESULT_READ_CAP: u64 = 1024 * 1024;
/// Node reason for runs found `running` with no live loop (app exit or crash).
pub const INTERRUPTED: &str = "interrupted";

/// Reject ids that are not plain folder names (they come from the frontend).
pub fn check_id(kind: &str, id: &str) -> AppResult<()> {
    if is_valid_node_id(id) {
        Ok(())
    } else {
        Err(AppError::new("not_found", format!("invalid {kind} {id:?}")))
    }
}

pub fn runs_dir(dir: &Path) -> PathBuf {
    dir.join("runs")
}

pub fn run_dir(dir: &Path, run_id: &str) -> PathBuf {
    runs_dir(dir).join(run_id)
}

pub fn node_run_dir(dir: &Path, run_id: &str, node_id: &str) -> PathBuf {
    run_dir(dir, run_id).join(node_id)
}

/// The last run ids this process issued. Run ids are only unique per graph
/// folder on disk, but the run registry and the terminal sessions are keyed
/// by run id alone: two graphs started in the same second (two Inbox tips,
/// say) must not share one. Ids carry their second, so a short memory is
/// enough.
static ISSUED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const ISSUED_MAX: usize = 1000;

/// A fresh run id for `dir` (UTC timestamp, `-002`, `-003`… if that second is
/// taken, zero padded so ids sort as text); creates the folder. Never one
/// this process issued for another graph in the same second.
pub fn new_run_id(dir: &Path, now: DateTime<Utc>) -> AppResult<String> {
    let base = now.format("%Y%m%dT%H%M%SZ").to_string();
    fs::create_dir_all(runs_dir(dir))?;
    let mut issued = ISSUED.lock().unwrap_or_else(|p| p.into_inner());
    for n in 1..1000 {
        let id = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n:03}")
        };
        if issued.contains(&id) {
            continue;
        }
        match fs::create_dir(run_dir(dir, &id)) {
            Ok(()) => {
                issued.push_back(id.clone());
                while issued.len() > ISSUED_MAX {
                    issued.pop_front();
                }
                return Ok(id);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::new("io", "could not allocate a run id"))
}

pub fn write_run_state(dir: &Path, state: &RunState) -> AppResult<()> {
    write_json(&run_dir(dir, &state.run_id).join(RUN_FILE), state)
}

/// run.json of folder `folder`, checked: its `run_id` must equal the folder
/// name, and node keys that are not valid ids are dropped (the file is data
/// from disk, not trusted).
fn read_checked(path: &Path, folder: &str) -> AppResult<RunState> {
    let state: RunState = read_json(path)?;
    if state.run_id != folder {
        return Err(AppError::new(
            "invalid_input",
            format!(
                "{} names run {:?}, not {folder:?}",
                path.display(),
                state.run_id
            ),
        ));
    }
    let nodes = state
        .nodes
        .into_iter()
        .filter(|(id, _)| {
            let ok = is_valid_node_id(id);
            if !ok {
                log::warn!("{}: dropping invalid node id {id:?}", path.display());
            }
            ok
        })
        .collect();
    Ok(RunState { nodes, ..state })
}

pub fn read_run_state(dir: &Path, run_id: &str) -> AppResult<RunState> {
    check_id("run id", run_id)?;
    read_checked(&run_dir(dir, run_id).join(RUN_FILE), run_id)
}

/// Replace the session entries of run `run_id`'s run.json by `sessions`
/// (matched by sid; others kept), leaving everything else, the run's status
/// included, as it was. For sessions that changed after the run's loop
/// ended (killed later), which no longer reach run.json themselves.
pub fn record_sessions(dir: &Path, run_id: &str, sessions: &[TerminalSession]) -> AppResult<()> {
    let state = read_run_state(dir, run_id)?;
    let merged = state
        .sessions
        .iter()
        .map(|old| {
            sessions
                .iter()
                .find(|s| s.sid == old.sid)
                .unwrap_or(old)
                .clone()
        })
        .collect();
    write_run_state(
        dir,
        &RunState {
            sessions: merged,
            ..state
        },
    )
}

/// Every readable run.json, newest first. Unreadable runs, and runs whose
/// `run_id` differs from their folder name, are logged and skipped.
pub fn list_run_states(dir: &Path) -> AppResult<Vec<RunState>> {
    let entries = match fs::read_dir(runs_dir(dir)) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut runs: Vec<RunState> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join(RUN_FILE).is_file())
        .filter_map(|e| {
            let folder = e.file_name().to_string_lossy().into_owned();
            if !is_valid_node_id(&folder) {
                return None;
            }
            match read_checked(&e.path().join(RUN_FILE), &folder) {
                Ok(state) => Some(state),
                Err(err) => {
                    log::warn!("skipping run {}: {err}", e.path().display());
                    None
                }
            }
        })
        .collect();
    runs.sort_by(|a: &RunState, b| b.run_id.cmp(&a.run_id));
    Ok(runs)
}

/// A run found `running` on disk whose loop is gone (the app exited or crashed):
/// the run becomes `cancelled` and every node not yet finished becomes
/// `cancelled` with reason [`INTERRUPTED`].
pub fn interrupted(state: RunState, now: DateTime<Utc>) -> RunState {
    let nodes = state
        .nodes
        .into_iter()
        .map(|(id, n)| {
            let open = matches!(
                n.status,
                NodeStatus::Pending | NodeStatus::Queued | NodeStatus::Running
            );
            let n = if open {
                crate::contract::NodeRunState {
                    status: NodeStatus::Cancelled,
                    reason: Some(INTERRUPTED.to_string()),
                    finished_at: n.finished_at.or(Some(now)),
                    ..n
                }
            } else {
                n
            };
            (id, n)
        })
        .collect();
    RunState {
        status: RunStatus::Cancelled,
        finished_at: state.finished_at.or(Some(now)),
        nodes,
        ..state
    }
}

/// Re-read `run_id` and, if it is still `running`, rewrite it as interrupted.
/// The caller guarantees no live loop owns the run (it holds the run registry).
/// Returns true when the file was rewritten.
pub fn sweep_run(dir: &Path, run_id: &str) -> AppResult<bool> {
    let state = read_run_state(dir, run_id)?;
    if state.status != RunStatus::Running {
        return Ok(false);
    }
    log::warn!("run {run_id} was left running (app exit or crash); marking it interrupted");
    write_run_state(dir, &interrupted(state, Utc::now()))?;
    Ok(true)
}

pub fn summarize_run(state: &RunState) -> RunSummary {
    let counts = state
        .nodes
        .values()
        .fold(StatusCounts::default(), |c, n| match n.status {
            NodeStatus::Pending => StatusCounts {
                pending: c.pending + 1,
                ..c
            },
            NodeStatus::Queued => StatusCounts {
                queued: c.queued + 1,
                ..c
            },
            NodeStatus::Running => StatusCounts {
                running: c.running + 1,
                ..c
            },
            NodeStatus::Succeeded => StatusCounts {
                succeeded: c.succeeded + 1,
                ..c
            },
            NodeStatus::Failed => StatusCounts {
                failed: c.failed + 1,
                ..c
            },
            NodeStatus::Blocked => StatusCounts {
                blocked: c.blocked + 1,
                ..c
            },
            NodeStatus::Skipped => StatusCounts {
                skipped: c.skipped + 1,
                ..c
            },
            NodeStatus::Cancelled => StatusCounts {
                cancelled: c.cancelled + 1,
                ..c
            },
            NodeStatus::Waiting => StatusCounts {
                waiting: c.waiting + 1,
                ..c
            },
        });
    let costs: Vec<f64> = state.nodes.values().filter_map(|n| n.cost_usd).collect();
    RunSummary {
        run_id: state.run_id.clone(),
        started_at: state.started_at,
        finished_at: state.finished_at,
        status: state.status,
        counts,
        total_cost_usd: (!costs.is_empty()).then(|| costs.iter().sum()),
    }
}

pub fn write_summary(dir: &Path, run_id: &str, node_id: &str, s: &NodeSummary) -> AppResult<()> {
    write_json(&node_run_dir(dir, run_id, node_id).join(SUMMARY_FILE), s)
}

pub fn read_summary(dir: &Path, run_id: &str, node_id: &str) -> AppResult<Option<NodeSummary>> {
    let path = node_run_dir(dir, run_id, node_id).join(SUMMARY_FILE);
    if path.is_file() {
        read_json(&path).map(Some)
    } else {
        Ok(None)
    }
}

fn open_optional(path: &Path) -> AppResult<Option<fs::File>> {
    match fs::File::open(path) {
        Ok(f) => Ok(Some(f)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::new("io", format!("{}: {e}", path.display()))),
    }
}

/// At most `cap` bytes of a text file (lossy UTF-8), with a `[… truncated]`
/// line when cut; `None` if the file does not exist.
pub fn read_capped(path: &Path, cap: u64) -> AppResult<Option<String>> {
    let Some(file) = open_optional(path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| AppError::new("io", format!("{}: {e}", path.display())))?;
    let cut = bytes.len() as u64 > cap;
    bytes.truncate(usize::try_from(cap).unwrap_or(usize::MAX));
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(Some(if cut {
        format!("{text}\n[… truncated]")
    } else {
        text
    }))
}

/// The last `max_lines` lines of a file, reading at most `max_bytes` from its
/// end (a partial first line in that window is dropped).
pub fn read_tail_lines(path: &Path, max_lines: usize, max_bytes: u64) -> AppResult<Vec<String>> {
    let Some(mut file) = open_optional(path)? else {
        return Ok(Vec::new());
    };
    let io = |e: std::io::Error| AppError::new("io", format!("{}: {e}", path.display()));
    let len = file.metadata().map_err(io)?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).map_err(io)?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes).map_err(io)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let from = lines.len().saturating_sub(max_lines);
    Ok(lines[from..].iter().map(|l| l.to_string()).collect())
}

/// result.md capped at [`RESULT_READ_CAP`] bytes.
pub fn read_result_md(dir: &Path, run_id: &str, node_id: &str) -> AppResult<Option<String>> {
    read_capped(
        &node_run_dir(dir, run_id, node_id).join(RESULT_FILE),
        RESULT_READ_CAP,
    )
}

pub fn read_node_result(dir: &Path, run_id: &str, node_id: &str) -> AppResult<NodeResult> {
    check_id("run id", run_id)?;
    check_id("node id", node_id)?;
    if !run_dir(dir, run_id).is_dir() {
        return Err(AppError::new("not_found", format!("no run {run_id}")));
    }
    let events_tail = read_tail_lines(
        &node_run_dir(dir, run_id, node_id).join(EVENTS_FILE),
        EVENTS_TAIL,
        EVENTS_TAIL_BYTES,
    )?;
    Ok(NodeResult {
        result_md: read_result_md(dir, run_id, node_id)?,
        summary: read_summary(dir, run_id, node_id)?,
        events_tail,
        questions: crate::terminal::questions::read(&node_run_dir(dir, run_id, node_id)),
    })
}

/// One GRAPH_LOG.md line; writes the header first when the file is new.
pub fn append_log(
    dir: &Path,
    run_id: &str,
    node_id: &str,
    status: &str,
    cost_usd: Option<f64>,
    duration_ms: Option<u64>,
) -> AppResult<()> {
    let path = dir.join(LOG_FILE);
    let header = if path.exists() { "" } else { LOG_HEADER };
    let cost = cost_usd.map_or("-".to_string(), |c| format!("{c:.4}"));
    let duration = duration_ms.map_or("-".to_string(), |d| d.to_string());
    append(
        &path,
        &format!("{header}{run_id}\t{node_id}\t{status}\t{cost}\t{duration}\n"),
    )
}

pub fn append_message(dir: &Path, run_id: &str, node_id: &str, m: &NodeMessage) -> AppResult<()> {
    let line = serde_json::to_string(m)?;
    append(
        &node_run_dir(dir, run_id, node_id).join(MESSAGES_FILE),
        &format!("{line}\n"),
    )
}

pub fn list_messages(dir: &Path, run_id: &str, node_id: &str) -> AppResult<Vec<NodeMessage>> {
    check_id("run id", run_id)?;
    check_id("node id", node_id)?;
    let text = read_optional(&node_run_dir(dir, run_id, node_id).join(MESSAGES_FILE))?;
    Ok(text
        .as_deref()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{MessageRole, NodeRunState, RunStatus};
    use std::collections::BTreeMap;

    fn state(run_id: &str) -> RunState {
        let nodes: BTreeMap<String, NodeRunState> = [
            ("a", NodeStatus::Succeeded, Some(0.5)),
            ("b", NodeStatus::Failed, Some(0.25)),
            ("c", NodeStatus::Skipped, None),
            ("d", NodeStatus::Blocked, None),
        ]
        .into_iter()
        .map(|(id, status, cost_usd)| {
            let s = NodeRunState {
                status,
                cost_usd,
                ..NodeRunState::pending()
            };
            (id.to_string(), s)
        })
        .collect();
        RunState {
            run_id: run_id.into(),
            graph_dir: "/g".into(),
            started_at: "2026-11-07T10:00:00Z".parse().unwrap(),
            finished_at: None,
            status: RunStatus::Failed,
            nodes,
            mode: crate::contract::RunMode::Headless,
            sessions: Vec::new(),
        }
    }

    /// Two graph folders started in the same second never share a run id
    /// (the run registry and the terminal sessions are keyed by it).
    #[test]
    fn run_ids_are_unique_across_graph_folders_within_the_process() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let now: DateTime<Utc> = "2026-11-07T10:16:00Z".parse().unwrap();
        let ra = new_run_id(a.path(), now).unwrap();
        let rb = new_run_id(b.path(), now).unwrap();
        assert_ne!(ra, rb);
        assert!(rb.starts_with("20261107T101600Z"));
        assert!(b.path().join("runs").join(&rb).is_dir());
    }

    #[test]
    fn run_folder_layout_and_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let now: DateTime<Utc> = "2026-11-07T10:15:00Z".parse().unwrap();
        let r1 = new_run_id(dir, now).unwrap();
        let r2 = new_run_id(dir, now).unwrap();
        assert_eq!(r1, "20261107T101500Z");
        assert_eq!(r2, "20261107T101500Z-002");
        assert!(dir.join("runs").join(&r1).is_dir());

        write_run_state(dir, &state(&r1)).unwrap();
        write_run_state(dir, &state(&r2)).unwrap();
        assert_eq!(read_run_state(dir, &r1).unwrap(), state(&r1));
        let ids: Vec<String> = list_run_states(dir)
            .unwrap()
            .into_iter()
            .map(|r| r.run_id)
            .collect();
        assert_eq!(ids, vec![r2.clone(), r1.clone()]);
        assert_eq!(read_run_state(dir, "../x").unwrap_err().code, "not_found");

        let summary = summarize_run(&state(&r1));
        assert_eq!(summary.counts.succeeded, 1);
        assert_eq!(summary.counts.failed, 1);
        assert_eq!(summary.counts.skipped, 1);
        assert_eq!(summary.counts.blocked, 1);
        assert_eq!(summary.total_cost_usd, Some(0.75));

        let node = node_run_dir(dir, &r1, "a");
        fs::create_dir_all(&node).unwrap();
        fs::write(node.join(RESULT_FILE), "done").unwrap();
        let lines: String = (0..250).map(|i| format!("{{\"n\":{i}}}\n")).collect();
        fs::write(node.join(EVENTS_FILE), lines).unwrap();
        let s = NodeSummary {
            session_id: Some("s".into()),
            cost_usd: Some(0.5),
            duration_ms: Some(10),
            num_turns: Some(2),
            is_error: false,
            status: NodeStatus::Succeeded,
            permission_denials: vec!["WebSearch".into(), "Bash".into()],
        };
        write_summary(dir, &r1, "a", &s).unwrap();
        let res = read_node_result(dir, &r1, "a").unwrap();
        assert_eq!(res.result_md.as_deref(), Some("done"));
        assert_eq!(res.summary, Some(s));
        assert_eq!(res.events_tail.len(), EVENTS_TAIL);
        assert_eq!(res.events_tail.last().unwrap(), "{\"n\":249}");
        let empty = read_node_result(dir, &r1, "zz").unwrap();
        assert_eq!(empty.result_md, None);
        assert_eq!(empty.summary, None);
        assert_eq!(
            read_node_result(dir, "nope", "a").unwrap_err().code,
            "not_found"
        );
    }

    #[test]
    fn files_without_permission_denials_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let node = serde_json::json!({
            "status": "blocked", "started_at": null, "finished_at": null,
            "session_id": null, "cost_usd": null, "num_turns": null, "reason": "May I?"
        });
        let run = serde_json::json!({
            "run_id": "R1", "graph_dir": "/g", "started_at": "2026-11-07T10:00:00Z",
            "finished_at": null, "status": "failed", "nodes": { "a": node }
        });
        fs::create_dir_all(node_run_dir(dir, "R1", "a")).unwrap();
        write_json(&run_dir(dir, "R1").join(RUN_FILE), &run).unwrap();
        let read = read_run_state(dir, "R1").unwrap();
        assert_eq!(read.nodes["a"].status, NodeStatus::Blocked);
        assert!(read.nodes["a"].permission_denials.is_empty());
        let summary = serde_json::json!({
            "session_id": null, "cost_usd": null, "duration_ms": null,
            "num_turns": null, "is_error": false, "status": "failed"
        });
        write_json(&node_run_dir(dir, "R1", "a").join(SUMMARY_FILE), &summary).unwrap();
        let s = read_summary(dir, "R1", "a").unwrap().unwrap();
        assert!(s.permission_denials.is_empty());
        // And the field is written back under its contract name.
        let v = serde_json::to_value(NodeSummary {
            permission_denials: vec!["Bash".into()],
            ..s
        })
        .unwrap();
        assert_eq!(v["permission_denials"], serde_json::json!(["Bash"]));
    }

    #[test]
    fn run_json_with_foreign_run_id_or_bad_node_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(run_dir(dir, "R1")).unwrap();
        // run.json in folder R1 claiming to be run R2: skipped and refused.
        write_json(&run_dir(dir, "R1").join(RUN_FILE), &state("R2")).unwrap();
        assert!(list_run_states(dir).unwrap().is_empty());
        assert_eq!(read_run_state(dir, "R1").unwrap_err().code, "invalid_input");
        // Bad node keys are dropped.
        let mut s = state("R1");
        s.nodes
            .insert("../evil".into(), crate::contract::NodeRunState::pending());
        write_json(&run_dir(dir, "R1").join(RUN_FILE), &s).unwrap();
        let read = read_run_state(dir, "R1").unwrap();
        assert_eq!(read, state("R1"));
        assert_eq!(list_run_states(dir).unwrap(), vec![state("R1")]);
    }

    #[test]
    fn stale_running_run_is_swept_to_interrupted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::create_dir_all(run_dir(dir, "R1")).unwrap();
        let mut s = state("R1");
        s.status = RunStatus::Running;
        s.nodes.insert(
            "d".into(),
            crate::contract::NodeRunState {
                status: NodeStatus::Running,
                ..crate::contract::NodeRunState::pending()
            },
        );
        s.nodes
            .insert("e".into(), crate::contract::NodeRunState::pending());
        write_run_state(dir, &s).unwrap();
        assert!(sweep_run(dir, "R1").unwrap());
        let after = read_run_state(dir, "R1").unwrap();
        assert_eq!(after.status, RunStatus::Cancelled);
        assert!(after.finished_at.is_some());
        for id in ["d", "e"] {
            assert_eq!(after.nodes[id].status, NodeStatus::Cancelled);
            assert_eq!(after.nodes[id].reason.as_deref(), Some(INTERRUPTED));
        }
        assert_eq!(after.nodes["a"].status, NodeStatus::Succeeded);
        assert_eq!(after.nodes["b"].status, NodeStatus::Failed);
        // A finished run is left alone.
        assert!(!sweep_run(dir, "R1").unwrap());
    }

    #[test]
    fn capped_reads() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("big.md");
        fs::write(&f, "x".repeat(100)).unwrap();
        assert_eq!(read_capped(&f, 200).unwrap().unwrap(), "x".repeat(100));
        let cut = read_capped(&f, 10).unwrap().unwrap();
        assert_eq!(cut, format!("{}\n[… truncated]", "x".repeat(10)));
        assert_eq!(read_capped(&tmp.path().join("none"), 10).unwrap(), None);

        let lines: String = (0..1000).map(|i| format!("line {i:04}\n")).collect();
        fs::write(&f, &lines).unwrap();
        let tail = read_tail_lines(&f, 5, 1_000_000).unwrap();
        assert_eq!(tail.first().unwrap(), "line 0995");
        assert_eq!(tail.last().unwrap(), "line 0999");
        // A small window drops its partial first line.
        let tail = read_tail_lines(&f, 100, 25).unwrap();
        assert_eq!(tail, vec!["line 0998", "line 0999"]);
        assert!(read_tail_lines(&tmp.path().join("none"), 5, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn graph_log_has_header_once() {
        let tmp = tempfile::tempdir().unwrap();
        append_log(tmp.path(), "R1", "a", "succeeded", Some(0.0123), Some(1500)).unwrap();
        append_log(tmp.path(), "R1", "b", "skipped", None, None).unwrap();
        let log = fs::read_to_string(tmp.path().join(LOG_FILE)).unwrap();
        assert_eq!(
            log,
            format!("{LOG_HEADER}R1\ta\tsucceeded\t0.0123\t1500\nR1\tb\tskipped\t-\t-\n")
        );
    }

    #[test]
    fn messages_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let m = NodeMessage {
            role: MessageRole::User,
            text: "line 1\nline 2".into(),
            ts: "2026-11-07T10:00:00Z".parse().unwrap(),
        };
        append_message(tmp.path(), "R1", "a", &m).unwrap();
        append_message(tmp.path(), "R1", "a", &m).unwrap();
        assert_eq!(
            list_messages(tmp.path(), "R1", "a").unwrap(),
            vec![m.clone(), m]
        );
        assert!(list_messages(tmp.path(), "R1", "b").unwrap().is_empty());
    }

    #[test]
    fn recorded_sessions_replace_their_entries_and_nothing_else() {
        use crate::contract::SessionStatus;
        let tmp = tempfile::tempdir().unwrap();
        let session = |sid: &str, alive: bool| TerminalSession {
            sid: sid.into(),
            title: "T".into(),
            node_ids: vec!["a".into()],
            status: SessionStatus::Done,
            pid: alive.then_some(42),
            alive,
            current_node: None,
            harness: "claude".into(),
            pending_prompt: None,
        };
        let before = RunState {
            status: RunStatus::Succeeded,
            sessions: vec![session("s1", true), session("s2", true)],
            ..state("r1")
        };
        write_run_state(tmp.path(), &before).unwrap();
        record_sessions(tmp.path(), "r1", &[session("s1", false)]).unwrap();
        let after = read_run_state(tmp.path(), "r1").unwrap();
        assert_eq!(after.status, RunStatus::Succeeded);
        assert_eq!(after.nodes, before.nodes);
        assert_eq!(
            after.sessions,
            vec![session("s1", false), session("s2", true)]
        );
        assert!(record_sessions(tmp.path(), "gone", &[]).is_err());
    }
}
