//! Tauri command handlers: the IPC surface of docs/ARCHITECTURE.md §6.
//!
//! Every command uses `rename_all = "snake_case"` so the frontend passes
//! argument names exactly as written in the brief (`run_id`, `node_id`, …);
//! Tauri's default would expect camelCase. The logic lives in `store`,
//! `run_store`, `run_api`, `trust` and `prompt`; these wrappers only adapt
//! arguments. Every `dir` is checked by [`crate::paths`] (absolute, local, no
//! UNC, canonicalised) and file I/O runs on the blocking pool.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::claude_bin;
use crate::contract::{
    AppError, AppResult, AppSettings, AssistantState, ClaudeCheck, Graph, HarnessCheck,
    HarnessProfile, LaunchOptions, NodeMessage, NodeResult, PtyReplay, RunState, RunSummary,
    RunTarget, TerminalSession, TrustStatus,
};
use crate::events::{Emitter, TauriEmitter};
use crate::paths::{canonical, dir_for_create, existing_dir};
use crate::prompt::{render_brief as render, upstream_placeholders, PREVIEW_DIR};
use crate::run_api::AppState;
use crate::run_loop::blocking;
use crate::run_store::check_id;
use crate::terminal::api as term;
use crate::{harness, launch, run_store, store};

fn emitter(app: &AppHandle) -> Arc<dyn Emitter> {
    Arc::new(TauriEmitter(app.clone()))
}

fn remember(dir: &Path) {
    if let Some(file) = store::default_recent_file() {
        if let Err(e) = store::push_recent(&file, dir) {
            log::warn!("could not update the recent list: {e}");
        }
    }
}

/// Sweep runs left `running` by an earlier app exit or crash; never fails the caller.
fn sweep(state: &AppState, dir: &Path) {
    match state.sweep_stale(dir) {
        Ok(swept) if !swept.is_empty() => {
            log::info!("marked interrupted runs: {}", swept.join(", "))
        }
        Ok(_) => {}
        Err(e) => log::warn!("stale-run sweep of {} failed: {e}", dir.display()),
    }
}

/// Uses the app settings, env `CLAUDE_GRAPH_CLAUDE_BIN`, `~/.local/bin`, PATH.
#[tauri::command(rename_all = "snake_case")]
pub async fn check_claude(state: State<'_, AppState>) -> AppResult<ClaudeCheck> {
    let st = state.inner().clone();
    let configured = blocking(move || Ok(st.app_settings().claude_bin)).await?;
    Ok(claude_bin::check(configured).await)
}

/// Harness profiles: built-ins (with overrides), then customs.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_harnesses(state: State<'_, AppState>) -> AppResult<Vec<HarnessProfile>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.harnesses())).await
}

/// Validate and store a profile (a built-in id stores an override of its
/// command, args, env and model); returns the new list.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_harness(
    state: State<'_, AppState>,
    profile: HarnessProfile,
) -> AppResult<Vec<HarnessProfile>> {
    let st = state.inner().clone();
    blocking(move || st.save_harness(&profile)).await
}

/// Delete a custom profile; returns the new list.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_harness(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<Vec<HarnessProfile>> {
    checked_harness_id(&id)?;
    let st = state.inner().clone();
    blocking(move || st.delete_harness(&id)).await
}

/// `<command> --version` with the profile's env (10 s timeout).
#[tauri::command(rename_all = "snake_case")]
pub async fn check_harness(state: State<'_, AppState>, id: String) -> AppResult<HarnessCheck> {
    checked_harness_id(&id)?;
    let st = state.inner().clone();
    let (list, claude_bin) =
        blocking(move || Ok((st.harnesses(), st.app_settings().claude_bin))).await?;
    let profile = harness::find(&list, &id)
        .cloned()
        .ok_or_else(|| AppError::new("not_found", format!("no harness {id:?}")))?;
    Ok(harness::check(&profile, claude_bin).await)
}

