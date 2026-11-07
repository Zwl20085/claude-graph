//! Assistant mode (docs/ARCHITECTURE.md "Assistant mode"): the graph is the
//! workflow definition and one long-lived Claude Code session, the work
//! scheduler, executes it. The app relays the user's instructions and answers
//! and shows only what the scheduler reports in its final reply.
//!
//! - [`charter`]: the bundled charter and `assistant/CHARTER.md`.
//! - [`render`]: the workflow as text ([`render_workflow`]).
//! - [`parse`]: the scheduler's reply as journal messages ([`parse_reply`]).
//! - [`journal`]: `assistant/session.json` and `assistant/journal.jsonl`.
//! - [`turn`]: one scheduler turn (prompt, command line, recording the outcome).
//! - [`api`]: the live-turn registry and the `AppState` methods behind the commands.
//!
//! No model, permission mode, tool list or turn limit is ever passed: the
//! user's own Claude Code settings decide those.

pub mod api;
pub mod charter;
pub mod journal;
pub mod parse;
pub mod render;
pub mod turn;

pub use api::AssistantTurns;
pub use charter::CHARTER;
pub use parse::{parse_reply, status_after, Section};
pub use render::render_workflow;

/// `run_id` of the scheduler's `node:event`s.
pub const RUN_ID: &str = "assistant";
/// `node_id` of the scheduler's `node:event`s.
pub const NODE_ID: &str = "scheduler";
