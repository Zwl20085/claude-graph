//! The custom adapter: any CLI. Program = `profile.command`, args =
//! `profile.args` (after `--settings` or `-c notify` per `stop_mode`).
//! Readiness: `idle_marker` on screen after the last dialog marker and 1 s
//! of quiet (no marker: 3 s of quiet). Submit: the typed text gone from the
//! last 3 non-blank rows. Stop per `stop_mode`.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use crate::contract::{HarnessProfile, StopMode};
use crate::harness::adapter::{Behavior, SpawnInput, StopSignal};
use crate::harness::profile::DEFAULT_QUIET_MS;
use crate::harness::{claude, codex};
use crate::terminal::deliver::{Expect, FALLBACK_QUIET};
use crate::terminal::live::Composer;
use crate::terminal::screen::Markers;

/// Readiness without an idle marker: this much quiet.
pub const QUIET_READY: Duration = Duration::from_secs(3);
/// Rows that count as the input area.
pub const INPUT_ROWS: usize = 3;

pub fn behavior(p: &HarnessProfile) -> Behavior {
    let idle: Vec<&str> = p.idle_marker.as_deref().into_iter().collect();
    let dialog: Vec<&str> = p.dialog_markers.iter().map(String::as_str).collect();
    let stop = match p.stop_mode {
        StopMode::Quiet => StopSignal::Quiet(Duration::from_millis(
            p.quiet_ms.unwrap_or(DEFAULT_QUIET_MS),
        )),
        StopMode::ClaudeHooks | StopMode::CodexNotify => StopSignal::Hook,
    };
    Behavior {
        label: String::new(),
        markers: Arc::new(Markers::of(&idle, &dialog)),
        start_hook: p.stop_mode == StopMode::ClaudeHooks,
        expect: Expect::Gone(Composer {
            glyph: None,
            rows: INPUT_ROWS,
            placeholders: Vec::new(),
            busy: Vec::new(),
        }),
        ready_quiet: if p.idle_marker.is_some() {
            FALLBACK_QUIET
        } else {
            QUIET_READY
        },
        stop,
        types_model: false,
    }
}

pub fn args(input: &SpawnInput<'_>) -> Vec<OsString> {
    match input.profile.stop_mode {
        StopMode::Quiet => Vec::new(),
        StopMode::ClaudeHooks => claude::settings_args(input),
        StopMode::CodexNotify => codex::notify_args(input),
    }
}
