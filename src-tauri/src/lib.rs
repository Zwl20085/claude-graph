//! Claude Graph backend. See docs/ARCHITECTURE.md.

use std::time::Duration;

use tauri::{Manager, RunEvent};

pub mod app_settings;
pub mod assistant;
pub mod claude_bin;
pub mod commands;
pub mod contract;
pub mod events;
pub mod graph_util;
pub mod job;
pub mod launch;
pub mod paths;
pub mod prompt;
pub mod run_api;
pub mod run_loop;
pub mod run_store;
pub mod runner;
pub mod scheduler;
pub mod store;
pub mod trust;
pub mod turns;

/// How long app exit waits for cancelled runs to kill their processes and
/// write run.json.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// App exit (last window closed, or exit requested): cancel every active run
/// and assistant turn and wait up to [`EXIT_WAIT`]. Whatever is left is caught by the Job Objects
/// (Windows) and, on the next open, by the stale-run sweep.
fn on_exit(app: &tauri::AppHandle) {
    let state = app.state::<run_api::AppState>().inner().clone();
    if state.active_count() == 0 && state.assistant_active_count() == 0 {
        return;
    }
    log::info!(
        "exit: cancelling {} active run(s) and {} assistant turn(s)",
        state.active_count(),
        state.assistant_active_count()
    );
    tauri::async_runtime::block_on(state.shutdown(EXIT_WAIT));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(run_api::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::check_claude,
            commands::get_app_settings,
            commands::set_app_settings,
            commands::create_graph,
            commands::open_graph,
            commands::save_graph,
            commands::list_recent,
            commands::pick_folder,
            commands::graph_trust_status,
            commands::trust_graph,
            commands::start_run,
            commands::cancel_run,
            commands::cancel_node,
            commands::get_run,
            commands::list_runs,
            commands::read_node_result,
            commands::render_brief,
            commands::reveal_graph_dir,
            commands::reveal_node_dir,
            commands::send_message,
            commands::list_messages,
            commands::launch_options,
            commands::assistant_state,
            commands::assistant_send,
            commands::assistant_cancel,
            commands::assistant_reset,
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            log::error!("error while building Claude Graph: {e}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let RunEvent::ExitRequested { .. } = event {
            on_exit(handle);
        }
    });
}
