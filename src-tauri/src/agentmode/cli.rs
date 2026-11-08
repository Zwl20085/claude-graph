//! `claude-graph graph <command> …`: the agent-called mode for agents that
//! prefer a shell (and for people). Same engine as the MCP tools.
//!
//! Exit codes: 0 ok, 1 error, 2 usage, 3 nothing ready (`next`), 4 all done (`next`).

use std::path::PathBuf;

use serde::Serialize;

use crate::contract::{AppError, AppResult};

use super::report::{Outcome, ReportInput};
use super::{client_name, next, open, report, run, text, Caller, Channel, CLI_WORD, SUMMARY_MAX};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_NOTHING_READY: i32 = 3;
pub const EXIT_DONE: i32 = 4;

pub const USAGE: &str = "\
usage: claude-graph graph <command> <dir> [options]
  open   <dir> [--new]                          start (or resume) the agent run of a graph folder
  next   <dir> [--run <id>] [--max <n>] [--retry <node>]
                                                hand out the next ready node(s) with their briefs
  report <dir> <node> --status done|failed|blocked
         [--summary <text> | --summary-file <path>] [--question <text>]
         [--written <path>]… [--run <id>]       report how a handed-out node ended
  note   <dir> <text> [--node <id>] [--run <id>] add a line to the run log shown in the app
  status <dir> [--run <id>]                     table of node statuses
common: --json (machine output), --client <name> (who is driving; default: detected)
exit codes: 0 ok, 1 error, 2 usage, 3 nothing ready, 4 all done";

/// Longest `--summary-file` read.
const SUMMARY_FILE_MAX: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Open {
        new_run: bool,
    },
    Next {
        max: usize,
        retry: Option<String>,
    },
    Report {
        node: String,
        outcome: Outcome,
        summary: Option<String>,
        summary_file: Option<PathBuf>,
        question: Option<String>,
        written: Vec<String>,
    },
    Note {
        text: String,
        node: Option<String>,
    },
    Status,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub command: Command,
    pub dir: String,
    pub run: Option<String>,
    pub json: bool,
    pub client: Option<String>,
}

fn value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Parse the arguments after `graph`.
pub fn parse(args: &[String]) -> Result<Cli, String> {
    let Some(word) = args.first() else {
        return Err("missing command".into());
    };
    if matches!(word.as_str(), "help" | "--help" | "-h") {
        return Ok(Cli {
            command: Command::Help,
            dir: String::new(),
            run: None,
            json: false,
            client: None,
        });
    }
    let mut positional = Vec::new();
    let (mut run, mut json, mut client, mut new_run) = (None, false, None, false);
    let (mut max, mut retry, mut status, mut summary) = (1usize, None, None, None);
    let (mut summary_file, mut question, mut node, mut written) = (None, None, None, Vec::new());
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--json" => json = true,
            "--new" => new_run = true,
            "--run" | "--client" | "--max" | "--retry" | "--status" | "--summary"
            | "--summary-file" | "--question" | "--node" | "--written" => {
                let v = value(args, i, a)?;
                i += 1;
                match a {
                    "--run" => run = Some(v),
                    "--client" => client = Some(v),
                    "--max" => {
                        max = v
                            .parse()
                            .ok()
                            .filter(|n| *n >= 1)
                            .ok_or("--max needs a positive number")?
                    }
                    "--retry" => retry = Some(v),
                    "--status" => {
                        status = Some(
                            Outcome::parse(&v).ok_or("--status must be done, failed or blocked")?,
                        )
                    }
                    "--summary" => summary = Some(v),
                    "--summary-file" => summary_file = Some(PathBuf::from(v)),
                    "--question" => question = Some(v),
                    "--node" => node = Some(v),
                    _ => written.push(v),
                }
            }
            "--" => positional.extend(args[i + 1..].iter().cloned()),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ => positional.push(args[i].clone()),
        }
        if a == "--" {
            break;
        }
        i += 1;
    }
    let mut pos = positional.into_iter();
    let dir = pos.next().ok_or("missing <dir>")?;
    let command = match word.as_str() {
        "open" => Command::Open { new_run },
        "next" => Command::Next { max, retry },
        "status" => Command::Status,
        "report" => {
            let node = pos.next().ok_or("missing <node>")?;
            if summary.is_some() && summary_file.is_some() {
                return Err("give --summary or --summary-file, not both".into());
            }
            Command::Report {
                node,
                outcome: status.ok_or("report needs --status done|failed|blocked")?,
                summary,
                summary_file,
                question,
                written,
            }
        }
        "note" => {
            let words: Vec<String> = pos.by_ref().collect();
            if words.is_empty() {
                return Err("missing note text".into());
            }
            Command::Note {
                text: words.join(" "),
                node,
            }
        }
        other => return Err(format!("unknown command {other:?}")),
    };
    if let Some(extra) = pos.next() {
        return Err(format!("unexpected argument {extra:?}"));
    }
    Ok(Cli {
        command,
        dir,
        run,
        json,
        client,
    })
}

