//! Locating the `claude` CLI and checking its version.
//!
//! Resolution order (docs/CLAUDE_CLI_NOTES.md, "Windows"):
//! 1. the app settings' `claude_bin` (`<data_dir>/claude-graph/settings.json`;
//!    authoritative: if it is invalid or missing, not found). `settings.claude_bin`
//!    in graph.json is never read, touched or executed;
//! 2. env `CLAUDE_GRAPH_CLAUDE_BIN`, a command line split on whitespace with
//!    double quotes grouping a token (for example `node scripts/fake-claude.mjs`);
//! 3. `%USERPROFILE%\.local\bin\claude.exe` (Windows) / `~/.local/bin/claude` (Unix);
//! 4. a PATH search (PATHEXT order on Windows, so `claude.exe` wins over `claude.cmd`).
//!
//! Spawn strategy (never `cmd /C`): an `.exe` is spawned directly. An npm
//! `.cmd`/`.bat` shim is resolved to `node.exe <shim dir>\node_modules\@anthropic-ai\claude-code\cli.js`
//! when that file exists (node.exe next to the shim, else on PATH); else to
//! `<shim dir>\node_modules\@anthropic-ai\claude-code\bin\claude.exe` (current
//! npm releases), spawned directly; otherwise the batch file itself is the
//! program. Headless runs spawn it through Rust's std, which applies its
//! batch-file argument escaping (and refuses arguments it cannot pass
//! safely, reported as `spawn_failed`). The pty (terminal mode) has no such
//! escaping: cmd.exe would read `&`, `|`, `%`, … in an argument, so
//! [`batch_args_safe`] refuses those spawns (see `terminal::pty`).
//! The runner reuses [`ClaudeCommand::command`] to spawn the real process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app_settings::validate_claude_bin;
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
    fn plain(path: PathBuf) -> Self {
        Self {
            display: path.display().to_string(),
            program: path,
            prefix_args: Vec::new(),
        }
    }

    /// The command for an executable found on disk (see the module docs).
    pub fn for_path(path: PathBuf, env: &ResolveEnv) -> Self {
        if !is_batch_file(&path) {
            return Self::plain(path);
        }
        if let Some((node, cli)) = npm_shim_target(&path, env) {
            return Self {
                display: format!("{} {}", node.display(), cli.display()),
                program: node,
                prefix_args: vec![cli.into_os_string()],
            };
        }
        match npm_shim_exe(&path) {
            Some(exe) => Self::plain(exe),
            None => Self::plain(path),
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

/// Characters cmd.exe reads in a batch file's command line.
pub const BATCH_UNSAFE: &[char] = &['&', '|', '<', '>', '^', '%', '!', '"', '(', ')', '\n', '\r'];

/// `Err(c)`: an argument holds `c`, which cmd.exe would interpret when the
/// program is a batch file started without std's escaping (the pty).
pub fn batch_args_safe<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Result<(), char> {
    for a in args {
        let a = a.as_ref().to_string_lossy();
        if let Some(c) = a.chars().find(|c| BATCH_UNSAFE.contains(c)) {
            return Err(c);
        }
    }
    Ok(())
}

pub fn is_batch_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

/// npm's `claude.cmd` runs `node <shim dir>\node_modules\@anthropic-ai\claude-code\cli.js`.
/// Returns `(node, cli.js)` when the script exists and a node.exe is found next
/// to the shim or on PATH.
fn npm_shim_target(shim: &Path, env: &ResolveEnv) -> Option<(PathBuf, PathBuf)> {
    let dir = shim.parent()?;
    let cli = npm_package_dir(dir).join("cli.js");
    if !cli.is_file() {
        return None;
    }
    let beside = dir.join(exe_name("node"));
    let node = if beside.is_file() {
        beside
    } else {
        find_on_path("node", env.path_var.as_ref(), env.pathext.as_deref())?
    };
    Some((node, cli))
}

/// Current npm releases ship `<shim dir>\node_modules\@anthropic-ai\claude-code\bin\claude.exe`
/// and the shim just forwards to it; spawning it directly avoids cmd.exe.
fn npm_shim_exe(shim: &Path) -> Option<PathBuf> {
    let exe = npm_package_dir(shim.parent()?)
        .join("bin")
        .join("claude.exe");
    exe.is_file().then_some(exe)
}

fn npm_package_dir(shim_dir: &Path) -> PathBuf {
    shim_dir
        .join("node_modules")
        .join("@anthropic-ai")
        .join("claude-code")
}

/// Inputs to resolution, separated from the process environment for testing.
#[derive(Debug, Clone, Default)]
pub struct ResolveEnv {
    /// `claude_bin` from the app settings (never from graph.json).
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
        return match validate_claude_bin(configured) {
            Ok(path) => Resolution::Found(ClaudeCommand::for_path(path, env)),
            Err(e) => {
                log::warn!("app setting claude_bin rejected: {e}");
                Resolution::Missing {
                    tried: Some(configured.to_string()),
                }
            }
        };
    }
    if let Some(line) = non_empty(env.env_override.as_deref()) {
        return resolve_command_line(line, env);
    }
    if let Some(home) = &env.home {
        let local = home.join(".local").join("bin").join(exe_name("claude"));
        if local.is_file() {
            return Resolution::Found(ClaudeCommand::for_path(local, env));
        }
    }
    match find_on_path("claude", env.path_var.as_ref(), env.pathext.as_deref()) {
        Some(p) => Resolution::Found(ClaudeCommand::for_path(p, env)),
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
pub fn resolve_command_line(line: &str, env: &ResolveEnv) -> Resolution {
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
            let base = ClaudeCommand::for_path(p, env);
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

pub fn exe_name(stem: &str) -> String {
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

/// Full check used by the `check_claude` command; `configured` is the app
/// settings' `claude_bin`.
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
        let canonical = crate::paths::canonical(&bin).unwrap();
        assert_eq!(
            resolve(&env),
            Resolution::Found(ClaudeCommand::plain(canonical))
        );

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
        // A configured value that is not the CLI (or a UNC path) is refused, not run.
        let other = dir.path().join("calc.exe");
        touch(&other);
        for bad in [
            other.display().to_string(),
            r"\\host\share\claude.exe".into(),
        ] {
            let env = ResolveEnv {
                configured: Some(bad.clone()),
                ..Default::default()
            };
            assert_eq!(resolve(&env), Resolution::Missing { tried: Some(bad) });
        }
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
    fn path_search_prefers_exe_and_spawns_cmd_shims_directly() {
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
                assert_eq!(cmd.program, shim, "the batch file itself, never cmd /C");
                assert!(cmd.prefix_args.is_empty());
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
    fn npm_shim_resolves_to_node_and_cli_js() {
        let dir = tempfile::tempdir().unwrap();
        let npm = dir.path().join("npm dir");
        let pkg = npm
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code");
        fs::create_dir_all(&pkg).unwrap();
        let shim = npm.join("claude.cmd");
        touch(&shim);
        let cli = pkg.join("cli.js");
        touch(&cli);
        // node on PATH (another folder).
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        // Named with the extension the explicit PATHEXT below expects, on every OS.
        let node_on_path = bin.join("node.exe");
        touch(&node_on_path);
        let env = ResolveEnv {
            path_var: Some(bin.as_os_str().to_owned()),
            pathext: Some(".EXE".into()),
            ..Default::default()
        };
        let cmd = ClaudeCommand::for_path(shim.clone(), &env);
        assert_eq!(cmd.program, node_on_path);
        assert_eq!(cmd.prefix_args, vec![cli.clone().into_os_string()]);
        // node next to the shim wins.
        let beside = npm.join(exe_name("node"));
        touch(&beside);
        assert_eq!(ClaudeCommand::for_path(shim.clone(), &env).program, beside);
        // Without cli.js the shim itself is the program.
        fs::remove_file(&cli).unwrap();
        assert_eq!(ClaudeCommand::for_path(shim.clone(), &env).program, shim);
    }

    #[test]
    fn npm_shim_resolves_to_bundled_claude_exe() {
        let dir = tempfile::tempdir().unwrap();
        let npm = dir.path().join("npm dir");
        let pkg_bin = npm
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin");
        fs::create_dir_all(&pkg_bin).unwrap();
        let shim = npm.join("claude.cmd");
        touch(&shim);
        let exe = pkg_bin.join("claude.exe");
        touch(&exe);
        // node is available, but without cli.js the bundled exe is used.
        touch(&npm.join(exe_name("node")));
        let env = ResolveEnv::default();
        let cmd = ClaudeCommand::for_path(shim.clone(), &env);
        assert_eq!(cmd.program, exe, "spawned directly, no cmd.exe");
        assert!(cmd.prefix_args.is_empty());
        assert_eq!(cmd.display, exe.display().to_string());
        // cli.js, when present, still wins.
        let cli = pkg_bin.parent().unwrap().join("cli.js");
        touch(&cli);
        let cmd = ClaudeCommand::for_path(shim.clone(), &env);
        assert_eq!(cmd.prefix_args, vec![cli.clone().into_os_string()]);
        // Neither: the shim itself.
        fs::remove_file(&cli).unwrap();
        fs::remove_file(&exe).unwrap();
        assert_eq!(ClaudeCommand::for_path(shim.clone(), &env).program, shim);
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
