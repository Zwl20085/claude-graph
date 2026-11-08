//! Agent runs on disk: `runs/<run>/run.json` (a [`RunState`] plus
//! `"engine": "agent"` and an `agent` object), a lock file so concurrent
//! calls (two CLI processes, parallel subagents) never interleave a
//! read-modify-write, and the run log `agent_log.jsonl` the GUI shows.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::{AppError, AppResult, RunState, RunStatus};
use crate::contract_agent::{AgentMeta, AGENT_ENGINE};
use crate::graph_util::is_valid_node_id;
use crate::paths::existing_dir;
use crate::run_store::{check_id, run_dir, runs_dir, RUN_FILE};
use crate::store::{append, read_optional, write_json, GRAPH_FILE};

/// The run log: one [`LogEntry`] per line.
pub const LOG_FILE: &str = "agent_log.jsonl";
const LOCK_FILE: &str = ".agent.lock";
const OPEN_LOCK_FILE: &str = ".agent-open.lock";
/// A lock older than this is left over from a killed process.
const LOCK_STALE: Duration = Duration::from_secs(30);
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// How many of the newest run folders are searched for an agent run.
const SCAN_MAX: usize = 50;

/// An agent run as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRun {
    pub state: RunState,
    pub meta: AgentMeta,
}

/// One line of the run log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: DateTime<Utc>,
    #[serde(default)]
    pub node_id: Option<String>,
    /// `note`, `handout`, `report`, `run`.
    pub kind: String,
    pub text: String,
}

/// The graph folder named by an agent: absolute (CLI callers may pass a
/// relative path, resolved against the current directory), local (no UNC
/// or device path), existing, canonical, holding a graph.json.
pub fn graph_dir(dir: &str, allow_relative: bool) -> AppResult<PathBuf> {
    let trimmed = dir.trim();
    let given = PathBuf::from(trimmed);
    let absolute = if allow_relative && !trimmed.is_empty() && given.is_relative() {
        std::path::absolute(&given).map_err(|e| AppError::new("invalid_input", e.to_string()))?
    } else {
        given
    };
    let dir = existing_dir(&absolute.to_string_lossy())?;
    if !dir.join(GRAPH_FILE).is_file() {
        return Err(AppError::new(
            "not_found",
            format!(
                "{} has no {GRAPH_FILE}; it is not a Claude Graph folder",
                dir.display()
            ),
        ));
    }
    Ok(dir)
}

fn parse_run(text: &str, folder: &str) -> AppResult<Option<AgentRun>> {
    let value: Value = serde_json::from_str(text)?;
    if value.get("engine").and_then(Value::as_str) != Some(AGENT_ENGINE) {
        return Ok(None);
    }
    let meta: AgentMeta =
        serde_json::from_value(value.get("agent").cloned().unwrap_or_default())
            .map_err(|e| AppError::new("json", format!("run {folder}: agent: {e}")))?;
    let state: RunState = serde_json::from_value(value)?;
    if state.run_id != folder {
        return Err(AppError::new(
            "invalid_input",
            format!("run.json of {folder} names run {:?}", state.run_id),
        ));
    }
    let nodes = state
        .nodes
        .into_iter()
        .filter(|(id, _)| is_valid_node_id(id))
        .collect();
    Ok(Some(AgentRun {
        state: RunState { nodes, ..state },
        meta,
    }))
}

/// The agent run `run_id` of `dir`; `None` when the run exists but another
/// engine wrote it; `not_found` when there is no such run.
pub fn read(dir: &Path, run_id: &str) -> AppResult<Option<AgentRun>> {
    check_id("run id", run_id)?;
    let path = run_dir(dir, run_id).join(RUN_FILE);
    let text = read_optional(&path)?
        .ok_or_else(|| AppError::new("not_found", format!("no run {run_id} in this folder")))?;
    parse_run(&text, run_id)
}

/// [`read`], but a run of another engine is `invalid_input`.
pub fn read_agent(dir: &Path, run_id: &str) -> AppResult<AgentRun> {
    read(dir, run_id)?.ok_or_else(|| {
        AppError::new(
            "invalid_input",
            format!("run {run_id} was started from the app, not by an agent"),
        )
    })
}

