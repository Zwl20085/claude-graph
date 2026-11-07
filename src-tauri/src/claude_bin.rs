//! Locating the `claude` CLI and checking its version.
//!
//! Resolution order (docs/CLAUDE_CLI_NOTES.md, "Windows"):
//! 1. `settings.claude_bin` if set (authoritative: if it does not exist, not found);
//! 2. env `CLAUDE_GRAPH_CLAUDE_BIN`, a command line split on whitespace with
//!    double quotes grouping a token (for example `node scripts/fake-claude.mjs`);
//! 3. `%USERPROFILE%\.local\bin\claude.exe` (Windows) / `~/.local/bin/claude` (Unix);
//! 4. a PATH search (PATHEXT order on Windows, so `claude.exe` wins over `claude.cmd`).
//!
//! A `.cmd` / `.bat` hit is spawned as `cmd /C <path> args…`.
//! The runner reuses [`ClaudeCommand::command`] to spawn the real process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::contract::ClaudeCheck;

/// Environment override used by tests and demos.
pub const ENV_CLAUDE_BIN: &str = "CLAUDE_GRAPH_CLAUDE_BIN";

const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How to start `claude`: `program prefix_args… <caller args…>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCommand {
    pub program: PathBuf,
    pub prefix_args: Vec<OsString>,
    /// Human-readable path shown in the UI.
    pub display: String,
}

impl ClaudeCommand {
    fn direct(path: PathBuf) -> Self {
        let display = path.display().to_string();
        if is_batch_file(&path) {
            Self {
                program: PathBuf::from("cmd"),
                prefix_args: vec![OsString::from("/C"), path.into_os_string()],
                display,
            }
        } else {
            Self {
                program: path,
                prefix_args: Vec::new(),
                display,
            }
        }
    }

    /// A tokio command with the prefix args applied, no console window on Windows,
    /// and `kill_on_drop` set. The caller adds its own args, stdio and cwd.
    pub fn command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.prefix_args).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
}

fn is_batch_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

/// Inputs to resolution, separated from the process environment for testing.
#[derive(Debug, Clone, Default)]
pub struct ResolveEnv {
    pub configured: Option<String>,
    pub env_override: Option<String>,
    pub home: Option<PathBuf>,
    pub path_var: Option<OsString>,
    pub pathext: Option<String>,
}

impl ResolveEnv {
    pub fn from_process(configured: Option<String>) -> Self {
        Self {
            configured,
            env_override: std::env::var(ENV_CLAUDE_BIN).ok(),
            home: dirs::home_dir(),
            path_var: std::env::var_os("PATH"),
            pathext: std::env::var("PATHEXT").ok(),
        }
    }
}

/// Outcome of resolution: either a command, or the path that was tried and missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Found(ClaudeCommand),
    Missing { tried: Option<String> },
}

pub fn resolve(env: &ResolveEnv) -> Resolution {
    if let Some(configured) = non_empty(env.configured.as_deref()) {
        let path = PathBuf::from(configured);
        return if path.is_file() {
            Resolution::Found(ClaudeCommand::direct(path))
        } else {
            Resolution::Missing {
                tried: Some(configured.to_string()),
            }
        };
    }
    if let Some(line) = non_empty(env.env_override.as_deref()) {
        return resolve_command_line(line, env);
    }
    if let Some(home) = &env.home {
        let local = home.join(".local").join("bin").join(exe_name("claude"));
        if local.is_file() {
            return Resolution::Found(ClaudeCommand::direct(local));
        }
    }
    match find_on_path("claude", env.path_var.as_ref(), env.pathext.as_deref()) {
        Some(p) => Resolution::Found(ClaudeCommand::direct(p)),
        None => Resolution::Missing { tried: None },
    }
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// Split a command line on whitespace; double quotes group a token
/// (`"C:\Program Files\nodejs\node.exe" script.mjs`). Backslashes are literal.
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current: Option<String> = None;
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.get_or_insert_with(String::new);
            }
            c if c.is_whitespace() && !quoted => {
                if let Some(t) = current.take() {
                    tokens.push(t);
                }
            }
            c => current.get_or_insert_with(String::new).push(c),
        }
    }
    tokens.extend(current);
    tokens
}

/// `node scripts/fake-claude.mjs` → program `node` (looked up on PATH) + args.
fn resolve_command_line(line: &str, env: &ResolveEnv) -> Resolution {
    let tokens = split_command_line(line);
    let Some((first, rest)) = tokens.split_first() else {
        return Resolution::Missing { tried: None };
    };
    let first = first.as_str();
    let rest: Vec<OsString> = rest.iter().map(OsString::from).collect();
    let first_path = PathBuf::from(first);
    let program = if first_path.is_file() {
        Some(first_path)
    } else if first_path.components().count() == 1 {
        find_on_path(first, env.path_var.as_ref(), env.pathext.as_deref())
    } else {
        None
    };
    match program {
        Some(p) => {
            let base = ClaudeCommand::direct(p);
            let display = std::iter::once(base.display.clone())
                .chain(rest.iter().map(|a| a.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(" ");
            Resolution::Found(ClaudeCommand {
                prefix_args: base.prefix_args.into_iter().chain(rest).collect(),
                display,
                ..base
            })
        }
        None => Resolution::Missing {
            tried: Some(line.to_string()),
        },
    }
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Search `path_var` for `name`. On Windows (or when `pathext` is given) every
/// PATHEXT extension is tried in order and the bare name is *not* tried
/// (npm puts an extension-less shell script `claude` next to `claude.cmd`).
/// Without extensions (Unix) only the bare name is tried.
pub fn find_on_path(
    name: &str,
    path_var: Option<&OsString>,
    pathext: Option<&str>,
) -> Option<PathBuf> {
    let path_var = path_var?;
    let exts: Vec<String> = match pathext {
        Some(p) => p
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect(),
        None if cfg!(windows) => [".COM", ".EXE", ".BAT", ".CMD"].map(String::from).to_vec(),
        None => Vec::new(),
    };
    std::env::split_paths(path_var).find_map(|dir| {
        let candidates: Vec<PathBuf> = if exts.is_empty() {
            vec![dir.join(name)]
        } else {
            exts.iter()
                .map(|ext| dir.join(format!("{name}{}", ext.to_ascii_lowercase())))
                .collect()
        };
        candidates.into_iter().find(|candidate| candidate.is_file())
    })
}

/// Run `<claude> --version` and return the first non-empty stdout line.
pub async fn read_version(cmd: &ClaudeCommand) -> Option<String> {
    let mut command = cmd.command();
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = tokio::time::timeout(VERSION_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Full check used by the `check_claude` command.
pub async fn check(configured: Option<String>) -> ClaudeCheck {
    match resolve(&ResolveEnv::from_process(configured)) {
        Resolution::Found(cmd) => {
            let version = read_version(&cmd).await;
            ClaudeCheck {
                found: true,
                path: Some(cmd.display),
                version,
            }
        }
        Resolution::Missing { tried } => ClaudeCheck {
            found: false,
            path: tried,
            version: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::write(p, b"").unwrap();
    }

    #[test]
    fn configured_path_is_authoritative() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("claude.exe");
        touch(&bin);
        let env = ResolveEnv {
            configured: Some(bin.display().to_string()),
            env_override: Some("node fake.mjs".into()),
            ..Default::default()
        };
        assert_eq!(resolve(&env), Resolution::Found(ClaudeCommand::direct(bin)));

        let missing = ResolveEnv {
            configured: Some("/no/such/claude".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve(&missing),
            Resolution::Missing {
                tried: Some("/no/such/claude".into())
            }
        );
    }

    #[test]
    fn env_override_is_split_into_program_and_args() {
        let dir = tempfile::tempdir().unwrap();
        let node = dir.path().join("node.exe");
        touch(&node);
        let env = ResolveEnv {
            env_override: Some("node scripts/fake-claude.mjs".into()),
            path_var: Some(dir.path().as_os_str().to_owned()),
            pathext: Some(".EXE".into()),
            ..Default::default()
        };
        match resolve(&env) {
            Resolution::Found(cmd) => {
                assert_eq!(cmd.program, node);
                assert_eq!(
                    cmd.prefix_args,
                    vec![OsString::from("scripts/fake-claude.mjs")]
                );
            }
            other => panic!("expected found, got {other:?}"),
        }
    }

    #[test]
    fn command_line_split_honours_quotes() {
        assert_eq!(
            split_command_line(r#""C:\Program Files\nodejs\node.exe" "a b.mjs" --x"#),
            vec![r"C:\Program Files\nodejs\node.exe", "a b.mjs", "--x"]
        );
        assert_eq!(split_command_line("  node   s.mjs "), vec!["node", "s.mjs"]);
        assert_eq!(split_command_line(r#""""#), vec![""]);
        assert!(split_command_line("   ").is_empty());

        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("with space");
        fs::create_dir_all(&spaced).unwrap();
        let node = spaced.join("node.exe");
        touch(&node);
        let env = ResolveEnv {
            env_override: Some(format!("\"{}\" script.mjs", node.display())),
            ..Default::default()
        };
        match resolve(&env) {
            Resolution::Found(cmd) => {
                assert_eq!(cmd.program, node);
                assert_eq!(cmd.prefix_args, vec![OsString::from("script.mjs")]);
            }
            other => panic!("expected found, got {other:?}"),
        }
    }

    #[test]
    fn path_search_prefers_exe_and_wraps_cmd_shims() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("claude.cmd");
        touch(&shim);
        touch(&dir.path().join("claude")); // npm's sh script, must be ignored
        let path_var = Some(dir.path().as_os_str().to_owned());
        let env = ResolveEnv {
            path_var: path_var.clone(),
            pathext: Some(".EXE;.CMD".into()),
            ..Default::default()
        };
        match resolve(&env) {
            Resolution::Found(cmd) => {
                assert_eq!(cmd.program, PathBuf::from("cmd"));
                assert_eq!(cmd.prefix_args[0], OsString::from("/C"));
                assert_eq!(PathBuf::from(&cmd.prefix_args[1]), shim);
            }
            other => panic!("expected found, got {other:?}"),
        }

        let exe = dir.path().join("claude.exe");
        touch(&exe);
        assert_eq!(
            find_on_path("claude", path_var.as_ref(), Some(".EXE;.CMD")),
            Some(exe)
        );
    }

    #[test]
    fn nothing_found_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let env = ResolveEnv {
            home: Some(dir.path().to_path_buf()),
            path_var: Some(dir.path().as_os_str().to_owned()),
            pathext: Some(".EXE".into()),
            ..Default::default()
        };
        assert_eq!(resolve(&env), Resolution::Missing { tried: None });
    }

    /// Spawns `node scripts/fake-claude.mjs --version` through the same code path
    /// the app uses. Needs `node` on PATH (true in CI and on dev machines).
    #[tokio::test]
    async fn reads_version_from_fake_claude() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/fake-claude.mjs");
        let env = ResolveEnv {
            env_override: Some(format!("node {}", script.display())),
            ..ResolveEnv::from_process(None)
        };
        let Resolution::Found(cmd) = resolve(&env) else {
            panic!("node not found on PATH");
        };
        let version = read_version(&cmd).await;
        assert_eq!(version.as_deref(), Some("2.1.0 (fake-claude)"));
    }

    /// Diagnostic against the real machine: `cargo test -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "depends on the local claude install"]
    async fn real_machine_check() {
        println!("{:?}", check(None).await);
    }
}
