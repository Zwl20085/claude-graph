//! One `claude -p` process (one turn of a node), docs/ARCHITECTURE.md §5.
//!
//! [`build_args`] makes the argument list, [`run_turn`] spawns through
//! [`ClaudeCommand::command`], writes stdin and closes it, streams stdout into
//! `events.jsonl` + `node:event`, stderr into `stderr.log` + `stderr` events,
//! kills the process tree on cancel, and [`evaluate`] decides the status.
//!
//! Lines longer than [`LINE_CAP`] are truncated and reported as `parse_error`.
//! The stream accumulator is shared with the reader task, so a pipe held open
//! by a grandchild never loses the parsed result: after the process exits with
//! a `result` seen, the reader gets [`RESULT_GRACE`] to reach EOF, then the
//! leftover tree is killed (Windows: the turn's Job Object is closed; Unix: the
//! process group) and the reader is aborted.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use chrono::Utc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::claude_bin::ClaudeCommand;
use crate::contract::{
    Model, Node, NodeEvent, NodeEventKind, NodeStatus, PermissionMode, Settings,
};
use crate::events::{truncate_chars, Emitter, ResultInfo, StreamAccumulator};
use crate::job::Job;
use crate::run_store::{EVENTS_FILE, STDERR_FILE};

/// How long to wait for the output pipes after the process exited when no
/// `result` was seen (a grandchild may still hold them open).
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// After a normal exit with a `result`, wait this long for EOF, then kill leftovers.
pub const RESULT_GRACE: Duration = Duration::from_secs(2);
/// Longest stdout/stderr line kept; longer lines are truncated (`parse_error`).
pub const LINE_CAP: usize = 1024 * 1024;
/// A deliverable counts only if written no earlier than this before the turn started.
const FRESHNESS_SLACK: Duration = Duration::from_secs(2);
const STDERR_TAIL_LINES: usize = 5;
const REASON_MAX: usize = 400;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn model_flag(node: Model, default: Model) -> Option<&'static str> {
    match if node == Model::Inherit {
        default
    } else {
        node
    } {
        Model::Sonnet => Some("sonnet"),
        Model::Opus => Some("opus"),
        Model::Haiku => Some("haiku"),
        Model::Inherit => None,
    }
}

pub fn permission_flag(node: PermissionMode, default: PermissionMode) -> Option<&'static str> {
    let mode = if node == PermissionMode::Inherit {
        default
    } else {
        node
    };
    match mode {
        PermissionMode::AcceptEdits => Some("acceptEdits"),
        PermissionMode::Default => Some("default"),
        PermissionMode::Plan => Some("plan"),
        PermissionMode::BypassPermissions => Some("bypassPermissions"),
        PermissionMode::DontAsk => Some("dontAsk"),
        PermissionMode::Inherit => None,
    }
}

/// A session to resume. `fork` adds `--fork-session` (used for
/// `continue_session_of`, so the upstream's own session is left untouched;
/// follow-up messages continue the node's session in place).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resume<'a> {
    pub session_id: &'a str,
    pub fork: bool,
}

/// `-p <instruction> --output-format stream-json --verbose [--model m]
/// [--permission-mode pm] --add-dir <graph dir> [--max-turns N]
/// [--resume id [--fork-session]] [--allowedTools t…]` (allowedTools is
/// variadic, so it goes last). The session id must already be validated.
pub fn build_args(
    node: &Node,
    settings: &Settings,
    graph_dir: &Path,
    resume: Option<Resume<'_>>,
    instruction: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        instruction.into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
    ];
    if let Some(m) = model_flag(node.model, settings.default_model) {
        args.extend(["--model".into(), m.into()]);
    }
    if let Some(p) = permission_flag(node.permission_mode, settings.default_permission_mode) {
        args.extend(["--permission-mode".into(), p.into()]);
    }
    args.extend(["--add-dir".into(), graph_dir.display().to_string()]);
    if let Some(n) = node.max_turns {
        args.extend(["--max-turns".into(), n.to_string()]);
    }
    if let Some(r) = resume.filter(|r| !r.session_id.is_empty()) {
        args.extend(["--resume".into(), r.session_id.into()]);
        if r.fork {
            args.push("--fork-session".into());
        }
    }
    let tools: Vec<String> = node
        .allowed_tools
        .iter()
        .flatten()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    if !tools.is_empty() {
        args.push("--allowedTools".into());
        args.extend(tools);
    }
    args
}

