//! Tauri command handlers: the IPC surface of docs/ARCHITECTURE.md §6.
//!
//! Every command uses `rename_all = "snake_case"` so the frontend passes
//! argument names exactly as written in the brief (`run_id`, `node_id`, …);
//! Tauri's default would expect camelCase. The logic lives in `store`,
//! `run_store`, `run_api` and `prompt`; these wrappers only adapt arguments.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::claude_bin;
use crate::contract::{
    AppError, AppResult, ClaudeCheck, Graph, NodeMessage, NodeResult, RunState, RunSummary,
    RunTarget,
};
use crate::events::{Emitter, TauriEmitter};
use crate::prompt::{render_brief as render, upstream_placeholders, PREVIEW_DIR};
use crate::run_api::AppState;
use crate::{run_store, store};

fn emitter(app: &AppHandle) -> Arc<dyn Emitter> {
    Arc::new(TauriEmitter(app.clone()))
}

fn graph_dir(dir: &str) -> AppResult<PathBuf> {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return Err(AppError::new("not_found", "no graph folder given"));
    }
    Ok(PathBuf::from(trimmed))
}

fn remember(dir: &Path) {
    if let Some(file) = store::default_recent_file() {
        if let Err(e) = store::push_recent(&file, dir) {
            log::warn!("could not update the recent list: {e}");
        }
    }
}

/// `claude_bin` is optional and may be omitted by the frontend (see ARCHITECTURE.md §6).
#[tauri::command(rename_all = "snake_case")]
pub async fn check_claude(claude_bin: Option<String>) -> AppResult<ClaudeCheck> {
    Ok(claude_bin::check(claude_bin).await)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_graph(dir: String, name: String) -> AppResult<Graph> {
    let dir = graph_dir(&dir)?;
    let graph = store::create_graph(&dir, &name)?;
    remember(&dir);
    Ok(graph)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_graph(dir: String) -> AppResult<Graph> {
    let dir = graph_dir(&dir)?;
    let graph = store::read_graph(&dir)?;
    remember(&dir);
    Ok(graph)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_graph(dir: String, graph: Graph) -> AppResult<()> {
    store::save_graph(&graph_dir(&dir)?, &graph)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_recent() -> AppResult<Vec<String>> {
    Ok(store::default_recent_file()
        .map(|f| store::load_recent(&f))
        .unwrap_or_default())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pick_folder(app: AppHandle, title: String) -> AppResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(title)
        .pick_folder(move |picked| {
            let _ = tx.send(picked.map(|p| p.to_string()));
        });
    rx.await
        .map_err(|_| AppError::new("internal", "folder dialog closed unexpectedly"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_run(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    target: RunTarget,
) -> AppResult<String> {
    let (run_id, _join) = state.start_run(&graph_dir(&dir)?, target, emitter(&app))?;
    Ok(run_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_run(state: State<'_, AppState>, run_id: String) -> AppResult<()> {
    state.cancel_run(&run_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_node(
    state: State<'_, AppState>,
    run_id: String,
    node_id: String,
) -> AppResult<()> {
    state.cancel_node(&run_id, &node_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_run(dir: String, run_id: String) -> AppResult<RunState> {
    run_store::read_run_state(&graph_dir(&dir)?, &run_id)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_runs(dir: String) -> AppResult<Vec<RunSummary>> {
    Ok(run_store::list_run_states(&graph_dir(&dir)?)?
        .iter()
        .map(run_store::summarize_run)
        .collect())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_node_result(
    dir: String,
    run_id: String,
    node_id: String,
) -> AppResult<NodeResult> {
    run_store::read_node_result(&graph_dir(&dir)?, &run_id, &node_id)
}

/// Preview without a folder: paths use the `<graph folder>` placeholder and
/// upstream inputs are shown as "not run yet".
#[tauri::command(rename_all = "snake_case")]
pub async fn render_brief(graph: Graph, node_id: String) -> AppResult<String> {
    let node = graph
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .ok_or_else(|| AppError::new("not_found", format!("no node {node_id:?}")))?;
    let upstream = upstream_placeholders(PREVIEW_DIR, &graph, node);
    Ok(render(PREVIEW_DIR, &graph, node, &upstream))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_path(app: AppHandle, path: String) -> AppResult<()> {
    if !Path::new(&path).exists() {
        return Err(AppError::new("not_found", format!("{path} does not exist")));
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| AppError::new("io", e.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    run_id: String,
    node_id: String,
    text: String,
) -> AppResult<()> {
    state
        .send_message(&graph_dir(&dir)?, &run_id, &node_id, &text, emitter(&app))
        .await
        .map(|_| ())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_messages(
    dir: String,
    run_id: String,
    node_id: String,
) -> AppResult<Vec<NodeMessage>> {
    run_store::list_messages(&graph_dir(&dir)?, &run_id, &node_id)
}
