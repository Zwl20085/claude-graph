//! Settings → "Use from your CLI": how to register `claude-graph
//! --mcp-graph` with each client, and the "Add" action the UI runs only
//! after the user confirmed the exact command or file shown.
//!
//! Verified on this machine (2026-11-08, docs/cli-addon.md):
//! - Claude Code 2.1.319: `claude mcp add --scope user claude-graph -- "<exe>" --mcp-graph`.
//! - Codex CLI 0.167.0: `codex mcp add claude_graph -- "<exe>" --mcp-graph`
//!   (writes `[mcp_servers.claude_graph]` to `~/.codex/config.toml`).
//! - dsh 0.2.1-rc.2: a `@deepseek-ai/dsh-mcp-client` entry in the home-level
//!   patch file `~/.dsh/cordis.patch.yml` (a YAML list; `[]` when empty).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::claude_bin::{resolve, ClaudeCommand, Resolution, ResolveEnv};
use crate::contract::{AppError, AppResult};
use crate::contract_agent::AgentClientConfig;
use crate::store::{read_optional, write_atomic};

use super::FLAG_MCP_GRAPH;

/// Server name for Claude Code and dsh (tools `mcp__claude-graph__…`). Not
/// `claude_graph`: that is the app's own per-turn approval server.
pub const SERVER_NAME: &str = "claude-graph";
/// Server name for Codex (`[mcp_servers.claude_graph]`).
pub const CODEX_SERVER_NAME: &str = "claude_graph";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);
/// Marker line of the dsh entry (detects an existing registration).
const DSH_MARKER: &str = "serverName: claude-graph";

fn quoted(p: &Path) -> String {
    format!("\"{}\"", p.display())
}

pub fn claude_command(exe: &Path) -> String {
    format!(
        "claude mcp add --scope user {SERVER_NAME} -- {} {FLAG_MCP_GRAPH}",
        quoted(exe)
    )
}

pub fn codex_command(exe: &Path) -> String {
    format!(
        "codex mcp add {CODEX_SERVER_NAME} -- {} {FLAG_MCP_GRAPH}",
        quoted(exe)
    )
}

/// TOML for `~/.codex/config.toml` (single-quoted literal: backslashes stay).
pub fn codex_toml(exe: &Path) -> String {
    format!(
        "[mcp_servers.{CODEX_SERVER_NAME}]\ncommand = '{}'\nargs = [\"{FLAG_MCP_GRAPH}\"]\n",
        exe.display()
    )
}

/// One dsh patch-list entry (YAML).
pub fn dsh_entry(exe: &Path) -> String {
    format!(
        "- insert:\n    - id: mcp-claude-graph\n      name: '@deepseek-ai/dsh-mcp-client'\n      \
         config:\n        {DSH_MARKER}\n        transport: stdio\n        command: '{}'\n        \
         args: ['{FLAG_MCP_GRAPH}']\n",
        exe.display().to_string().replace('\'', "''")
    )
}

/// `.mcp.json` for a project (Claude Code).
pub fn mcp_json(exe: &Path) -> String {
    let v = serde_json::json!({ "mcpServers": { SERVER_NAME: {
        "command": exe.display().to_string(), "args": [FLAG_MCP_GRAPH]
    }}});
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

fn dsh_patch_file() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".dsh").join("cordis.patch.yml"))
}

fn codex_config_file() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("config.toml"))
}

/// The dsh patch file with our entry added; `Err` when its shape is unknown
/// (the user pastes the entry by hand then).
pub fn dsh_patched(current: Option<&str>, exe: &Path) -> Result<String, String> {
    let entry = dsh_entry(exe);
    let text = current.unwrap_or_default();
    let body = text.trim();
    if body.is_empty() || body == "[]" {
        return Ok(entry);
    }
    if text.contains(DSH_MARKER) {
        return Err("the dsh patch file already has a claude-graph entry".into());
    }
    let block_list = text
        .lines()
        .map(str::trim_end)
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .is_some_and(|l| l.starts_with("- "));
    if !block_list {
        return Err("the dsh patch file is not a plain YAML list; add the entry by hand".into());
    }
    let sep = if text.ends_with('\n') { "" } else { "\n" };
    Ok(format!("{text}{sep}{entry}"))
}

fn contains(path: Option<PathBuf>, needle: &str) -> bool {
    path.and_then(|p| read_optional(&p).ok().flatten())
        .is_some_and(|t| t.contains(needle))
}

fn claude() -> Option<ClaudeCommand> {
    match resolve(&ResolveEnv::from_process(None)) {
        Resolution::Found(c) => Some(c),
        Resolution::Missing { .. } => None,
    }
}

fn codex() -> Option<ClaudeCommand> {
    crate::harness::command::default_codex(&ResolveEnv::from_process(None)).ok()
}

async fn run(cmd: Option<ClaudeCommand>, name: &str, args: &[&str]) -> AppResult<(bool, String)> {
    let cmd = cmd.ok_or_else(|| AppError::new("not_found", format!("{name} was not found")))?;
    let mut command = cmd.command();
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let out = tokio::time::timeout(CLIENT_TIMEOUT, command.output())
        .await
        .map_err(|_| AppError::new("timeout", format!("{name} did not answer")))?
        .map_err(|e| AppError::new("io", format!("{name}: {e}")))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok((out.status.success(), text.trim().to_string()))
}

