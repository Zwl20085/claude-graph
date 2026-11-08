//! Finding the program of a harness: `profile.command` (a path, or a name
//! looked up on PATH), else the kind's default (the claude resolution chain;
//! `codex` from env `CLAUDE_GRAPH_CODEX_BIN` or PATH). Never `cmd /C`: an npm
//! `codex.cmd` shim becomes `node <shim dir>/node_modules/@openai/codex/bin/codex.js`
//! when that file exists; any other batch file is spawned as the program.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::claude_bin::{
    exe_name, find_on_path, is_batch_file, split_command_line, ClaudeCommand, ResolveEnv,
};
use crate::contract::{AppError, AppResult, HarnessKind, HarnessProfile};
use crate::paths::is_unc_or_device;

/// Env override for the default `codex` (a command line, like
/// `CLAUDE_GRAPH_CLAUDE_BIN`; tests point it at `node scripts/fake-codex.mjs`).
pub const ENV_CODEX_BIN: &str = "CLAUDE_GRAPH_CODEX_BIN";
/// Env override for the default `dsh` (a command line; tests point it at
/// `node scripts/fake-dsh.mjs`).
pub const ENV_DSH_BIN: &str = "CLAUDE_GRAPH_DSH_BIN";

fn plain(path: PathBuf) -> ClaudeCommand {
    ClaudeCommand {
        display: path.display().to_string(),
        program: path,
        prefix_args: Vec::new(),
    }
}

/// `node codex.js` for an npm `codex.cmd` shim, `node bin.js` for a dsh
/// shim ([`dsh_shim`]), else the file itself.
fn for_path(path: PathBuf, env: &ResolveEnv) -> ClaudeCommand {
    if !is_batch_file(&path) {
        return plain(path);
    }
    if let Some(cmd) = dsh_shim(&path, env) {
        return cmd;
    }
    let script = path.parent().map(|d| {
        d.join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js")
    });
    let node = path.parent().map(|d| d.join(exe_name("node")));
    match (script.filter(|s| s.is_file()), node) {
        (Some(script), Some(beside)) => {
            let node = if beside.is_file() {
                Some(beside)
            } else {
                find_on_path("node", env.path_var.as_ref(), env.pathext.as_deref())
            };
            match node {
                Some(node) => ClaudeCommand {
                    display: format!("{} {}", node.display(), script.display()),
                    program: node,
                    prefix_args: vec![script.into_os_string()],
                },
                None => plain(path),
            }
        }
        _ => plain(path),
    }
}

/// A program path or a name looked up on PATH.
pub fn resolve_program(command: &str, env: &ResolveEnv) -> AppResult<ClaudeCommand> {
    let command = command.trim();
    if is_unc_or_device(command) {
        return Err(AppError::new(
            "invalid_input",
            format!("{command} is a network or device path"),
        ));
    }
    let path = Path::new(command);
    let found = if path.components().count() > 1 || path.is_absolute() {
        path.is_file().then(|| path.to_path_buf())
    } else {
        find_on_path(command, env.path_var.as_ref(), env.pathext.as_deref())
            .or_else(|| path.is_file().then(|| path.to_path_buf()))
    };
    found
        .map(|p| for_path(p, env))
        .ok_or_else(|| AppError::new("not_found", format!("program {command} not found")))
}

/// `node "x y.mjs"` style override line → program + args.
fn command_line(line: &str, var: &str, env: &ResolveEnv) -> AppResult<ClaudeCommand> {
    let tokens = split_command_line(line);
    let (first, rest) = tokens
        .split_first()
        .ok_or_else(|| AppError::new("not_found", format!("{var} is empty")))?;
    let base = resolve_program(first, env)?;
    let rest: Vec<OsString> = rest.iter().map(OsString::from).collect();
    Ok(ClaudeCommand {
        display: std::iter::once(base.display.clone())
            .chain(rest.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" "),
        prefix_args: base.prefix_args.into_iter().chain(rest).collect(),
        program: base.program,
    })
}

/// The default `dsh`: env [`ENV_DSH_BIN`], else `dsh` on PATH (its npm /
/// installer shims resolved to `node <bin.js>`, see [`dsh_shim`]).
pub fn default_dsh(env: &ResolveEnv) -> AppResult<ClaudeCommand> {
    match std::env::var(ENV_DSH_BIN)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        Some(line) => command_line(&line, ENV_DSH_BIN, env),
        None => resolve_program("dsh", env).map_err(|_| {
            AppError::new(
                "not_found",
                "dsh (DeepSeek Harness) not found on PATH; install it or set the command in the dsh harness",
            )
        }),
    }
}

/// The default `codex`: env [`ENV_CODEX_BIN`], else PATH.
pub fn default_codex(env: &ResolveEnv) -> AppResult<ClaudeCommand> {
    match std::env::var(ENV_CODEX_BIN)
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        Some(line) => command_line(&line, ENV_CODEX_BIN, env),
        None => resolve_program("codex", env).map_err(|_| {
            AppError::new(
                "not_found",
                "codex CLI not found on PATH; install it or set the command in the Codex harness",
            )
        }),
    }
}

/// The program of `profile`. `claude` is the already resolved default claude
/// command (used by claude-kind profiles without a command).
pub fn program_of(
    profile: &HarnessProfile,
    claude: Option<&ClaudeCommand>,
    env: &ResolveEnv,
) -> AppResult<ClaudeCommand> {
    if let Some(c) = profile.command.as_deref() {
        return resolve_program(c, env);
    }
    match profile.kind {
        HarnessKind::Claude => claude
            .cloned()
            .ok_or_else(|| AppError::new("claude_not_found", "claude CLI not found")),
        HarnessKind::Codex => default_codex(env),
        HarnessKind::DeepseekHarness => default_dsh(env),
        HarnessKind::Custom => Err(AppError::new(
            "invalid_input",
            format!("harness {} has no command", profile.id),
        )),
    }
}

