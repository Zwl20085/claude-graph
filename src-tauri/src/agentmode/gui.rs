//! Tauri commands of the agent-called mode (registered in lib.rs):
//! `agent_watch`, `agent_abandon`, `agent_client_configs`, `agent_install`.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::contract::{AppError, AppResult};
use crate::contract_agent::{AgentClientConfig, AgentRunInfo, EVENT_AGENT_RUN};
use crate::events::{Emitter, TauriEmitter};
use crate::paths::existing_dir;
use crate::run_loop::blocking;

use super::watch::{AgentWatch, Sink, Update};
use super::{install, open, run};

struct TauriSink(AppHandle);

impl Sink for TauriSink {
    fn update(&self, update: &Update) {
        use tauri::Emitter as _;
        let emitter = TauriEmitter(self.0.clone());
        if let Some(state) = &update.state {
            emitter.run_state(state);
        }
        for e in &update.events {
            emitter.node_event(e);
        }
        if let Err(e) = self.0.emit(EVENT_AGENT_RUN, &update.info) {
            log::warn!("emit {EVENT_AGENT_RUN} failed: {e}");
        }
    }
}

fn exe() -> AppResult<PathBuf> {
    std::env::current_exe().map_err(|e| AppError::new("io", format!("app path: {e}")))
}

/// Watch `dir` for agent runs (`null`: stop); returns its newest agent run.
#[tauri::command(rename_all = "snake_case")]
pub async fn agent_watch(
    app: AppHandle,
    watch: State<'_, AgentWatch>,
    dir: Option<String>,
) -> AppResult<Option<AgentRunInfo>> {
    let watch = watch.inner().clone();
    blocking(move || {
        let dir = dir
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .map(existing_dir)
            .transpose()?;
        Ok(watch.watch(dir, Arc::new(TauriSink(app))))
    })
    .await
}

/// The GUI's Stop for an agent run: the run is marked cancelled.
#[tauri::command(rename_all = "snake_case")]
pub async fn agent_abandon(dir: String, run_id: String) -> AppResult<()> {
    blocking(move || open::abandon(&existing_dir(&dir)?, &run_id).map(|_| ())).await
}

/// Snippets for Claude Code, Codex and dsh, with whether each has the server.
#[tauri::command(rename_all = "snake_case")]
pub async fn agent_client_configs() -> AppResult<Vec<AgentClientConfig>> {
    Ok(install::configs(&exe()?).await)
}

/// "Add to <client>" (the UI asked the user first, showing the command or file).
#[tauri::command(rename_all = "snake_case")]
pub async fn agent_install(client: String) -> AppResult<String> {
    install::install(&client, &exe()?).await
}

/// `run_active` when an agent run is active in `dir` (a run from the app must wait).
pub fn refuse_if_agent_active(dir: &std::path::Path) -> AppResult<()> {
    match run::active(dir) {
        Some(r) => Err(AppError::new(
            "run_active",
            format!(
                "an agent run ({}, driven by {} in a CLI) is active in this folder; stop it first",
                r.state.run_id, r.meta.client
            ),
        )),
        None => Ok(()),
    }
}