/// Who is driving, when `--client` is not given (only whether a variable
/// exists is looked at, never its value).
fn detected_client() -> String {
    let has = |k: &str| std::env::var_os(k).is_some();
    if has("CLAUDECODE") || has("CLAUDE_CODE_ENTRYPOINT") {
        "Claude Code".into()
    } else if has("CODEX_SANDBOX") || has("CODEX_THREAD_ID") {
        "Codex".into()
    } else {
        "a shell".into()
    }
}

fn read_summary_file(path: &PathBuf) -> AppResult<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::new("not_found", format!("{}: {e}", path.display())))?;
    let mut text = String::new();
    file.take(SUMMARY_FILE_MAX)
        .read_to_string(&mut text)
        .map_err(|e| AppError::new("invalid_input", format!("{}: {e}", path.display())))?;
    Ok(super::cut_bytes(&text, SUMMARY_MAX).to_string())
}

fn emit<T: Serialize>(json: bool, value: &T, text: String) -> AppResult<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

fn run_id(dir: &std::path::Path, given: Option<&str>) -> AppResult<String> {
    Ok(open::find(dir, given)?.state.run_id)
}

/// Run a parsed command; the exit code.
pub fn execute(cli: &Cli) -> AppResult<i32> {
    if cli.command == Command::Help {
        println!("{USAGE}");
        return Ok(EXIT_OK);
    }
    let dir = run::graph_dir(&cli.dir, true)?;
    let caller = Caller {
        client: cli
            .client
            .as_deref()
            .map_or_else(detected_client, client_name),
        channel: Channel::Cli,
    };
    match &cli.command {
        Command::Open { new_run } => {
            let o = open::open(&dir, &caller, *new_run)?;
            emit(cli.json, &o, text::overview(&o))?;
        }
        Command::Next { max, retry } => {
            let id = run_id(&dir, cli.run.as_deref())?;
            let n = next::next(&dir, &id, &caller, *max, retry.as_deref())?;
            emit(cli.json, &n, text::next(&n))?;
            if n.done {
                return Ok(EXIT_DONE);
            }
            if n.nodes.is_empty() {
                return Ok(EXIT_NOTHING_READY);
            }
        }
        Command::Report {
            node,
            outcome,
            summary,
            summary_file,
            question,
            written,
        } => {
            let summary = match summary_file {
                Some(p) => read_summary_file(p)?,
                None => summary.clone().unwrap_or_default(),
            };
            let input = ReportInput {
                node_id: node.clone(),
                outcome: *outcome,
                summary,
                question: question.clone(),
                deliverables_written: written.clone(),
            };
            let id = run_id(&dir, cli.run.as_deref())?;
            let r = report::report(&dir, &id, &caller, &input)?;
            emit(cli.json, &r, text::report(&r))?;
        }
        Command::Note { text: note, node } => {
            open::note(&dir, cli.run.as_deref(), node.as_deref(), note)?;
            emit(
                cli.json,
                &serde_json::json!({ "ok": true }),
                "Noted.\n".into(),
            )?;
        }
        Command::Status => {
            let s = open::status(&dir, cli.run.as_deref())?;
            emit(cli.json, &s, text::status(&s))?;
        }
        Command::Help => {}
    }
    Ok(EXIT_OK)
}

