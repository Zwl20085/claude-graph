//! What a session needs from a harness: how to start it ([`spawn_spec`]) and
//! how to read it ([`Behavior`]: markers, submit check, stop signal).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::claude_bin::{ClaudeCommand, ResolveEnv};
use crate::contract::{AppResult, HarnessKind, HarnessProfile};
use crate::harness::command::program_of;
use crate::harness::env::expand_profile;
use crate::harness::{claude, codex, custom, dsh};
use crate::terminal::deliver::Expect;
use crate::terminal::screen::Markers;

/// How the end of a turn shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopSignal {
    /// A `stop` line in hooks.log (Claude Code's Stop hook, Codex's notify).
    Hook,
    /// No output for this long after the submit.
    Quiet(Duration),
}

/// How a session talks to its CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// One interactive process in a pty; messages are typed.
    Pty,
    /// One headless process per message, chained by the CLI's session id
    /// (`crate::terminal::chained`; dsh).
    Chained,
}

/// How a session reads its CLI.
#[derive(Debug, Clone)]
pub struct Behavior {
    pub label: String,
    pub markers: Arc<Markers>,
    /// The CLI reports its start with a `start` hook line (else: readiness).
    pub start_hook: bool,
    pub expect: Expect,
    /// Type after this much quiet when no input box and no dialog show.
    pub ready_quiet: Duration,
    pub stop: StopSignal,
    /// A node's Claude model is typed as `/model <m>` (claude kind only).
    pub types_model: bool,
    pub driver: Driver,
}

/// The behavior of `profile`'s kind.
pub fn behavior(profile: &HarnessProfile) -> Behavior {
    let mut b = match profile.kind {
        HarnessKind::Claude => claude::behavior(),
        HarnessKind::Codex => codex::behavior(),
        HarnessKind::Custom => custom::behavior(profile),
        HarnessKind::DeepseekHarness => dsh::behavior(),
    };
    b.label = profile.label.clone();
    b
}

/// Inputs of [`spawn_spec`].
pub struct SpawnInput<'a> {
    pub profile: &'a HarnessProfile,
    /// The default claude command (claude-kind profiles without a command).
    pub claude: Option<&'a ClaudeCommand>,
    pub graph_dir: &'a Path,
    pub cwd: &'a Path,
    /// The session folder: `hooks.json` and `hooks.log` live here.
    pub session_dir: &'a Path,
    /// The app binary, run by Codex's notify as `<exe> --notify <hooks.log>`.
    pub notify_exe: &'a Path,
    /// `--permission-mode` value for claude-kind sessions.
    pub permission: Option<&'a str>,
    /// Environment lookup for `${VAR}` (the app's process environment).
    pub lookup: &'a dyn Fn(&str) -> Option<String>,
    pub resolve: &'a ResolveEnv,
}

/// The process to start: program, args, and the profile's expanded env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// `(key, Some(value))` from the profile (values never logged).
    pub env: Vec<(String, Option<String>)>,
    /// Program and prefix for the log (no env).
    pub display: String,
}

/// Program, args and env of a session of `input.profile`. Fails, before
/// anything is spawned, on a missing `${VAR}` or a program not found.
pub fn spawn_spec(input: &SpawnInput<'_>) -> AppResult<Launch> {
    let env = expand_profile(input.profile, input.lookup)?;
    let cmd = program_of(input.profile, input.claude, input.resolve)?;
    let kind_args = match input.profile.kind {
        HarnessKind::Claude => claude::args(input),
        HarnessKind::Codex => codex::args(input),
        HarnessKind::Custom => custom::args(input),
        HarnessKind::DeepseekHarness => dsh::args(input),
    };
    let args = cmd
        .prefix_args
        .iter()
        .cloned()
        .chain(kind_args)
        .chain(input.profile.args.iter().map(OsString::from))
        .collect();
    Ok(Launch {
        program: cmd.program,
        args,
        env,
        display: cmd.display,
    })
}

/// `-c notify=[...]` value: a TOML array of basic strings (backslashes and
/// quotes escaped, control characters as `\uXXXX`).
pub fn notify_toml(exe: &Path, hooks_log: &Path) -> String {
    let items = [
        toml_string(&exe.display().to_string()),
        toml_string("--notify"),
        toml_string(&hooks_log.display().to_string()),
    ];
    format!("notify=[{}]", items.join(","))
}

fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_toml_doubles_backslashes_and_escapes_quotes() {
        let v = notify_toml(
            Path::new(r"C:\Program Files\cg\claude-graph.exe"),
            Path::new(r#"C:\g "x"\hooks.log"#),
        );
        assert_eq!(
            v,
            r#"notify=["C:\\Program Files\\cg\\claude-graph.exe","--notify","C:\\g \"x\"\\hooks.log"]"#
        );
        assert_eq!(toml_string("a\tb"), r#""a\u0009b""#);
    }
}
