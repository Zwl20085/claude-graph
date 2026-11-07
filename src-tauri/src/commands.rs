//! Tauri command handlers: the IPC surface of docs/ARCHITECTURE.md §6.
//!
//! Every command uses `rename_all = "snake_case"` so the frontend passes
//! argument names exactly as written in the brief (`run_id`, `node_id`, …);
//! Tauri's default would expect camelCase.
//!
//! Only `check_claude` and `list_recent` are real; the rest return
//! `AppError { code: "not_implemented" }` until the backend engineer fills them in.

use crate::claude_bin;
use crate::contract::{
    AppError, AppResult, ClaudeCheck, Graph, NodeResult, RunState, RunSummary, RunTarget,
};

/// `claude_bin` is optional and may be omitted by the frontend (see ARCHITECTURE.md §6).
#[tauri::command(rename_all = "snake_case")]
pub async fn check_claude(claude_bin: Option<String>) -> AppResult<ClaudeCheck> {
    Ok(claude_bin::check(claude_bin).await)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_graph(dir: String, name: String) -> AppResult<Graph> {
    let _ = (dir, name);
    Err(AppError::not_implemented("create_graph"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_graph(dir: String) -> AppResult<Graph> {
    let _ = dir;
    Err(AppError::not_implemented("open_graph"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_graph(dir: String, graph: Graph) -> AppResult<()> {
    let _ = (dir, graph);
    Err(AppError::not_implemented("save_graph"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_recent() -> AppResult<Vec<String>> {
    Ok(Vec::new())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pick_folder(title: String) -> AppResult<Option<String>> {
    let _ = title;
    Err(AppError::not_implemented("pick_folder"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_run(dir: String, target: RunTarget) -> AppResult<String> {
    let _ = (dir, target);
    Err(AppError::not_implemented("start_run"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_run(run_id: String) -> AppResult<()> {
    let _ = run_id;
    Err(AppError::not_implemented("cancel_run"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_node(run_id: String, node_id: String) -> AppResult<()> {
    let _ = (run_id, node_id);
    Err(AppError::not_implemented("cancel_node"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_run(dir: String, run_id: String) -> AppResult<RunState> {
    let _ = (dir, run_id);
    Err(AppError::not_implemented("get_run"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_runs(dir: String) -> AppResult<Vec<RunSummary>> {
    let _ = dir;
    Err(AppError::not_implemented("list_runs"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_node_result(
    dir: String,
    run_id: String,
    node_id: String,
) -> AppResult<NodeResult> {
    let _ = (dir, run_id, node_id);
    Err(AppError::not_implemented("read_node_result"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn render_brief(graph: Graph, node_id: String) -> AppResult<String> {
    let _ = (graph, node_id);
    Err(AppError::not_implemented("render_brief"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_path(path: String) -> AppResult<()> {
    let _ = path;
    Err(AppError::not_implemented("open_path"))
}