/// Windows release builds are GUI-subsystem programs: a shell's console is
/// not attached, so attach to the parent's console when stdout has no handle
/// (output into a pipe keeps working as is).
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
    };
    // SAFETY: GetStdHandle only reads this process's standard handle table.
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if handle.is_null() {
        // SAFETY: AttachConsole takes a process id constant and has no
        // pointer arguments; failure (no parent console) is harmless.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

#[cfg(not(windows))]
fn attach_console() {}

/// `Some(exit code)` when argv (after the executable) starts with `graph`.
pub fn from_process() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some(CLI_WORD) {
        return None;
    }
    attach_console();
    let cli = match parse(&args[1..]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("claude-graph graph: {e}\n{USAGE}");
            return Some(EXIT_USAGE);
        }
    };
    let json = cli.json;
    Some(match execute(&cli) {
        Ok(code) => code,
        Err(e) => {
            if json {
                let err = serde_json::json!({ "error": { "code": e.code, "message": e.message } });
                println!("{err}");
            }
            eprintln!("claude-graph graph: {} ({})", e.message, e.code);
            EXIT_ERROR
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(list: &[&str]) -> Result<Cli, String> {
        parse(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_every_command() {
        let c = p(&["open", "C:/g", "--new", "--json"]).unwrap();
        assert_eq!(c.command, Command::Open { new_run: true });
        assert!(c.json);
        let c = p(&["next", "C:/g", "--run", "r1", "--max", "2", "--retry", "a"]).unwrap();
        assert_eq!(
            c.command,
            Command::Next {
                max: 2,
                retry: Some("a".into())
            }
        );
        assert_eq!(c.run.as_deref(), Some("r1"));
        let c = p(&[
            "report",
            "C:/g",
            "n1",
            "--status",
            "blocked",
            "--summary",
            "s",
            "--question",
            "q?",
            "--written",
            "a.md",
            "--written",
            "b.md",
        ])
        .unwrap();
        assert_eq!(
            c.command,
            Command::Report {
                node: "n1".into(),
                outcome: Outcome::Blocked,
                summary: Some("s".into()),
                summary_file: None,
                question: Some("q?".into()),
                written: vec!["a.md".into(), "b.md".into()],
            }
        );
        let c = p(&["note", "C:/g", "hello", "there", "--node", "n1"]).unwrap();
        assert_eq!(
            c.command,
            Command::Note {
                text: "hello there".into(),
                node: Some("n1".into())
            }
        );
        assert_eq!(p(&["status", "C:/g"]).unwrap().command, Command::Status);
        assert_eq!(p(&["--help"]).unwrap().command, Command::Help);
    }

    #[test]
    fn rejects_bad_usage() {
        assert!(p(&[]).is_err());
        assert!(p(&["open"]).is_err());
        assert!(p(&["fly", "C:/g"]).is_err());
        assert!(p(&["next", "C:/g", "--max", "0"]).is_err());
        assert!(p(&["next", "C:/g", "--run"]).is_err());
        assert!(p(&["report", "C:/g", "n"]).is_err());
        assert!(p(&["report", "C:/g", "n", "--status", "maybe"]).is_err());
        assert!(p(&[
            "report",
            "C:/g",
            "n",
            "--status",
            "done",
            "--summary",
            "a",
            "--summary-file",
            "f"
        ])
        .is_err());
        assert!(p(&["status", "C:/g", "extra"]).is_err());
        assert!(p(&["status", "C:/g", "--bogus"]).is_err());
        assert!(p(&["note", "C:/g"]).is_err());
    }
}