pub fn write(dir: &Path, run: &AgentRun) -> AppResult<()> {
    let mut value = serde_json::to_value(&run.state)?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("engine".into(), Value::String(AGENT_ENGINE.into()));
        obj.insert("agent".into(), serde_json::to_value(&run.meta)?);
    }
    write_json(&run_dir(dir, &run.state.run_id).join(RUN_FILE), &value)
}

/// Run folder names of `dir`, newest first (at most [`SCAN_MAX`]).
fn run_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(runs_dir(dir)) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|id| is_valid_node_id(id))
        .collect();
    ids.sort_by(|a, b| b.cmp(a));
    ids.truncate(SCAN_MAX);
    ids
}

/// Every readable run.json among the newest runs: (id, agent run or `None`
/// for another engine, status).
fn scan(dir: &Path) -> Vec<(String, Option<AgentRun>, Option<RunStatus>)> {
    run_ids(dir)
        .into_iter()
        .filter_map(|id| {
            let text = read_optional(&run_dir(dir, &id).join(RUN_FILE)).ok()??;
            let status = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| serde_json::from_value(v.get("status")?.clone()).ok());
            let run = parse_run(&text, &id).ok()?;
            Some((id, run, status))
        })
        .collect()
}

/// The newest agent run of `dir`, finished or not (stops at the first one).
pub fn latest(dir: &Path) -> Option<AgentRun> {
    run_ids(dir).into_iter().find_map(|id| {
        let text = read_optional(&run_dir(dir, &id).join(RUN_FILE)).ok()??;
        parse_run(&text, &id).ok()?
    })
}

/// The newest agent run still `running`.
pub fn active(dir: &Path) -> Option<AgentRun> {
    scan(dir)
        .into_iter()
        .filter_map(|(_, run, _)| run)
        .find(|r| r.state.status == RunStatus::Running)
}

/// Whether run `run_id` of `dir` is an agent run (the stale-run sweep leaves those alone).
pub fn is_agent_run(dir: &Path, run_id: &str) -> bool {
    matches!(read(dir, run_id), Ok(Some(_)))
}

/// A run started from the app that run.json says is still `running`.
pub fn running_app_run(dir: &Path) -> Option<String> {
    scan(dir)
        .into_iter()
        .find(|(_, run, status)| run.is_none() && *status == Some(RunStatus::Running))
        .map(|(id, _, _)| id)
}

/// An exclusive lock file, removed on drop.
pub struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_file(&self.path) {
            eprintln!("claude-graph: removing {}: {e}", self.path.display());
        }
    }
}

fn is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age > LOCK_STALE)
}