fn checked_harness_id(id: &str) -> AppResult<()> {
    if harness::profile::valid_id(id) {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_input",
            format!("invalid harness id {id:?}"),
        ))
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_app_settings(state: State<'_, AppState>) -> AppResult<AppSettings> {
    let st = state.inner().clone();
    blocking(move || Ok(st.app_settings())).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_app_settings(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> AppResult<AppSettings> {
    let st = state.inner().clone();
    blocking(move || st.set_app_settings(&settings)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_graph(dir: String, name: String) -> AppResult<Graph> {
    blocking(move || {
        let dir = dir_for_create(&dir)?;
        let graph = store::create_graph(&dir, &name)?;
        remember(&dir);
        Ok(graph)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_graph(state: State<'_, AppState>, dir: String) -> AppResult<Graph> {
    let st = state.inner().clone();
    blocking(move || {
        let dir = existing_dir(&dir)?;
        let graph = store::read_graph(&dir)?;
        sweep(&st, &dir);
        remember(&dir);
        Ok(graph)
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_graph(dir: String, graph: Graph) -> AppResult<()> {
    blocking(move || store::save_graph(&existing_dir(&dir)?, &graph)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_recent() -> AppResult<Vec<String>> {
    blocking(|| {
        Ok(store::default_recent_file()
            .map(|f| store::load_recent(&f))
            .unwrap_or_default())
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pick_folder(app: AppHandle, title: String) -> AppResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(title)
        .pick_folder(move |picked| {
            if tx.send(picked.map(|p| p.to_string())).is_err() {
                log::warn!("folder dialog answered after the command gave up");
            }
        });
    rx.await
        .map_err(|_| AppError::new("internal", "folder dialog closed unexpectedly"))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn graph_trust_status(state: State<'_, AppState>, dir: String) -> AppResult<TrustStatus> {
    let st = state.inner().clone();
    blocking(move || st.trust_status(&existing_dir(&dir)?)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn trust_graph(state: State<'_, AppState>, dir: String) -> AppResult<()> {
    let st = state.inner().clone();
    blocking(move || st.trust_graph(&existing_dir(&dir)?)).await
}

/// `untrusted_graph` when the graph's elevated settings were not accepted.
#[tauri::command(rename_all = "snake_case")]
pub async fn start_run(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    target: RunTarget,
) -> AppResult<String> {
    let st = state.inner().clone();
    let em = emitter(&app);
    blocking(move || {
        let (run_id, _join) = st.start_by_mode(&existing_dir(&dir)?, target, em)?;
        Ok(run_id)
    })
    .await
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
    blocking(move || run_store::read_run_state(&existing_dir(&dir)?, &run_id)).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_runs(state: State<'_, AppState>, dir: String) -> AppResult<Vec<RunSummary>> {
    let st = state.inner().clone();
    blocking(move || {
        let dir = existing_dir(&dir)?;
        sweep(&st, &dir);
        Ok(run_store::list_run_states(&dir)?
            .iter()
            .map(run_store::summarize_run)
            .collect())
    })
    .await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_node_result(
    dir: String,
    run_id: String,
    node_id: String,
) -> AppResult<NodeResult> {
    blocking(move || run_store::read_node_result(&existing_dir(&dir)?, &run_id, &node_id)).await
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

/// The canonical path to reveal: `<dir>/graph.json`, or a node's deliverables
/// folder (created if missing; the node must exist in the saved graph). The
/// result must lie inside the canonical graph folder.
pub fn reveal_target(dir: &str, node_id: Option<&str>) -> AppResult<PathBuf> {
    let root = existing_dir(dir)?;
    let target = match node_id {
        None => root.join(store::GRAPH_FILE),
        Some(id) => {
            check_id("node id", id)?;
            let graph = store::read_graph(&root)?;
            if !graph.nodes.iter().any(|n| n.id == id) {
                return Err(AppError::new("not_found", format!("no node {id:?}")));
            }
            let d = store::deliverables_dir(&root, id);
            std::fs::create_dir_all(&d)?;
            d
        }
    };
    let canon = canonical(&target)?;
    if !canon.starts_with(&root) {
        return Err(AppError::new(
            "invalid_input",
            format!("{} is outside the graph folder", canon.display()),
        ));
    }
    Ok(canon)
}

fn reveal(app: &AppHandle, path: &Path) -> AppResult<()> {
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| AppError::new("io", e.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reveal_graph_dir(app: AppHandle, dir: String) -> AppResult<()> {
    let path = blocking(move || reveal_target(&dir, None)).await?;
    reveal(&app, &path)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reveal_node_dir(app: AppHandle, dir: String, node_id: String) -> AppResult<()> {
    let path = blocking(move || reveal_target(&dir, Some(&node_id))).await?;
    reveal(&app, &path)
}

/// `untrusted_graph` when the graph's elevated settings were not accepted.
#[tauri::command(rename_all = "snake_case")]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    run_id: String,
    node_id: String,
    text: String,
) -> AppResult<()> {
    let dir = blocking(move || existing_dir(&dir)).await?;
    state
        .send_message(&dir, &run_id, &node_id, &text, emitter(&app))
        .await
        .map(|_| ())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_messages(
    dir: String,
    run_id: String,
    node_id: String,
) -> AppResult<Vec<NodeMessage>> {
    blocking(move || run_store::list_messages(&existing_dir(&dir)?, &run_id, &node_id)).await
}

/// Startup request from argv (`claude-graph <dir> [--run]`) or env
/// (`CLAUDE_GRAPH_OPEN`, `CLAUDE_GRAPH_AUTORUN`); see ARCHITECTURE.md "Launching".
#[tauri::command(rename_all = "snake_case")]
pub async fn launch_options() -> AppResult<LaunchOptions> {
    Ok(launch::from_process())
}

/// Assistant mode: session and journal of the graph (empty idle state when
/// `assistant/` is missing); sweeps a session left `working` by a crash.
#[tauri::command(rename_all = "snake_case")]
pub async fn assistant_state(state: State<'_, AppState>, dir: String) -> AppResult<AssistantState> {
    let st = state.inner().clone();
    blocking(move || st.assistant_state(&existing_dir(&dir)?)).await
}

/// `invalid_input` for blank text, `assistant_busy` while a turn runs. The
/// workflow is read from graph.json on disk (the frontend saves first).
#[tauri::command(rename_all = "snake_case")]
pub async fn assistant_send(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    text: String,
) -> AppResult<AssistantState> {
    let st = state.inner().clone();
    let em = emitter(&app);
    blocking(move || st.assistant_send(&existing_dir(&dir)?, &text, em)).await
}

/// `not_found` when no turn runs.
#[tauri::command(rename_all = "snake_case")]
pub async fn assistant_cancel(state: State<'_, AppState>, dir: String) -> AppResult<()> {
    let st = state.inner().clone();
    blocking(move || st.assistant_cancel(&existing_dir(&dir)?)).await
}

/// `assistant_busy` while a turn runs.
#[tauri::command(rename_all = "snake_case")]
pub async fn assistant_reset(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> AppResult<AssistantState> {
    let st = state.inner().clone();
    let em = emitter(&app);
    blocking(move || st.assistant_reset(&existing_dir(&dir)?, em)).await
}

/// Terminal mode: the sessions of a run (live registry, else run.json in
/// `dir`). `dir` may be `""`: then only live sessions, `not_found` otherwise.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_sessions(
    state: State<'_, AppState>,
    dir: String,
    run_id: String,
) -> AppResult<Vec<TerminalSession>> {
    let st = state.inner().clone();
    blocking(move || {
        let dir = match dir.trim() {
            "" => None,
            d => Some(existing_dir(d)?),
        };
        term::list_sessions(st.terminals(), dir.as_deref(), &run_id)
    })
    .await
}

/// Terminal mode: raw text to the session's pty (keyboard, Messages tab).
#[tauri::command(rename_all = "snake_case")]
pub async fn pty_write(
    state: State<'_, AppState>,
    run_id: String,
    sid: String,
    text: String,
) -> AppResult<()> {
    term::pty_write(state.terminals(), &run_id, &sid, &text)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pty_resize(
    state: State<'_, AppState>,
    run_id: String,
    sid: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    term::pty_resize(state.terminals(), &run_id, &sid, cols, rows)
}

/// Terminal mode: the output so far (last 512 KiB) and its chunk count.
#[tauri::command(rename_all = "snake_case")]
pub async fn pty_replay(
    state: State<'_, AppState>,
    run_id: String,
    sid: String,
) -> AppResult<PtyReplay> {
    term::pty_replay(state.terminals(), &run_id, &sid)
}

/// Terminal mode: create (or focus) the session's window.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_session_window(
    app: AppHandle,
    state: State<'_, AppState>,
    run_id: String,
    sid: String,
) -> AppResult<()> {
    term::open_session_window(state.terminals(), &*emitter(&app), &run_id, &sid)
}

/// Terminal mode: answer the question or dialog a session waits on.
#[tauri::command(rename_all = "snake_case")]
pub async fn answer_prompt(
    state: State<'_, AppState>,
    run_id: String,
    sid: String,
    prompt_id: String,
    answer: crate::contract::PromptAnswer,
) -> AppResult<()> {
    crate::terminal::prompt_api::answer_prompt(state.terminals(), &run_id, &sid, &prompt_id, answer)
}

/// Terminal mode: a node waiting on a reply question is done without an answer.
#[tauri::command(rename_all = "snake_case")]
pub async fn continue_node(
    state: State<'_, AppState>,
    run_id: String,
    sid: String,
    node_id: String,
) -> AppResult<()> {
    crate::terminal::prompt_api::continue_node(state.terminals(), &run_id, &sid, &node_id)
}

/// Terminal mode: the prompts the live sessions of a run wait on.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_session_prompts(
    state: State<'_, AppState>,
    run_id: String,
) -> AppResult<Vec<crate::contract::SessionPrompt>> {
    crate::terminal::prompt_api::list_session_prompts(state.terminals(), &run_id)
}

/// "Ask me in the app": the prompts waiting for the user, oldest first.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_permission_prompts(
    state: State<'_, AppState>,
) -> AppResult<Vec<crate::contract::PermissionPrompt>> {
    Ok(state.approvals().list())
}

/// "Ask me in the app": answer one prompt (`not_found` when already answered).
#[tauri::command(rename_all = "snake_case")]
pub async fn answer_permission(
    state: State<'_, AppState>,
    answer: crate::contract::PermissionAnswer,
) -> AppResult<()> {
    state.approvals().answer(&answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_targets_stay_inside_the_graph() {
        let tmp = tempfile::tempdir().unwrap();
        let g: Graph =
            serde_json::from_str(include_str!("../../examples/hello-pipeline/graph.json")).unwrap();
        store::save_graph(tmp.path(), &g).unwrap();
        let dir = tmp.path().display().to_string();
        let root = crate::paths::abs(tmp.path());
        assert_eq!(
            reveal_target(&dir, None).unwrap(),
            root.join(store::GRAPH_FILE)
        );
        let node = reveal_target(&dir, Some("survey")).unwrap();
        assert!(node.starts_with(&root) && node.ends_with("deliverables"));
        assert_eq!(
            reveal_target(&dir, Some("../..")).unwrap_err().code,
            "not_found"
        );
        assert_eq!(
            reveal_target(&dir, Some("nope")).unwrap_err().code,
            "not_found"
        );
        assert_eq!(
            reveal_target("relative", None).unwrap_err().code,
            "invalid_input"
        );
    }
}
