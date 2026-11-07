// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Hidden mode (Codex's `notify` program): append `stop` to a session's
    // hooks.log and exit, before anything of Tauri starts.
    if let Some(code) = claude_graph_lib::launch::notify_from_process() {
        std::process::exit(code);
    }
    claude_graph_lib::run()
}
