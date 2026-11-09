//! Claude Graph backend. See docs/ARCHITECTURE.md.

use std::time::Duration;

use tauri::{Manager, RunEvent};

pub mod agentmode;
pub mod app_settings;
pub mod approve;
pub mod assistant;
pub mod claude_bin;
pub mod commands;
pub mod contract;
pub mod contract_agent;
pub mod contract_inbox;
pub mod contract_prompt;
pub mod events;
pub mod graph_util;
pub mod harness;
pub mod inbox;
pub mod job;
pub mod launch;
pub mod mcp;
pub mod paths;
pub mod prompt;
pub mod run_api;
pub mod run_loop;
pub mod run_store;
pub mod runner;
pub mod scheduler;
pub mod store;
pub mod terminal;
pub mod trust;
pub mod turns;

/// How long app exit waits for cancelled runs to kill their processes and
/// write run.json.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// App exit (last window closed, or exit requested): cancel every active run
/// and assistant turn, kill every terminal session, and wait up to [`EXIT_WAIT`]. Whatever is left is caught by the Job Objects
/// (Windows) and, on the next open, by the stale-run sweep.
fn on_exit(app: &tauri::AppHandle) {
    // First: no inbox write-back for the sessions killed below (tips left
    // `running` are picked up by the stale-claim rule on the next scan).
    if let Some(inbox) = app.try_state::<inbox::Inbox>() {
        inbox.shutdown();
    }
    let state = app.state::<run_api::AppState>().inner().clone();
    if state.active_count() == 0
        && state.assistant_active_count() == 0
        && state.terminals().live_count() == 0
    {
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
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(run_api::AppState::default())
        .manage(agentmode::watch::AgentWatch::default())
        .setup(|app| {
            let state = app.state::<run_api::AppState>().inner().clone();
            let settings = state.app_settings();
            let inbox = inbox::gui::for_app(app.handle(), state);
            app.manage(inbox.clone());
            inbox::gui::start_timer(inbox);
            if let Err(e) = inbox::gui::apply_autostart(app.handle(), settings.autostart) {
                log::warn!("{}", e.message);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_claude,
            commands::list_harnesses,
            commands::save_harness,
            commands::delete_harness,
            commands::check_harness,
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
            commands::list_sessions,
            commands::pty_write,
            commands::pty_resize,
            commands::pty_replay,
            commands::open_session_window,
            commands::list_permission_prompts,
            commands::answer_permission,
            commands::answer_prompt,
            commands::continue_node,
            commands::list_session_prompts,
            agentmode::gui::agent_watch,
            agentmode::gui::agent_abandon,
            agentmode::gui::agent_client_configs,
            agentmode::gui::agent_install,
            inbox::gui::inbox_status,
            inbox::gui::inbox_scan_now,
            inbox::gui::inbox_reveal_results,
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
