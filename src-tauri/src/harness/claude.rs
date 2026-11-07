//! The claude adapter: Claude Code as terminal mode always ran it
//! (`--settings hooks.json --add-dir <graph> [--permission-mode] [--model]`),
//! with the profile's env on top (the `claude-deepseek` profile differs only
//! by env). Start, submit and stop come from the hooks.

use std::ffi::OsString;

use crate::harness::adapter::{Behavior, SpawnInput, StopSignal};
use crate::terminal::deliver::{Expect, FALLBACK_QUIET};
use crate::terminal::hooks::HOOKS_FILE;
use crate::terminal::screen::Markers;

pub fn behavior() -> Behavior {
    Behavior {
        label: String::new(),
        markers: Markers::claude(),
        start_hook: true,
        expect: Expect::Submit,
        ready_quiet: FALLBACK_QUIET,
        stop: StopSignal::Hook,
        types_model: true,
    }
}

/// `--settings <hooks.json>` (shared with custom `claude_hooks` profiles).
pub fn settings_args(input: &SpawnInput<'_>) -> Vec<OsString> {
    vec![
        "--settings".into(),
        input.session_dir.join(HOOKS_FILE).into_os_string(),
    ]
}

pub fn args(input: &SpawnInput<'_>) -> Vec<OsString> {
    let mut args = settings_args(input);
    args.extend(["--add-dir".into(), input.graph_dir.as_os_str().to_owned()]);
    if let Some(p) = input.permission {
        args.extend(["--permission-mode".into(), p.into()]);
    }
    if let Some(m) = &input.profile.model {
        args.extend(["--model".into(), m.into()]);
    }
    args
}
