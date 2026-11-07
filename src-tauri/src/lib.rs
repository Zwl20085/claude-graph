//! Claude Graph backend. See docs/ARCHITECTURE.md.

pub mod claude_bin;
pub mod commands;
pub mod contract;
pub mod events;
pub mod graph_util;
pub mod prompt;
pub mod run_api;
pub mod run_loop;
pub mod run_store;
pub mod runner;
pub mod scheduler;
pub mod store;
pub mod turns;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
            commands::create_graph,
            commands::open_graph,
            commands::save_graph,
            commands::list_recent,
            commands::pick_folder,
            commands::start_run,
            commands::cancel_run,
            commands::cancel_node,
            commands::get_run,
            commands::list_runs,
            commands::read_node_result,
            commands::render_brief,
            commands::open_path,
            commands::send_message,
            commands::list_messages,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            log::error!("error while running Claude Graph: {e}");
            std::process::exit(1);
        });
}
