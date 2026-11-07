//! Startup options: `claude-graph <graph-dir> [--run]`, or the env fallback
//! `CLAUDE_GRAPH_OPEN=<dir>` / `CLAUDE_GRAPH_AUTORUN=1` (handy under `tauri dev`).
//! See docs/ARCHITECTURE.md "Launching".

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::contract::LaunchOptions;

pub const ENV_OPEN: &str = "CLAUDE_GRAPH_OPEN";
pub const ENV_AUTORUN: &str = "CLAUDE_GRAPH_AUTORUN";
pub const FLAG_RUN: &str = "--run";

fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// `args` are the arguments after the executable. The first argument that is
/// not a flag is the graph folder; `--run` asks for "run all"; `--` and other
/// flags are ignored. Env fills in what argv leaves unset. `autorun` is false
/// when no folder was given.
pub fn parse_launch(args: &[String], env: &HashMap<String, String>) -> LaunchOptions {
    let mut open_dir = None;
    let mut run_flag = false;
    for arg in args {
        if arg == FLAG_RUN {
            run_flag = true;
        } else if !arg.starts_with('-') && open_dir.is_none() {
            open_dir = non_empty(Some(arg));
        }
    }
    let open_dir = open_dir.or_else(|| non_empty(env.get(ENV_OPEN)));
    let autorun = open_dir.is_some() && (run_flag || truthy(env.get(ENV_AUTORUN)));
    LaunchOptions { open_dir, autorun }
}

/// [`parse_launch`] over this process's argv and environment. A relative
/// folder is made absolute against the current directory, so the recent list
/// and run records hold a stable path.
pub fn from_process() -> LaunchOptions {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env: HashMap<String, String> = [ENV_OPEN, ENV_AUTORUN]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    let parsed = parse_launch(&args, &env);
    LaunchOptions {
        open_dir: parsed.open_dir.map(absolute),
        ..parsed
    }
}

/// Hidden mode flag: `claude-graph --notify <hooks.log> [json]` (Codex's
/// `notify` program, docs/ARCHITECTURE.md "Harness profiles").
pub const FLAG_NOTIFY: &str = "--notify";
const NOTIFY_FILE: &str = "hooks.log";

/// `Some((file, payload))` when `args` (after the executable) ask for the
/// notify mode. The payload is whatever came last (Codex appends one JSON
/// argument); it may hold any characters.
pub fn notify_args(args: &[String]) -> Option<(PathBuf, Option<&str>)> {
    match args {
        [flag, file, rest @ ..] if flag == FLAG_NOTIFY => {
            Some((PathBuf::from(file), rest.last().map(String::as_str)))
        }
        _ => None,
    }
}

/// The hooks.log line for a notify payload: `stop`, plus the payload
/// re-serialized as one line of compact JSON when it is a JSON object (so
/// the session can tell its own turn from Codex's side turns). Anything else
/// is dropped unread.
pub fn notify_line(payload: Option<&str>) -> String {
    let object = payload
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .filter(serde_json::Value::is_object)
        .and_then(|v| serde_json::to_string(&v).ok());
    match object {
        Some(json) => format!("stop {json}\n"),
        None => "stop\n".to_string(),
    }
}

/// Append [`notify_line`] to `file`, which must be a session's `hooks.log`.
pub fn notify(file: &Path, payload: Option<&str>) -> std::io::Result<()> {
    use std::io::Write as _;
    if file.file_name().and_then(|n| n.to_str()) != Some(NOTIFY_FILE) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("--notify only appends to a {NOTIFY_FILE}"),
        ));
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    f.write_all(notify_line(payload).as_bytes())
}

/// The notify mode, if argv asks for it: returns the exit code to use.
pub fn notify_from_process() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (file, payload) = notify_args(&args)?;
    Some(match notify(&file, payload) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("claude-graph --notify: {e}");
            1
        }
    })
}