fn acquire(path: PathBuf) -> AppResult<Lock> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(Lock { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if is_stale(&path) {
                    let _ = fs::remove_file(&path);
                    continue;
                }
                if Instant::now() > deadline {
                    return Err(AppError::new(
                        "busy",
                        format!("{} is held by another call; try again", path.display()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Lock run `run_id` for one read-modify-write.
pub fn lock_run(dir: &Path, run_id: &str) -> AppResult<Lock> {
    check_id("run id", run_id)?;
    acquire(run_dir(dir, run_id).join(LOCK_FILE))
}

/// Lock the folder while a run is chosen or created.
pub fn lock_open(dir: &Path) -> AppResult<Lock> {
    acquire(runs_dir(dir).join(OPEN_LOCK_FILE))
}

/// Append one line to the run log (failures are reported, not fatal).
pub fn log(dir: &Path, run_id: &str, node_id: Option<&str>, kind: &str, text: &str) {
    let entry = LogEntry {
        ts: Utc::now(),
        node_id: node_id.map(str::to_string),
        kind: kind.to_string(),
        text: text.to_string(),
    };
    let line = match serde_json::to_string(&entry) {
        Ok(l) => l,
        Err(e) => return eprintln!("claude-graph: run log: {e}"),
    };
    if let Err(e) = append(&run_dir(dir, run_id).join(LOG_FILE), &format!("{line}\n")) {
        eprintln!("claude-graph: run log: {e}");
    }
}

/// Log entries from byte `from` on, and the offset after the last whole line.
pub fn read_log(dir: &Path, run_id: &str, from: u64) -> (Vec<LogEntry>, u64) {
    let path = run_dir(dir, run_id).join(LOG_FILE);
    let Ok(bytes) = fs::read(&path) else {
        return (Vec::new(), from);
    };
    let start = usize::try_from(from).unwrap_or(usize::MAX).min(bytes.len());
    let tail = &bytes[start..];
    let whole = tail.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let entries = String::from_utf8_lossy(&tail[..whole])
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    (entries, (start + whole) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{NodeRunState, RunMode};

    fn run(id: &str, status: RunStatus) -> AgentRun {
        AgentRun {
            state: RunState {
                run_id: id.into(),
                graph_dir: "/g".into(),
                started_at: Utc::now(),
                finished_at: None,
                status,
                nodes: [("a".to_string(), NodeRunState::pending())].into(),
                mode: RunMode::Terminal,
                sessions: Vec::new(),
            },
            meta: AgentMeta {
                client: "Claude Code".into(),
                channel: "mcp".into(),
                last_seen: Utc::now(),
            },
        }
    }

    #[test]
    fn round_trip_and_engine_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let r = run("20260101T000000Z", RunStatus::Running);
        write(tmp.path(), &r).unwrap();
        let raw = fs::read_to_string(run_dir(tmp.path(), &r.state.run_id).join(RUN_FILE)).unwrap();
        assert!(raw.contains("\"engine\": \"agent\""));
        assert_eq!(read(tmp.path(), &r.state.run_id).unwrap(), Some(r.clone()));
        assert!(is_agent_run(tmp.path(), &r.state.run_id));
        // The app's own reader still loads it.
        let st = crate::run_store::read_run_state(tmp.path(), &r.state.run_id).unwrap();
        assert_eq!(st, r.state);
        assert_eq!(active(tmp.path()).unwrap().state.run_id, r.state.run_id);
    }

    #[test]
    fn app_runs_are_not_agent_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let r = run("20260101T000000Z", RunStatus::Running);
        crate::run_store::write_run_state(tmp.path(), &r.state).unwrap();
        assert_eq!(read(tmp.path(), &r.state.run_id).unwrap(), None);
        assert!(read_agent(tmp.path(), &r.state.run_id).is_err());
        assert_eq!(
            running_app_run(tmp.path()).as_deref(),
            Some("20260101T000000Z")
        );
        assert!(active(tmp.path()).is_none());
        assert!(read(tmp.path(), "nope").is_err());
        assert!(read(tmp.path(), "../x").is_err());
    }

    #[test]
    fn lock_is_exclusive_and_released() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(run_dir(tmp.path(), "r1")).unwrap();
        let first = lock_run(tmp.path(), "r1").unwrap();
        let path = run_dir(tmp.path(), "r1").join(LOCK_FILE);
        assert!(path.exists());
        drop(first);
        assert!(!path.exists());
        let _again = lock_run(tmp.path(), "r1").unwrap();
    }

    #[test]
    fn log_reads_whole_lines_from_an_offset() {
        let tmp = tempfile::tempdir().unwrap();
        log(tmp.path(), "r1", None, "note", "one");
        let (a, off) = read_log(tmp.path(), "r1", 0);
        assert_eq!(a.len(), 1);
        log(tmp.path(), "r1", Some("n"), "note", "two");
        let (b, off2) = read_log(tmp.path(), "r1", off);
        assert_eq!((b.len(), b[0].node_id.as_deref()), (1, Some("n")));
        assert!(off2 > off);
        assert!(read_log(tmp.path(), "r1", off2).0.is_empty());
    }

    #[test]
    fn graph_dir_needs_graph_json_and_rejects_unc() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().to_string_lossy().to_string();
        assert_eq!(graph_dir(&d, false).unwrap_err().code, "not_found");
        fs::write(tmp.path().join(GRAPH_FILE), "{}").unwrap();
        assert!(graph_dir(&d, false).is_ok());
        assert!(graph_dir(r"\\server\share\g", false).is_err());
        assert!(graph_dir("relative/g", false).is_err());
        assert!(graph_dir("", true).is_err());
    }
}
