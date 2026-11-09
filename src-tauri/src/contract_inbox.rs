//! Inbox IPC types (docs/INBOX.md). Re-exported by [`crate::contract`];
//! mirrored in `src/types/contract.ts`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Event name: the whole [`InboxStatus`] after every inbox change.
pub const EVENT_INBOX_UPDATE: &str = "inbox:update";

pub const INBOX_INTERVAL_DEFAULT: u32 = 12;
pub const INBOX_INTERVAL_MIN: u32 = 1;
pub const INBOX_INTERVAL_MAX: u32 = 168;
pub const INBOX_PARALLEL_DEFAULT: u32 = 2;
pub const INBOX_PARALLEL_MAX: u32 = 4;
pub const INBOX_TIMEOUT_DEFAULT: u32 = 120;
pub const INBOX_TIMEOUT_MIN: u32 = 5;
pub const INBOX_TIMEOUT_MAX: u32 = 24 * 60;

/// `inbox` of the app settings (per machine).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxSettings {
    /// Off by default; enable on one machine only.
    pub enabled: bool,
    /// Absolute path of the board's tips folder; empty = not chosen.
    pub folder: String,
    pub interval_hours: u32,
    pub max_parallel: u32,
    pub timeout_minutes: u32,
}

impl Default for InboxSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            folder: String::new(),
            interval_hours: INBOX_INTERVAL_DEFAULT,
            max_parallel: INBOX_PARALLEL_DEFAULT,
            timeout_minutes: INBOX_TIMEOUT_DEFAULT,
        }
    }
}

/// Protocol display state of a tip (docs/AGENT-PROTOCOL.md section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxTipState {
    Queued,
    Running,
    Done,
    Question,
    Failed,
    /// `agent.v` above 1: shown without state.
    None,
}

/// What this app is doing with a tip right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxPhase {
    /// Eligible, waiting for a free slot (not claimed yet).
    Waiting,
    /// Claimed; the session is working.
    Working,
    /// The session waits for the user (question written to the tip).
    Question,
    /// Outcome written; the session may still be open.
    Finished,
}

/// The run of a tip in this app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxRun {
    /// The protocol's `agent.run` (empty while waiting).
    pub run: String,
    pub phase: InboxPhase,
    /// The generated graph folder.
    pub graph_dir: String,
    /// The graph run (terminal run id) once started.
    pub graph_run_id: Option<String>,
    /// The session id of that run, for `open_session_window`.
    pub sid: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    /// Last thing that happened (outcome, write-back problem).
    pub note: Option<String>,
}

/// One agent tip of the folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxTip {
    /// File stem.
    pub id: String,
    pub title: String,
    pub state: InboxTipState,
    /// Why it runs, or why it is skipped, at the last scan.
    pub decision: String,
    pub eligible: bool,
    /// `agent.host` / `agent.at` as read.
    pub host: Option<String>,
    pub at: Option<String>,
    pub run: Option<InboxRun>,
}

/// A file the scan could not read (never written).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxSkipped {
    pub file: String,
    pub reason: String,
}

/// One line of the inbox activity log (newest last, capped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxLogLine {
    pub ts: DateTime<Utc>,
    pub tip: Option<String>,
    pub text: String,
}

/// Return value of `inbox_status` / `inbox_scan_now`; payload of `inbox:update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxStatus {
    pub enabled: bool,
    /// The configured folder (empty when none).
    pub folder: String,
    /// Shown when `folder` is empty; never adopted silently.
    pub suggested_folder: Option<String>,
    /// This machine as written to `agent.host`.
    pub host: String,
    pub scanning: bool,
    pub last_scan: Option<DateTime<Utc>>,
    pub next_scan: Option<DateTime<Utc>>,
    /// Why the last scan failed, if it did.
    pub last_error: Option<String>,
    pub tips: Vec<InboxTip>,
    pub skipped: Vec<InboxSkipped>,
    /// Non-agent tips seen at the last scan (never read past their frontmatter).
    pub other_tips: u32,
    pub log: Vec<InboxLogLine>,
}
