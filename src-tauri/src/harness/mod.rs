//! Harness profiles (docs/ARCHITECTURE.md "Harness profiles"): which
//! coding-agent CLI a session runs — Claude Code, Claude Code against
//! DeepSeek, OpenAI Codex CLI, or a custom command — and the adapters that
//! start and read each kind.
//!
//! - [`profile`]: types, built-ins, validation.
//! - [`store`]: `harnesses.json` in the app data folder (machine-local).
//! - [`env`]: `${VAR}` substitution at spawn time.
//! - [`command`]: finding the program.
//! - [`adapter`]: spawn spec and behavior; [`claude`], [`codex`], [`custom`].

pub mod adapter;
pub mod claude;
pub mod codex;
pub mod command;
pub mod custom;
pub mod dsh;
pub mod env;
pub mod headless;
pub mod profile;
pub mod store;

use std::time::Duration;

use crate::claude_bin::{resolve, ClaudeCommand, Resolution, ResolveEnv};
use crate::contract::{
    AppError, AppResult, Graph, HarnessCheck, HarnessKind, HarnessProfile, Node, Settings,
};

/// `check_harness` gives up after this long.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// The harness id a node runs with: its own, else the graph's.
pub fn effective<'a>(node: &'a Node, settings: &'a Settings) -> &'a str {
    node.harness
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| settings.harness.trim())
}

pub fn find<'a>(profiles: &'a [HarnessProfile], id: &str) -> Option<&'a HarnessProfile> {
    profiles.iter().find(|p| p.id == id)
}

fn undefined(node: &str, id: &str) -> AppError {
    AppError::new(
        "invalid_input",
        format!(
            "node {node}: harness {id} is not defined on this machine; add it under \
             Settings \u{2192} Harnesses"
        ),
    )
}

/// Every node of `targets` names a harness defined in `profiles`; with
/// `headless`, a claude-kind one (the `claude -p` engine drives Claude Code only).
pub fn check_targets<'a>(
    graph: &Graph,
    targets: impl IntoIterator<Item = &'a String>,
    profiles: &[HarnessProfile],
    headless: bool,
) -> AppResult<()> {
    for id in targets {
        let Some(node) = graph.nodes.iter().find(|n| &n.id == id) else {
            continue;
        };
        let h = effective(node, &graph.settings);
        let profile = find(profiles, h).ok_or_else(|| undefined(id, h))?;
        if headless && profile.kind != HarnessKind::Claude {
            return Err(AppError::new(
                "invalid_input",
                format!(
                    "node {id}: harness {h} ({}) runs in terminal mode only; switch the \
                     graph to terminal mode or pick a Claude Code harness",
                    profile.label
                ),
            ));
        }
    }
    Ok(())
}

/// `check_harness`: `<program> --version` with the profile's env (never
/// printed), [`CHECK_TIMEOUT`]. `claude_bin` is the app settings' value.
pub async fn check(profile: &HarnessProfile, claude_bin: Option<String>) -> HarnessCheck {
    let fail = |message: String| HarnessCheck {
        id: profile.id.clone(),
        ok: false,
        version: None,
        message: Some(message),
    };
    let env = match env::process_env(profile) {
        Ok(e) => e,
        Err(e) => return fail(e.message),
    };
    let resolve_env = ResolveEnv::from_process(claude_bin);
    let claude = match resolve(&resolve_env) {
        Resolution::Found(c) => Some(c),
        Resolution::Missing { .. } => None,
    };
    let cmd: ClaudeCommand = match command::program_of(profile, claude.as_ref(), &resolve_env) {
        Ok(c) => c,
        Err(e) => return fail(e.message),
    };
    let mut command = cmd.command();
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in env {
        if let Some(v) = v {
            command.env(k, v);
        }
    }
    let output = match tokio::time::timeout(CHECK_TIMEOUT, command.output()).await {
        Err(_) => return fail(format!("{} --version did not answer in 10 s", cmd.display)),
        Ok(Err(e)) => return fail(format!("could not start {}: {e}", cmd.display)),
        Ok(Ok(o)) => o,
    };
    let first = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
    };
    if output.status.success() {
        HarnessCheck {
            id: profile.id.clone(),
            ok: true,
            version: first(&output.stdout),
            message: Some(cmd.display),
        }
    } else {
        let why = first(&output.stderr).unwrap_or_else(|| format!("{}", output.status));
        fail(format!("{} --version failed: {why}", cmd.display))
    }
}

#[cfg(test)]
mod tests;
