//! The codex adapter: OpenAI Codex CLI in its inline TUI.
//!
//! `codex -C <cwd> --no-alt-screen [-m <model>] -c notify=[<app>,"--notify",<hooks.log>] <args…>`.
//! No hooks are used (they need hook trust): readiness is the composer
//! placeholder on screen after the last dialog marker; a submit is the typed
//! text leaving the composer (the last `›` row and below) with "Working"
//! shown or the placeholder gone; the end of a turn is the `stop` line the
//! notify program appends (verified with Codex 0.167, docs/CLAUDE_CLI_NOTES.md).

use std::ffi::OsString;

use crate::harness::adapter::{notify_toml, Behavior, Driver, SpawnInput, StopSignal};
use crate::terminal::deliver::{Expect, FALLBACK_QUIET};
use crate::terminal::hooks::HOOKS_LOG;
use crate::terminal::live::Composer;
use crate::terminal::screen::{normalize, Markers};

pub const NO_ALT_SCREEN: &str = "--no-alt-screen";
/// The empty composer's placeholder texts.
pub const IDLE: [&str; 2] = ["Ask Codex to do anything", "Ask a follow-up question"];
/// Codex's folder-trust dialog (current and older wording).
pub const DIALOGS: [&str; 5] = [
    "Folder access",
    "Trust this folder",
    "Continue only if you trust",
    "Do you trust the contents",
    "Trust and continue",
];
/// The composer's prompt glyph.
pub const GLYPH: char = '\u{203a}';
/// Shown while a prompt is being worked on ("Working (3s • esc to interrupt)").
pub const BUSY: [&str; 2] = ["working", "esc to interrupt"];

pub fn behavior() -> Behavior {
    Behavior {
        label: String::new(),
        markers: std::sync::Arc::new(Markers::of(&IDLE, &DIALOGS)),
        start_hook: false,
        expect: Expect::Gone(Composer {
            glyph: Some(GLYPH),
            rows: 4,
            placeholders: IDLE.iter().map(|p| normalize(p)).collect(),
            busy: BUSY.iter().map(|b| b.to_string()).collect(),
        }),
        ready_quiet: FALLBACK_QUIET,
        stop: StopSignal::Hook,
        types_model: false,
        driver: Driver::Pty,
    }
}

/// `-c notify=[…]` pointing at this session's hooks.log.
pub fn notify_args(input: &SpawnInput<'_>) -> Vec<OsString> {
    let log = input.session_dir.join(HOOKS_LOG);
    vec!["-c".into(), notify_toml(input.notify_exe, &log).into()]
}

pub fn args(input: &SpawnInput<'_>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-C".into(), input.cwd.as_os_str().to_owned()];
    // The flag may also be in the profile's args (the built-in has it there);
    // passing it twice is an error in clap.
    if !input.profile.args.iter().any(|a| a == NO_ALT_SCREEN) {
        args.push(NO_ALT_SCREEN.into());
    }
    if let Some(m) = &input.profile.model {
        args.extend(["-m".into(), m.into()]);
    }
    args.extend(notify_args(input));
    args
}
