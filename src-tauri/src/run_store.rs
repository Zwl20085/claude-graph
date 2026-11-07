//! Run folders (docs/ARCHITECTURE.md §2): `runs/<run-id>/run.json`, per-node
//! prompt.md / events.jsonl / result.md / summary.json / messages.jsonl /
//! stderr.log, and the append-only GRAPH_LOG.md.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::contract::{
    AppError, AppResult, NodeMessage, NodeResult, NodeStatus, NodeSummary, RunState, RunSummary,
    StatusCounts,
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

/// A fresh run id (UTC timestamp, `-2`, `-3`… if that second is taken); creates the folder.
pub fn new_run_id(dir: &Path, now: DateTime<Utc>) -> AppResult<String> {
    let base = now.format("%Y%m%dT%H%M%SZ").to_string();
    fs::create_dir_all(runs_dir(dir))?;
    for n in 1..1000 {
        let id = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        match fs::create_dir(run_dir(dir, &id)) {
            Ok(()) => return Ok(id),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(AppError::new("io", "could not allocate a run id"))
}

pub fn write_run_state(dir: &Path, state: &RunState) -> AppResult<()> {
    write_json(&run_dir(dir, &state.run_id).join(RUN_FILE), state)
}

pub fn read_run_state(dir: &Path, run_id: &str) -> AppResult<RunState> {
    check_id("run id", run_id)?;
    read_json(&run_dir(dir, run_id).join(RUN_FILE))
}

/// Every readable run.json, newest first. Unreadable runs are logged and skipped.
pub fn list_run_states(dir: &Path) -> AppResult<Vec<RunState>> {
    let entries = match fs::read_dir(runs_dir(dir)) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut runs: Vec<RunState> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join(RUN_FILE).is_file())
        .filter_map(|e| match read_json(&e.path().join(RUN_FILE)) {
            Ok(state) => Some(state),
            Err(err) => {
                log::warn!("skipping unreadable run {}: {err}", e.path().display());
                None
            }
        })
        .collect();
    runs.sort_by(|a: &RunState, b| b.run_id.cmp(&a.run_id));
    Ok(runs)
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
            NodeStatus::Skipped => StatusCounts {
                skipped: c.skipped + 1,
                ..c
            },
            NodeStatus::Cancelled => StatusCounts {
                cancelled: c.cancelled + 1,
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

pub fn read_result_md(dir: &Path, run_id: &str, node_id: &str) -> AppResult<Option<String>> {
    read_optional(&node_run_dir(dir, run_id, node_id).join(RESULT_FILE))
}

pub fn read_node_result(dir: &Path, run_id: &str, node_id: &str) -> AppResult<NodeResult> {
    check_id("run id", run_id)?;
    check_id("node id", node_id)?;
    if !run_dir(dir, run_id).is_dir() {
        return Err(AppError::new("not_found", format!("no run {run_id}")));
    }
    let events = read_optional(&node_run_dir(dir, run_id, node_id).join(EVENTS_FILE))?;
    let lines: Vec<String> = events
        .as_deref()
        .unwrap_or("")
        .lines()
        .map(str::to_string)
        .collect();
    let events_tail = lines[lines.len().saturating_sub(EVENTS_TAIL)..].to_vec();
    Ok(NodeResult {
        result_md: read_result_md(dir, run_id, node_id)?,
        summary: read_summary(dir, run_id, node_id)?,
        events_tail,
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
        }
    }

    #[test]
    fn run_folder_layout_and_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let now: DateTime<Utc> = "2026-11-07T10:15:00Z".parse().unwrap();
        let r1 = new_run_id(dir, now).unwrap();
        let r2 = new_run_id(dir, now).unwrap();
        assert_eq!(r1, "20261107T101500Z");
        assert_eq!(r2, "20261107T101500Z-2");
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
}