fn absolute(dir: String) -> String {
    std::path::absolute(&dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn opts(dir: Option<&str>, autorun: bool) -> LaunchOptions {
        LaunchOptions {
            open_dir: dir.map(str::to_string),
            autorun,
        }
    }

    #[test]
    fn nothing_given() {
        assert_eq!(parse_launch(&[], &env(&[])), opts(None, false));
    }

    #[test]
    fn dir_and_run_flag_in_any_order() {
        let e = env(&[]);
        assert_eq!(
            parse_launch(&args(&["C:/g"]), &e),
            opts(Some("C:/g"), false)
        );
        assert_eq!(
            parse_launch(&args(&["C:/g", "--run"]), &e),
            opts(Some("C:/g"), true)
        );
        assert_eq!(
            parse_launch(&args(&["--run", "C:/g"]), &e),
            opts(Some("C:/g"), true)
        );
    }

    #[test]
    fn double_dash_and_unknown_flags_are_ignored_first_dir_wins() {
        let a = args(&["--", "--verbose", "/a b/g1", "/g2", "--run"]);
        assert_eq!(parse_launch(&a, &env(&[])), opts(Some("/a b/g1"), true));
    }

    #[test]
    fn run_without_dir_does_not_autorun() {
        assert_eq!(
            parse_launch(&args(&["--run"]), &env(&[])),
            opts(None, false)
        );
        let e = env(&[(ENV_AUTORUN, "1")]);
        assert_eq!(parse_launch(&[], &e), opts(None, false));
    }

    #[test]
    fn env_fallback() {
        let e = env(&[(ENV_OPEN, " /env/g "), (ENV_AUTORUN, "1")]);
        assert_eq!(parse_launch(&[], &e), opts(Some("/env/g"), true));
        let e = env(&[(ENV_OPEN, "/env/g"), (ENV_AUTORUN, "TRUE")]);
        assert_eq!(parse_launch(&args(&["--"]), &e), opts(Some("/env/g"), true));
        let e = env(&[(ENV_OPEN, "/env/g"), (ENV_AUTORUN, "0")]);
        assert_eq!(parse_launch(&[], &e), opts(Some("/env/g"), false));
        let e = env(&[(ENV_OPEN, "   ")]);
        assert_eq!(parse_launch(&[], &e), opts(None, false));
    }

    #[test]
    fn argv_beats_env_and_env_autorun_applies_to_argv_dir() {
        let e = env(&[(ENV_OPEN, "/env/g"), (ENV_AUTORUN, "1")]);
        assert_eq!(
            parse_launch(&args(&["/argv/g"]), &e),
            opts(Some("/argv/g"), true)
        );
    }

    #[test]
    fn notify_mode_parses_any_payload() {
        assert_eq!(notify_args(&args(&["/g", "--notify", "x"])), None);
        assert_eq!(notify_args(&args(&["--notify"])), None);
        let only_file = args(&["--notify", "C:/s/hooks.log"]);
        let (file, payload) = notify_args(&only_file).unwrap();
        assert_eq!((file, payload), (PathBuf::from("C:/s/hooks.log"), None));
        let hostile = "{\"a\":\"\\\"&|<>\"}\r\n& del /q *| echo pwned";
        let a = args(&["--notify", "/s/hooks.log", hostile]);
        assert_eq!(notify_args(&a).unwrap().1, Some(hostile));
        assert_eq!(notify_line(Some(hostile)), "stop\n");
        assert_eq!(notify_line(None), "stop\n");
        assert_eq!(notify_line(Some("[1]")), "stop\n");
        let line = notify_line(Some("{\"input-messages\":[\"a\\nb & c | d \\\"q\\\"\"]}"));
        assert_eq!(line.lines().count(), 1);
        assert!(line.starts_with("stop {") && line.contains("a\\nb & c | d"));
    }

    #[test]
    fn notify_appends_only_to_hooks_log() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("hooks.log");
        notify(&log, Some("not json \n with lines")).unwrap();
        notify(&log, None).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "stop\nstop\n");
        let other = tmp.path().join("evil.txt");
        assert!(notify(&other, None).is_err());
        assert!(!other.exists());
    }

    #[test]
    fn serialises_snake_case() {
        let json = serde_json::to_string(&opts(Some("/g"), true)).unwrap();
        assert_eq!(json, r#"{"open_dir":"/g","autorun":true}"#);
    }
}