/// The dsh launcher script next to an install folder.
fn dsh_script(dir: &Path) -> Option<PathBuf> {
    let script = dir
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js");
    script.is_file().then_some(script)
}

/// A `dsh.cmd` shim as `node <bin.js>` without cmd.exe. Handles the npm shim
/// (bin.js beside it) and the installer's two-level shim (verified
/// 2026-11-08: `%APPDATA%\npm\dsh.cmd` sets `PATH=<node dir>;%PATH%` and
/// `call`s `<install>\dsh.cmd`, which runs `node <install>\node_modules\
/// @deepseek-ai\dsh\lib\bin.js`). Node: the one the shim puts first on
/// PATH, else one beside the script's install folder, else `node` on PATH.
pub fn dsh_shim(shim: &Path, env: &ResolveEnv) -> Option<ClaudeCommand> {
    let text = std::fs::read_to_string(shim).unwrap_or_default();
    let quoted = |prefix: &str| -> Option<String> {
        text.lines().find_map(|l| {
            let l = l.trim();
            let at = l.to_ascii_lowercase().find(prefix)?;
            let rest = &l[at + prefix.len()..];
            Some(rest.split('"').next()?.to_string())
        })
    };
    let dir = shim.parent()?;
    let inner = quoted("call \"").map(PathBuf::from);
    let install = match dsh_script(dir) {
        Some(_) => dir.to_path_buf(),
        None => inner.as_deref().and_then(Path::parent)?.to_path_buf(),
    };
    let script = dsh_script(&install)?;
    let path_dir = quoted("set \"path=")
        .and_then(|v| v.split(';').next().map(PathBuf::from))
        .filter(|d| d.is_absolute());
    let node = path_dir
        .map(|d| d.join(exe_name("node")))
        .filter(|n| n.is_file())
        .or_else(|| Some(install.join(exe_name("node"))).filter(|n| n.is_file()))
        .or_else(|| find_on_path("node", env.path_var.as_ref(), env.pathext.as_deref()))?;
    Some(ClaudeCommand {
        display: format!("{} {}", node.display(), script.display()),
        program: node,
        prefix_args: vec![script.into_os_string()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn dsh_shims_resolve_to_node_and_bin_js() {
        let tmp = tempfile::tempdir().unwrap();
        let npm = tmp.path().join("npm");
        let install = tmp.path().join("Programs").join("DeepSeekHarness");
        let node_dir = install.join("node-v24");
        let bin = install
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib");
        fs::create_dir_all(&npm).unwrap();
        fs::create_dir_all(&node_dir).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("bin.js"), "").unwrap();
        fs::write(node_dir.join(exe_name("node")), "").unwrap();
        let shim = npm.join("dsh.cmd");
        fs::write(
            &shim,
            format!(
                "@echo off\r\nsetlocal\r\nset \"PATH={};%PATH%\"\r\ncall \"{}\" %*\r\n",
                node_dir.display(),
                install.join("dsh.cmd").display()
            ),
        )
        .unwrap();
        let env = ResolveEnv::default();
        let cmd = dsh_shim(&shim, &env).expect("resolved");
        assert_eq!(cmd.program, node_dir.join(exe_name("node")));
        assert_eq!(cmd.prefix_args, vec![bin.join("bin.js").into_os_string()]);
        // The npm form: bin.js beside the shim, node beside it.
        let npm2 = tmp.path().join("npm2");
        let bin2 = npm2
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib");
        fs::create_dir_all(&bin2).unwrap();
        fs::write(bin2.join("bin.js"), "").unwrap();
        fs::write(npm2.join(exe_name("node")), "").unwrap();
        fs::write(npm2.join("dsh.cmd"), "@echo off").unwrap();
        let cmd = dsh_shim(&npm2.join("dsh.cmd"), &env).expect("npm shim");
        assert_eq!(cmd.program, npm2.join(exe_name("node")));
        // Not a dsh shim.
        fs::write(npm.join("other.cmd"), "@echo off").unwrap();
        assert!(dsh_shim(&npm.join("other.cmd"), &env).is_none());
    }

    #[test]
    fn names_are_found_on_path_and_shims_resolve_to_node() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let env = ResolveEnv {
            path_var: Some(dir.as_os_str().to_owned()),
            pathext: Some(".EXE;.CMD".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_program("codex", &env).unwrap_err().code,
            "not_found"
        );
        let shim = dir.join("codex.cmd");
        fs::write(&shim, "").unwrap();
        assert_eq!(resolve_program("codex", &env).unwrap().program, shim);
        // A claude package next to it is never used for codex.
        let claude_pkg = dir
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code");
        fs::create_dir_all(&claude_pkg).unwrap();
        fs::write(claude_pkg.join("cli.js"), "").unwrap();
        let bin = dir
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("codex.js"), "").unwrap();
        fs::write(dir.join(exe_name("node")), "").unwrap();
        let cmd = resolve_program("codex", &env).unwrap();
        assert_eq!(cmd.program, dir.join(exe_name("node")));
        assert_eq!(cmd.prefix_args, vec![bin.join("codex.js").into_os_string()]);
        let exe = dir.join("my tool.exe");
        fs::write(&exe, "").unwrap();
        assert_eq!(
            resolve_program(&exe.display().to_string(), &env)
                .unwrap()
                .program,
            exe
        );
        assert_eq!(
            resolve_program(r"\\host\x\a.exe", &env).unwrap_err().code,
            "invalid_input"
        );
    }
}