/// The three clients' snippets and whether each already has the server.
pub async fn configs(exe: &Path) -> Vec<AgentClientConfig> {
    let claude_has = matches!(
        run(claude(), "claude", &["mcp", "get", SERVER_NAME]).await,
        Ok((true, _))
    );
    let codex_has = contains(
        codex_config_file(),
        &format!("[mcp_servers.{CODEX_SERVER_NAME}]"),
    );
    let dsh_has = contains(dsh_patch_file(), DSH_MARKER);
    let dsh_path = dsh_patch_file().map(|p| p.display().to_string());
    vec![
        AgentClientConfig {
            id: "claude".into(),
            name: "Claude Code".into(),
            snippet: format!(
                "{}\n\n# or, per project, .mcp.json:\n{}",
                claude_command(exe),
                mcp_json(exe)
            ),
            config_path: None,
            add_action: format!("run: {}", claude_command(exe)),
            installed: claude_has,
        },
        AgentClientConfig {
            id: "codex".into(),
            name: "Codex".into(),
            snippet: format!(
                "{}\n\n# or add to ~/.codex/config.toml:\n{}",
                codex_command(exe),
                codex_toml(exe)
            ),
            config_path: None,
            add_action: format!("run: {}", codex_command(exe)),
            installed: codex_has,
        },
        AgentClientConfig {
            id: "dsh".into(),
            name: "DeepSeek Harness (dsh)".into(),
            snippet: format!(
                "# ~/.dsh/cordis.patch.yml (all profiles), or a file passed with --patch:\n{}",
                dsh_entry(exe)
            ),
            add_action: format!(
                "add the entry to {}",
                dsh_path.as_deref().unwrap_or("~/.dsh/cordis.patch.yml")
            ),
            config_path: dsh_path,
            installed: dsh_has,
        },
    ]
}

/// "Add to <client>", after the UI's confirmation. Returns what was done.
pub async fn install(client: &str, exe: &Path) -> AppResult<String> {
    let exe_s = exe.display().to_string();
    match client {
        "claude" => {
            let args = [
                "mcp",
                "add",
                "--scope",
                "user",
                SERVER_NAME,
                "--",
                &exe_s,
                FLAG_MCP_GRAPH,
            ];
            let (ok, out) = run(claude(), "claude", &args).await?;
            done(ok, out)
        }
        "codex" => {
            let args = [
                "mcp",
                "add",
                CODEX_SERVER_NAME,
                "--",
                &exe_s,
                FLAG_MCP_GRAPH,
            ];
            let (ok, out) = run(codex(), "codex", &args).await?;
            done(ok, out)
        }
        "dsh" => {
            let path =
                dsh_patch_file().ok_or_else(|| AppError::new("not_found", "no home folder"))?;
            let current = read_optional(&path)?;
            let text = dsh_patched(current.as_deref(), exe)
                .map_err(|e| AppError::new("invalid_input", e))?;
            write_atomic(&path, text.as_bytes())?;
            Ok(format!(
                "added the claude-graph entry to {}",
                path.display()
            ))
        }
        other => Err(AppError::new(
            "invalid_input",
            format!("unknown client {other:?}"),
        )),
    }
}

fn done(ok: bool, out: String) -> AppResult<String> {
    if ok {
        Ok(out)
    } else {
        Err(AppError::new("client_failed", out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_quote_the_exe() {
        let exe = Path::new(r"C:\Program Files\Claude Graph\claude-graph.exe");
        assert_eq!(
            claude_command(exe),
            r#"claude mcp add --scope user claude-graph -- "C:\Program Files\Claude Graph\claude-graph.exe" --mcp-graph"#
        );
        assert!(
            codex_toml(exe).contains(r"command = 'C:\Program Files\Claude Graph\claude-graph.exe'")
        );
        let v: serde_json::Value = serde_json::from_str(&mcp_json(exe)).unwrap();
        assert_eq!(v["mcpServers"]["claude-graph"]["args"][0], "--mcp-graph");
        assert!(dsh_entry(Path::new("C:/it's/x.exe")).contains("command: 'C:/it''s/x.exe'"));
    }

    #[test]
    fn dsh_patch_file_shapes() {
        let exe = Path::new("C:/x.exe");
        assert_eq!(dsh_patched(None, exe).unwrap(), dsh_entry(exe));
        assert_eq!(dsh_patched(Some(" []\n"), exe).unwrap(), dsh_entry(exe));
        let list = "# mine\n- insert:\n    - id: other\n";
        let patched = dsh_patched(Some(list), exe).unwrap();
        assert!(patched.starts_with(list) && patched.ends_with(&dsh_entry(exe)));
        assert!(dsh_patched(Some(&patched), exe).is_err());
        assert!(dsh_patched(Some("foo: bar\n"), exe).is_err());
    }
}
