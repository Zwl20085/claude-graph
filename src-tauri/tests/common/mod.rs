//! Shared helpers for the end-to-end tests: a temp graph folder made from the
//! example graph, the fake CLI wired through `CLAUDE_GRAPH_CLAUDE_BIN`, and an
//! event recorder.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use claude_graph_lib::claude_bin::ENV_CLAUDE_BIN;
use claude_graph_lib::contract::{
    AppResult, AssistantState, Graph, NodeEvent, NodeEventKind, PtyData, RunState,
};
use claude_graph_lib::events::{Emitter, WindowSpec};
use claude_graph_lib::run_api::{abs, AppState};
use claude_graph_lib::store;
use tokio::task::JoinHandle;

pub const TIMEOUT: Duration = Duration::from_secs(90);

pub fn fake_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join("fake-claude.mjs")
        .canonicalize()
        .expect("fake-claude.mjs exists")
}

/// Point the runner at `node <abs>/scripts/fake-claude.mjs [extra]`. Every test
/// in one test binary must pass the same `extra` (the env is process-wide).
pub fn use_fake_claude(extra: &str) {
    let script = fake_script().display().to_string();
    // canonicalize() on Windows gives a \\?\ path; node accepts it, but keep it plain.
    let script = script.trim_start_matches(r"\\?\").to_string();
    let line = format!("node \"{script}\" {extra}");
    // Tests in a binary all set the same value, so concurrent writes are benign.
    std::env::set_var(ENV_CLAUDE_BIN, line.trim());
}

pub fn example_graph() -> Graph {
    serde_json::from_str(include_str!("../../../examples/hello-pipeline/graph.json")).unwrap()
}

/// A temp folder holding `graph` saved through the store.
pub fn graph_dir(graph: &Graph) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    store::save_graph(tmp.path(), graph).unwrap();
    tmp
}

#[derive(Default)]
pub struct Recorder {
    pub states: Mutex<Vec<RunState>>,
    pub events: Mutex<Vec<NodeEvent>>,
    pub assistant: Mutex<Vec<AssistantState>>,
    /// Terminal mode: labels of the windows opened, in order.
    pub windows: Mutex<Vec<WindowSpec>>,
    /// Terminal mode: `pty:data` chunks (window label, payload).
    pub pty: Mutex<Vec<(String, PtyData)>>,
}

impl Emitter for Recorder {
    fn run_state(&self, state: &RunState) {
        self.states.lock().unwrap().push(state.clone());
    }
    fn node_event(&self, event: &NodeEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
    fn assistant_update(&self, state: &AssistantState) {
        self.assistant.lock().unwrap().push(state.clone());
    }
    fn pty_data(&self, label: &str, data: &PtyData) {
        self.pty
            .lock()
            .unwrap()
            .push((label.to_string(), data.clone()));
    }
    fn open_window(&self, spec: &WindowSpec) -> AppResult<()> {
        self.windows.lock().unwrap().push(spec.clone());
        Ok(())
    }
}

impl Recorder {
    pub fn events_of(&self, node: &str, kind: NodeEventKind) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.node_id == node && e.kind == kind)
            .map(|e| e.text.clone())
            .collect()
    }
}

pub async fn finish(join: JoinHandle<RunState>) -> RunState {
    tokio::time::timeout(TIMEOUT, join)
        .await
        .expect("run finished in time")
        .expect("run task did not panic")
}

/// An app state with its own temp data folder (no user settings, no trust
/// records), so tests never read or write the real app data.
pub fn fresh_app() -> AppState {
    let data = tempfile::tempdir().unwrap().keep();
    AppState::new(data)
}

/// [`fresh_app`] that has already trusted the graph saved in `dir`.
pub fn trusted_app(dir: &Path) -> AppState {
    let app = fresh_app();
    app.trust_graph(&abs(dir)).expect("trust_graph");
    app
}