/// Everything one turn needs.
#[derive(Clone)]
pub struct TurnSpec {
    pub run_id: String,
    pub node_id: String,
    pub claude: ClaudeCommand,
    pub args: Vec<String>,
    pub stdin_text: String,
    pub cwd: PathBuf,
    /// `runs/<run>/<node>/`; events.jsonl and stderr.log are appended here.
    pub out_dir: PathBuf,
    /// Absolute paths that must exist, written during this turn, after it.
    pub required: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TurnOutcome {
    pub status: NodeStatus,
    pub reason: Option<String>,
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub num_turns: Option<u32>,
    /// From the CLI result, else wall clock.
    pub duration_ms: u64,
    pub is_error: bool,
    pub final_text: Option<String>,
}

impl TurnOutcome {
    fn failed(reason: String, started: Instant) -> Self {
        Self {
            status: NodeStatus::Failed,
            reason: Some(reason),
            session_id: None,
            cost_usd: None,
            num_turns: None,
            duration_ms: elapsed_ms(started),
            is_error: true,
            final_text: None,
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// The command line as one display string (logged; shown in test output).
pub fn display_command(spec: &TurnSpec) -> String {
    let quote = |a: String| {
        if a.contains(char::is_whitespace) {
            format!("\"{}\"", a.replace('"', "\\\""))
        } else {
            a
        }
    };
    std::iter::once(spec.claude.program.display().to_string())
        .chain(
            spec.claude
                .prefix_args
                .iter()
                .map(|a| a.to_string_lossy().into_owned()),
        )
        .chain(spec.args.iter().cloned())
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn emit(emitter: &dyn Emitter, spec: &TurnSpec, kind: NodeEventKind, text: String) {
    emitter.node_event(&NodeEvent {
        run_id: spec.run_id.clone(),
        node_id: spec.node_id.clone(),
        kind,
        text,
        ts: Utc::now(),
    });
}

async fn open_append(path: &Path) -> std::io::Result<tokio::fs::File> {
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
}

type SharedAcc = Arc<Mutex<StreamAccumulator>>;
type SharedTail = Arc<Mutex<VecDeque<String>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Run one turn. `kill` flipping to `true` kills the process tree.
pub async fn run_turn(
    spec: TurnSpec,
    emitter: Arc<dyn Emitter>,
    mut kill: watch::Receiver<bool>,
) -> TurnOutcome {
    let started = Instant::now();
    let started_wall = SystemTime::now();
    let already_killed = *kill.borrow();
    if already_killed {
        return evaluate_cancelled(started);
    }
    if !spec.cwd.is_dir() {
        let reason = format!("working directory {} does not exist", spec.cwd.display());
        return TurnOutcome::failed(reason, started);
    }
    if let Err(e) = tokio::fs::create_dir_all(&spec.out_dir).await {
        return TurnOutcome::failed(format!("io: {}: {e}", spec.out_dir.display()), started);
    }
    let command_line = display_command(&spec);
    log::info!("node {}: {command_line}", spec.node_id);
    emit(
        &*emitter,
        &spec,
        NodeEventKind::System,
        format!("spawn: {command_line}"),
    );

    let mut cmd = spec.claude.command();
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let reason = format!("spawn_failed: could not start {}: {e}", spec.claude.display);
            emit(&*emitter, &spec, NodeEventKind::Stderr, reason.clone());
            return TurnOutcome::failed(reason, started);
        }
    };
    let mut job = Job::assign(&child);
    let pid = child.id();

    let stdin = child.stdin.take();
    let text = spec.stdin_text.clone();
    let writer = tokio::spawn(async move {
        if let Some(mut stdin) = stdin {
            if let Err(e) = stdin.write_all(text.as_bytes()).await {
                log::warn!("writing the brief to claude's stdin failed: {e}");
            }
            if let Err(e) = stdin.shutdown().await {
                log::warn!("closing claude's stdin failed: {e}");
            }
        }
    });
    let acc: SharedAcc = Arc::new(Mutex::new(StreamAccumulator::new()));
    let tail: SharedTail = Arc::new(Mutex::new(VecDeque::new()));
    let mut stdout = tokio::spawn(read_stdout(
        child.stdout.take(),
        spec.clone(),
        emitter.clone(),
        acc.clone(),
    ));
    let mut stderr = tokio::spawn(read_stderr(
        child.stderr.take(),
        spec.clone(),
        emitter.clone(),
        tail.clone(),
    ));

    // A dropped kill sender (loop gone) is not a kill: only `Ok` counts.
    let exited = tokio::select! {
        status = child.wait() => Some(status.ok()),
        Ok(_) = kill.wait_for(|k| *k) => None,
    };
    let (exit, cancelled) = match exited {
        Some(status) => (status, false),
        None => {
            if let Some(pid) = pid {
                kill_tree(pid).await;
            }
            if let Err(e) = child.start_kill() {
                log::debug!("start_kill after taskkill: {e}");
            }
            (child.wait().await.ok(), true)
        }
    };
    writer.abort();
    drain(&mut stdout, &mut stderr, &acc, pid, &mut job, &spec.node_id).await;
    drop(job);
    let acc = lock(&acc).clone();
    let stderr_tail = Vec::from(lock(&tail).clone()).join("\n");
    if cancelled {
        return TurnOutcome {
            session_id: acc.session_id().map(str::to_string),
            ..evaluate_cancelled(started)
        };
    }
    let (missing, stale) = check_deliverables(&spec.required, started_wall);
    let final_text = acc.final_text();
    let code = exit.and_then(|s| s.code());
    let (status, reason) = evaluate(
        code,
        acc.result(),
        final_text.as_deref(),
        &Deliverables {
            missing: &missing,
            stale: &stale,
        },
        &stderr_tail,
    );
    let result = acc.result();
    TurnOutcome {
        status,
        reason,
        session_id: acc.session_id().map(str::to_string),
        cost_usd: result.and_then(|r| r.cost_usd),
        num_turns: result.and_then(|r| r.num_turns),
        duration_ms: result
            .and_then(|r| r.duration_ms)
            .unwrap_or_else(|| elapsed_ms(started)),
        is_error: result.map_or(true, |r| r.is_error),
        final_text,
    }
}

/// Wait for the readers to reach EOF. With a `result` seen, only
/// [`RESULT_GRACE`]; else up to [`DRAIN_TIMEOUT`]. Then kill whatever still
/// holds the pipes and abort the readers (their shared state is kept).
async fn drain(
    stdout: &mut JoinHandle<()>,
    stderr: &mut JoinHandle<()>,
    acc: &SharedAcc,
    pid: Option<u32>,
    job: &mut Option<Job>,
    node_id: &str,
) {
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    let mut done = tokio::time::timeout(RESULT_GRACE, &mut *stdout)
        .await
        .is_ok();
    if !done && lock(acc).result().is_none() {
        let rest = deadline.saturating_duration_since(Instant::now());
        done = tokio::time::timeout(rest, &mut *stdout).await.is_ok();
    }
    if !done {
        log::warn!("node {node_id}: stdout still open after the process exited; killing leftovers");
        kill_leftovers(pid, job).await;
        if tokio::time::timeout(Duration::from_millis(500), &mut *stdout)
            .await
            .is_err()
        {
            stdout.abort();
        }
    }
    if tokio::time::timeout(Duration::from_secs(1), &mut *stderr)
        .await
        .is_err()
    {
        kill_leftovers(pid, job).await;
        stderr.abort();
    }
}

/// Kill processes left behind by a finished child: close its Job Object
/// (Windows) or kill its process group (Unix).
async fn kill_leftovers(pid: Option<u32>, job: &mut Option<Job>) {
    drop(job.take());
    #[cfg(unix)]
    if let Some(pid) = pid {
        kill_tree(pid).await;
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn evaluate_cancelled(started: Instant) -> TurnOutcome {
    TurnOutcome {
        status: NodeStatus::Cancelled,
        reason: Some("cancelled by user".into()),
        ..TurnOutcome::failed(String::new(), started)
    }
}

/// `(missing, stale)` required deliverables: absent, or last written before
/// `since` minus [`FRESHNESS_SLACK`] (left over from an earlier run).
pub fn check_deliverables(required: &[PathBuf], since: SystemTime) -> (Vec<String>, Vec<String>) {
    let floor = since.checked_sub(FRESHNESS_SLACK).unwrap_or(since);
    let mut missing = Vec::new();
    let mut stale = Vec::new();
    for p in required {
        let shown = p.display().to_string();
        match std::fs::metadata(p) {
            Ok(m) if m.is_file() => {
                let fresh = m.modified().map(|t| t >= floor).unwrap_or(true);
                if !fresh {
                    stale.push(shown);
                }
            }
            _ => missing.push(shown),
        }
    }
    (missing, stale)
}

enum Line {
    Eof,
    Read { truncated: bool },
}

/// Read one line into `buf` (cleared first), keeping at most `cap` bytes and
/// discarding the rest of an overlong line.
async fn read_line_capped<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<Line> {
    buf.clear();
    let mut truncated = false;
    let mut any = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if any {
                Line::Read { truncated }
            } else {
                Line::Eof
            });
        }
        any = true;
        let (len, end) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        let take = len.min(cap.saturating_sub(buf.len()));
        buf.extend_from_slice(&available[..take]);
        truncated |= take < len;
        reader.consume(len);
        if end {
            return Ok(Line::Read { truncated });
        }
    }
}

async fn read_stdout(
    stdout: Option<tokio::process::ChildStdout>,
    spec: TurnSpec,
    emitter: Arc<dyn Emitter>,
    acc: SharedAcc,
) {
    let Some(stdout) = stdout else { return };
    let mut file = open_append(&spec.out_dir.join(EVENTS_FILE)).await.ok();
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        let truncated = match read_line_capped(&mut reader, &mut buf, LINE_CAP).await {
            Ok(Line::Eof) => break,
            Ok(Line::Read { truncated }) => truncated,
            Err(e) => {
                log::warn!("reading claude stdout failed: {e}");
                break;
            }
        };
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(f) = file.as_mut() {
            if let Err(e) = f.write_all(format!("{line}\n").as_bytes()).await {
                log::warn!("appending to {EVENTS_FILE} failed: {e}");
            }
        }
        let events = if truncated {
            vec![(
                NodeEventKind::ParseError,
                format!(
                    "line longer than {LINE_CAP} bytes truncated: {}",
                    truncate_chars(line, 200)
                ),
            )]
        } else {
            lock(&acc).feed(line)
        };
        for (kind, text) in events {
            emit(&*emitter, &spec, kind, text);
        }
    }
    if let Some(mut f) = file {
        if let Err(e) = f.flush().await {
            log::warn!("flushing {EVENTS_FILE} failed: {e}");
        }
    }
}

/// Keeps the last few stderr lines in `tail` (for failure reasons).
async fn read_stderr(
    stderr: Option<tokio::process::ChildStderr>,
    spec: TurnSpec,
    emitter: Arc<dyn Emitter>,
    tail: SharedTail,
) {
    let Some(stderr) = stderr else { return };
    let mut file = open_append(&spec.out_dir.join(STDERR_FILE)).await.ok();
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    while let Ok(Line::Read { truncated }) = read_line_capped(&mut reader, &mut buf, LINE_CAP).await
    {
        let line = String::from_utf8_lossy(&buf).trim_end().to_string();
        if let Some(f) = file.as_mut() {
            if let Err(e) = f.write_all(format!("{line}\n").as_bytes()).await {
                log::warn!("appending to {STDERR_FILE} failed: {e}");
            }
        }
        if truncated {
            emit(
                &*emitter,
                &spec,
                NodeEventKind::ParseError,
                format!("stderr line longer than {LINE_CAP} bytes truncated"),
            );
        }
        if !line.trim().is_empty() {
            emit(&*emitter, &spec, NodeEventKind::Stderr, line.clone());
            let mut t = lock(&tail);
            if t.len() == STDERR_TAIL_LINES {
                t.pop_front();
            }
            t.push_back(line);
        }
    }
    if let Some(mut f) = file {
        if let Err(e) = f.flush().await {
            log::warn!("flushing {STDERR_FILE} failed: {e}");
        }
    }
}

/// Kill the process and all its children.
pub async fn kill_tree(pid: u32) {
    #[cfg(windows)]
    let result = {
        let mut cmd = tokio::process::Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        cmd.status().await
    };
    #[cfg(unix)]
    let result = tokio::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    if let Err(e) = result {
        log::warn!("killing process tree {pid} failed: {e}");
    }
}

/// The status line the agent ended with: `Some(true)` DONE, `Some(false)` BLOCKED
/// (with the text after it), `None` if absent. The last status line wins.
pub fn final_status(text: &str) -> Option<(bool, String)> {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().enumerate().rev().find_map(|(i, l)| {
        let t = l.trim();
        if t.eq_ignore_ascii_case("## Status: DONE") {
            Some((true, String::new()))
        } else if t.to_ascii_uppercase().starts_with("## STATUS: BLOCKED") {
            let after = lines[i + 1..].join(" ");
            Some((false, after.trim().to_string()))
        } else {
            None
        }
    })
}

/// Required deliverables that are absent, or present but not written this turn.
#[derive(Debug, Clone, Copy, Default)]
pub struct Deliverables<'a> {
    pub missing: &'a [String],
    pub stale: &'a [String],
}

