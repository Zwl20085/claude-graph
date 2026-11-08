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
/// `notify` program), or `claude-graph --notify <hooks.log> --event <word>
/// --stdin` (Claude Code hook commands: the payload JSON comes on stdin).
/// See docs/ARCHITECTURE.md "Harness profiles" and "Turn outcomes".
pub const FLAG_NOTIFY: &str = "--notify";
const FLAG_EVENT: &str = "--event";
const FLAG_STDIN: &str = "--stdin";
const NOTIFY_FILE: &str = "hooks.log";
/// Words a hooks.log line may start with.
pub const NOTIFY_WORDS: [&str; 4] = ["start", "submit", "stop", "notify"];
/// At most this much of stdin is read; a larger payload is dropped.
pub const STDIN_MAX: usize = 4 * 1024 * 1024;

/// A parsed notify invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyArgs<'a> {
    pub file: PathBuf,
    /// The line's word (`stop` unless `--event` names another).
    pub word: &'a str,
    /// Read the payload from stdin.
    pub stdin: bool,
    /// The payload argument (Codex appends one JSON argument).
    pub payload: Option<&'a str>,
}

/// `Some(..)` when `args` (after the executable) ask for the notify mode.
/// An unknown `--event` word is `None` (nothing is written).
pub fn notify_args(args: &[String]) -> Option<NotifyArgs<'_>> {
    let [flag, file, rest @ ..] = args else {
        return None;
    };
    if flag != FLAG_NOTIFY {
        return None;
    }
    let mut word = "stop";
    let mut stdin = false;
    let mut payload = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            FLAG_EVENT => {
                word = rest.get(i + 1).map(String::as_str)?;
                if !NOTIFY_WORDS.contains(&word) {
                    return None;
                }
                i += 1;
            }
            FLAG_STDIN => stdin = true,
            other => payload = Some(other),
        }
        i += 1;
    }
    Some(NotifyArgs {
        file: PathBuf::from(file),
        word,
        stdin,
        payload,
    })
}

/// The hooks.log line for a payload: `<word>`, plus the payload
/// re-serialized as one line of compact JSON when it is a JSON object (so
/// the session can read the hook's fields). Anything else is dropped unread.
pub fn notify_line_for(word: &str, payload: Option<&str>) -> String {
    let object = payload
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .filter(serde_json::Value::is_object)
        .and_then(|v| serde_json::to_string(&v).ok());
    match object {
        Some(json) => format!("{word} {json}\n"),
        None => format!("{word}\n"),
    }
}

/// [`notify_line_for`] with the word `stop` (Codex's notify).
pub fn notify_line(payload: Option<&str>) -> String {
    notify_line_for("stop", payload)
}

/// Append a line to `file`, which must be a session's `hooks.log`.
pub fn notify_word(file: &Path, word: &str, payload: Option<&str>) -> std::io::Result<()> {
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
    f.write_all(notify_line_for(word, payload).as_bytes())
}

/// [`notify_word`] with `stop` (Codex).
pub fn notify(file: &Path, payload: Option<&str>) -> std::io::Result<()> {
    notify_word(file, "stop", payload)
}

/// Up to [`STDIN_MAX`] bytes of stdin as text (`None` when larger or not UTF-8).
fn read_stdin() -> Option<String> {
    use std::io::Read as _;
    let mut buf = Vec::new();
    let limit = u64::try_from(STDIN_MAX).unwrap_or(u64::MAX) + 1;
    std::io::stdin().take(limit).read_to_end(&mut buf).ok()?;
    if buf.len() > STDIN_MAX {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// The notify mode, if argv asks for it: returns the exit code to use.
pub fn notify_from_process() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(FLAG_NOTIFY) {
        return None;
    }
    let Some(parsed) = notify_args(&args) else {
        eprintln!("claude-graph --notify: bad arguments");
        return Some(2);
    };
    let input = if parsed.stdin { read_stdin() } else { None };
    let payload = input.as_deref().or(parsed.payload);
    Some(match notify_word(&parsed.file, parsed.word, payload) {
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
        let n = notify_args(&only_file).unwrap();
        assert_eq!(
            (n.file, n.payload, n.word),
            (PathBuf::from("C:/s/hooks.log"), None, "stop")
        );
        let hostile = "{\"a\":\"\\\"&|<>\"}\r\n& del /q *| echo pwned";
        let a = args(&["--notify", "/s/hooks.log", hostile]);
        assert_eq!(notify_args(&a).unwrap().payload, Some(hostile));
        let hook = args(&["--notify", "/s/hooks.log", "--event", "submit", "--stdin"]);
        let n = notify_args(&hook).unwrap();
        assert_eq!((n.word, n.stdin, n.payload), ("submit", true, None));
        let bad = args(&["--notify", "/s/hooks.log", "--event", "rm"]);
        assert_eq!(notify_args(&bad), None);
        assert_eq!(
            notify_args(&args(&["--notify", "/s/h.log", "--event"])),
            None
        );
        assert_eq!(
            notify_line_for("start", Some("{\"x\": 1}")),
            "start {\"x\":1}\n"
        );
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
