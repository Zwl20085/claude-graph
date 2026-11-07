//! Startup options: `claude-graph <graph-dir> [--run]`, or the env fallback
//! `CLAUDE_GRAPH_OPEN=<dir>` / `CLAUDE_GRAPH_AUTORUN=1` (handy under `tauri dev`).
//! See docs/ARCHITECTURE.md "Launching".

use std::collections::HashMap;

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
    fn serialises_snake_case() {
        let json = serde_json::to_string(&opts(Some("/g"), true)).unwrap();
        assert_eq!(json, r#"{"open_dir":"/g","autorun":true}"#);
    }
}
