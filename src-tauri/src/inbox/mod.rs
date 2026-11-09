//! Inbox: the runner role of docs/AGENT-PROTOCOL.md. Claude Graph scans the
//! board's tips folder, runs each eligible `kind: agent` tip in its own
//! one-node terminal graph, and writes the outcome back into the tip.
//! See docs/INBOX.md.
//!
//! - Pure protocol core: [`split`] (frontmatter / body), [`tip`] (parsing),
//!   [`hash`] (section 4), [`eligible`] (sections 5 and 6), [`patch`] (the
//!   `agent` map only), [`block`] (section 3 and the 7 outcome table),
//!   [`ops`] (claim / finish as text transformations).
//! - Folder I/O: [`folder`] (listing, stamp, guarded whole-file write),
//!   [`writeback`] (claim / finish on the file), [`results`] (copies into
//!   `agent/<tip id>/`), [`scan`] (one read-only pass).
//! - Orchestration: [`graphgen`] (the generated graph), [`runner`] (scans,
//!   queue, state), [`start`], [`watch`] (outcome of one run), [`timer`].
//! - Environment: [`env`] (injectable dependencies), [`settings`], [`host`],
//!   [`gui`] (Tauri commands).

pub mod block;
pub mod eligible;
pub mod env;
pub mod folder;
pub mod graphgen;
pub mod gui;
pub mod hash;
pub mod host;
pub mod ops;
pub mod patch;
pub mod persist;
pub mod results;
pub(crate) mod retry;
pub mod runner;
pub mod scan;
pub mod settings;
pub mod split;
pub(crate) mod start;
pub mod timer;
pub mod tip;
pub(crate) mod watch;
pub mod writeback;

pub use env::{InboxEnv, InboxSink, NullSink, RunConfig};
pub use runner::Inbox;

/// `dir` is one of the generated inbox graphs under `data_dir` (they are
/// never added to the recent list).
pub fn is_inbox_graph(data_dir: &std::path::Path, dir: &std::path::Path) -> bool {
    let root = crate::paths::abs(&graphgen::inbox_root(data_dir));
    crate::paths::abs(dir).starts_with(root)
}
