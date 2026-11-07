//! Terminal mode (docs/ARCHITECTURE.md "Terminal mode (default)"): `Run all`
//! starts one interactive `claude` per connected component of the graph in a
//! pseudo terminal, shows it in its own window, and types the node messages
//! in order as if the user typed them. Between nodes it waits for Claude
//! Code's Stop hook. The process stays alive after the last node so the user
//! can keep talking in the window.
//!
//! - [`components`]: components, send order, the sessions of a run target.
//! - [`message`]: the typed message rule and the terminal-mode brief.
//! - [`hooks`]: `hooks.json` (`--settings`) and `hooks.log` polling.
//! - [`pty`]: the portable-pty process, its reader thread and transcript.
//! - [`screen`]: output as text (UTF-8, escapes, dialog detection).
//! - [`deliver`]: the readiness gate and the verified delivery of one message.
//! - `port`: delivery on the live pty; follow-up messages.
//! - [`node`]: one node inside a session (brief, evaluation, files).
//! - [`session`]: one session's state machine (`startup`: its first phase,
//!   `send`: one node).
//! - [`run`]: the run loop (session slots, cancel, run.json).
//! - [`api`]: the live-session registry and the command-level functions.

pub mod api;
pub mod components;
pub mod deliver;
pub mod hooks;
pub mod message;
pub mod node;
mod port;
pub mod pty;
pub mod run;
pub mod screen;
mod send;
pub mod session;
mod startup;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::contract::{NodeStatus, SessionStatus};
use crate::run_store::run_dir;

pub use api::{SessionShared, Terminals};
pub use components::{plan, SessionPlan};

/// Default terminal size until the window reports its own.
pub const DEFAULT_COLS: u16 = 120;
pub const DEFAULT_ROWS: u16 = 40;
pub const TRANSCRIPT_FILE: &str = "transcript.log";
pub const SESSION_FILE: &str = "session.json";

/// `runs/<run>/sessions/<sid>/`.
pub fn session_dir(graph_dir: &Path, run_id: &str, sid: &str) -> PathBuf {
    run_dir(graph_dir, run_id).join("sessions").join(sid)
}

/// Label of a session window.
pub fn window_label(run_id: &str, sid: &str) -> String {
    format!("term-{run_id}-{sid}")
}

/// Content of `runs/<run>/sessions/<sid>/session.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionFile {
    pub sid: String,
    pub title: String,
    pub node_ids: Vec<String>,
    pub pid: Option<u32>,
    pub status: SessionStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Content of `runs/<run>/<node>/summary.json` in terminal mode: the contract
/// fields plus the headless [`crate::contract::NodeSummary`] fields, so
/// `read_node_result` reads it unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalNodeSummary {
    pub session: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub status: NodeStatus,
    pub reason: Option<String>,
    pub session_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub num_turns: Option<u32>,
    pub is_error: bool,
    pub permission_denials: Vec<String>,
}

/// A change reported by a session task to the run loop.
#[derive(Debug, Clone, PartialEq)]
pub enum TermMsg {
    Node {
        node_id: String,
        status: NodeStatus,
        reason: Option<String>,
        started_at: Option<DateTime<Utc>>,
        finished_at: Option<DateTime<Utc>>,
    },
    /// The session's new public state.
    Session(crate::contract::TerminalSession),
}
