// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Hidden mode (Codex's `notify` program): append `stop` to a session's
    // hooks.log and exit, before anything of Tauri starts.
    if let Some(code) = claude_graph_lib::launch::notify_from_process() {
        std::process::exit(code);
    }
    // Hidden mode (the "Ask me in the app" approval server): MCP on stdio,
    // relaying to the running app over loopback.
    if let Some(code) = claude_graph_lib::approve::mcp::from_process() {
        std::process::exit(code);
    }
    // Agent-called mode: `--mcp-graph` (stdio MCP server) and `graph …` (CLI).
    if let Some(code) = claude_graph_lib::agentmode::mcp_tools::from_process() {
        std::process::exit(code);
    }
    if let Some(code) = claude_graph_lib::agentmode::cli::from_process() {
        std::process::exit(code);
    }
    claude_graph_lib::run()
}
