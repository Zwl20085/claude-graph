//! Tauri side of the inbox: the `inbox:update` sink, the commands
//! (`inbox_status`, `inbox_scan_now`, `inbox_reveal_results`), autostart and
//! the timer start-up (registered in lib.rs).

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::contract::{AppError, AppResult, InboxStatus, EVENT_INBOX_UPDATE};
use crate::events::TauriEmitter;
use crate::run_api::AppState;
use crate::run_loop::blocking;

use super::env::{InboxEnv, InboxSink};
use super::runner::Inbox;

struct TauriSink(AppHandle);

impl InboxSink for TauriSink {
    fn update(&self, status: &InboxStatus) {
        use tauri::Emitter as _;
        if let Err(e) = self.0.emit(EVENT_INBOX_UPDATE, status) {
            log::warn!("emit {EVENT_INBOX_UPDATE} failed: {e}");
        }
    }
}

/// The app's inbox (the run emitter opens session windows as usual).
pub fn for_app(app: &AppHandle, state: AppState) -> Inbox {
    Inbox::new(InboxEnv::for_app(
        state,
        Arc::new(TauriEmitter(app.clone())),
        Arc::new(TauriSink(app.clone())),
    ))
}

/// Spawn the timer.
pub fn start_timer(inbox: Inbox) {
    tauri::async_runtime::spawn(super::timer::run(
        inbox,
        super::timer::FIRST_DELAY,
        super::timer::TICK,
    ));
}

/// Make the OS autostart entry match `wanted` (no-op when it already does).
pub fn apply_autostart(app: &AppHandle, wanted: bool) -> AppResult<()> {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    let err = |e: tauri_plugin_autostart::Error| AppError::new("io", format!("autostart: {e}"));
    if launcher.is_enabled().map_err(err)? == wanted {
        return Ok(());
    }
    if wanted {
        launcher.enable().map_err(err)
    } else {
        launcher.disable().map_err(err)
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn inbox_status(inbox: State<'_, Inbox>) -> AppResult<InboxStatus> {
    let ib = inbox.inner().clone();
    blocking(move || Ok(ib.status())).await
}

/// `invalid_input` when the inbox is off.
#[tauri::command(rename_all = "snake_case")]
pub async fn inbox_scan_now(inbox: State<'_, Inbox>) -> AppResult<InboxStatus> {
    inbox.inner().clone().scan_now().await
}

/// Reveal `<tips folder>/agent/<tip id>/` of a tip the last scan listed.
#[tauri::command(rename_all = "snake_case")]
pub async fn inbox_reveal_results(
    app: AppHandle,
    inbox: State<'_, Inbox>,
    tip_id: String,
) -> AppResult<()> {
    let ib = inbox.inner().clone();
    let path: PathBuf = blocking(move || {
        let status = ib.status();
        if !status.tips.iter().any(|t| t.id == tip_id) {
            return Err(AppError::new(
                "not_found",
                format!("no agent tip {tip_id:?}"),
            ));
        }
        let tips = PathBuf::from(&status.folder);
        if !tips.join("agent").join(&tip_id).is_dir() {
            return Err(AppError::new("not_found", "no results for this tip yet"));
        }
        let dir = super::results::results_dir(&tips, &tip_id)
            .map_err(|e| AppError::new("invalid_input", e.to_string()))?;
        Ok(dir)
    })
    .await?;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| AppError::new("io", e.to_string()))
}