/// Decide a finished (not cancelled) turn's status and reason.
pub fn evaluate(
    exit_code: Option<i32>,
    result: Option<&ResultInfo>,
    final_text: Option<&str>,
    deliverables: &Deliverables<'_>,
    stderr_tail: &str,
) -> (NodeStatus, Option<String>) {
    let failed = |r: String| (NodeStatus::Failed, Some(truncate_chars(&r, REASON_MAX)));
    let code_text = exit_code.map_or("unknown".to_string(), |c| c.to_string());
    let Some(result) = result else {
        let tail = if stderr_tail.is_empty() {
            String::new()
        } else {
            format!(": {stderr_tail}")
        };
        return failed(format!(
            "claude exited with code {code_text} without a result{tail}"
        ));
    };
    if result.is_error {
        let detail = if result.errors.is_empty() {
            result.result.clone().unwrap_or_default()
        } else {
            result.errors.join("; ")
        };
        return failed(format!("claude reported {}: {detail}", result.subtype));
    }
    if exit_code != Some(0) {
        return failed(format!("claude exited with code {code_text}"));
    }
    match final_text.and_then(final_status) {
        Some((false, why)) => failed(format!("agent reported ## Status: BLOCKED: {why}")),
        None => failed("final message has no '## Status: DONE' line".to_string()),
        Some((true, _)) if !deliverables.missing.is_empty() => failed(format!(
            "missing deliverables: {}",
            deliverables.missing.join(", ")
        )),
        Some((true, _)) if !deliverables.stale.is_empty() => failed(
            deliverables
                .stale
                .iter()
                .map(|p| format!("deliverable {p} was not written in this run"))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        Some((true, _)) => (NodeStatus::Succeeded, None),
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
