//! Harness profiles: the types (re-exported by [`crate::contract`]), the
//! built-in profiles and the validation applied at the command boundary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::contract::{AppError, AppResult};
use crate::paths::is_unc_or_device;

/// Which adapter drives the CLI (readiness, submit and stop detection, command shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    Claude,
    Codex,
    Custom,
    /// DeepSeek Harness CLI `dsh`: headless only, one process per message,
    /// chained by its session id (docs/ARCHITECTURE.md "dsh harness").
    #[serde(rename = "deepseek-harness")]
    DeepseekHarness,
}

/// How the end of a turn is detected (custom profiles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopMode {
    /// No output for `quiet_ms` after the submit.
    #[default]
    Quiet,
    /// Claude Code style hooks (`--settings hooks.json`).
    ClaudeHooks,
    /// Codex style `-c notify=[…]`.
    CodexNotify,
}

/// One harness profile (docs/ARCHITECTURE.md "Harness profiles").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessProfile {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub kind: HarnessKind,
    /// `None`: resolved like today (claude chain; `codex` on PATH).
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Values may reference `${VAR}` (substituted at spawn, never stored expanded).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub idle_marker: Option<String>,
    #[serde(default)]
    pub dialog_markers: Vec<String>,
    #[serde(default)]
    pub stop_mode: StopMode,
    #[serde(default)]
    pub quiet_ms: Option<u64>,
    /// Custom: the regex of a dialog's option rows (named groups `label`,
    /// optional `num` and `cur`); none = the generic numbered-list pattern.
    #[serde(default)]
    pub option_pattern: Option<String>,
    /// Custom: extra texts that mark a dialog's footer or question.
    #[serde(default)]
    pub dialog_hints: Vec<String>,
    /// deepseek-harness: the dsh profile (`--profile`); none = `headless`.
    #[serde(default)]
    pub dsh_profile: Option<String>,
}

/// Return value of `check_harness`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessCheck {
    pub id: String,
    pub ok: bool,
    pub version: Option<String>,
    pub message: Option<String>,
}

pub const CLAUDE_ID: &str = "claude";
pub const DEEPSEEK_ID: &str = "claude-deepseek";
pub const CODEX_ID: &str = "codex";
pub const DSH_ID: &str = "deepseek-harness";
/// The dsh profile used when a profile names none.
pub const DEFAULT_DSH_PROFILE: &str = "headless";
const PATTERN_MAX: usize = 300;
/// Default of `quiet_ms` for `stop_mode: quiet`.
pub const DEFAULT_QUIET_MS: u64 = 8000;
pub const QUIET_MS_RANGE: (u64, u64) = (500, 600_000);
const LABEL_MAX: usize = 60;
const COMMAND_MAX: usize = 1024;
const ARG_MAX: usize = 4096;
const ARGS_MAX: usize = 64;
const ENV_MAX: usize = 64;
const ENV_VALUE_MAX: usize = 8192;
const MODEL_MAX: usize = 100;
const MARKER_MAX: usize = 200;
const MARKERS_MAX: usize = 16;

const DEEPSEEK_ENV: [(&str, &str); 7] = [
    ("ANTHROPIC_BASE_URL", "https://api.deepseek.com/anthropic"),
    ("ANTHROPIC_AUTH_TOKEN", "${DEEPSEEK_API_KEY}"),
    ("ANTHROPIC_MODEL", "deepseek-flash[1m]"),
    ("ANTHROPIC_DEFAULT_OPUS_MODEL", "deepseek-flash[1m]"),
    ("ANTHROPIC_DEFAULT_SONNET_MODEL", "deepseek-flash[1m]"),
    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "deepseek-flash"),
    ("CLAUDE_CODE_SUBAGENT_MODEL", "deepseek-flash"),
];

fn builtin(id: &str, label: &str, kind: HarnessKind) -> HarnessProfile {
    HarnessProfile {
        id: id.into(),
        label: label.into(),
        kind,
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        model: None,
        builtin: true,
        idle_marker: None,
        dialog_markers: Vec::new(),
        stop_mode: StopMode::Quiet,
        quiet_ms: None,
        option_pattern: None,
        dialog_hints: Vec::new(),
        dsh_profile: None,
    }
}

/// The built-in profiles, in list order.
pub fn builtins() -> Vec<HarnessProfile> {
    let claude = builtin(CLAUDE_ID, "Claude Code", HarnessKind::Claude);
    let deepseek = HarnessProfile {
        env: DEEPSEEK_ENV
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..builtin(
            DEEPSEEK_ID,
            "Claude Code via DeepSeek API",
            HarnessKind::Claude,
        )
    };
    let codex = HarnessProfile {
        args: vec!["--no-alt-screen".into()],
        ..builtin(CODEX_ID, "Codex CLI", HarnessKind::Codex)
    };
    let dsh = builtin(
        DSH_ID,
        "DeepSeek Harness (dsh)",
        HarnessKind::DeepseekHarness,
    );
    vec![claude, deepseek, codex, dsh]
}

pub fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new("invalid_input", msg)
}

/// `^[a-z0-9][a-z0-9-]{0,31}$`.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    (1..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `^[A-Za-z_][A-Za-z0-9_]*$` (at most 128 chars).
pub fn valid_env_key(key: &str) -> bool {
    let b = key.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn has_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

fn text_field(what: &str, value: &str, max: usize) -> AppResult<()> {
    if value.chars().count() > max {
        return Err(invalid(format!("{what} is longer than {max} chars")));
    }
    if has_control(value) {
        return Err(invalid(format!("{what} contains control characters")));
    }
    Ok(())
}

/// A trimmed optional text: blank becomes `None`.
fn trimmed(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn check_command(command: &str) -> AppResult<()> {
    text_field("command", command, COMMAND_MAX)?;
    if is_unc_or_device(command) {
        return Err(invalid(format!(
            "command {command} is a network or device path; use a local program"
        )));
    }
    Ok(())
}

fn check_model(model: &str) -> AppResult<()> {
    let ok = model.len() <= MODEL_MAX
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:-/[]@".contains(c));
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "model {model:?} may only use letters, digits and . _ : - / [ ] @ (at most {MODEL_MAX})"
        )))
    }
}

fn check_lists(p: &HarnessProfile) -> AppResult<()> {
    if p.args.len() > ARGS_MAX {
        return Err(invalid(format!("at most {ARGS_MAX} args")));
    }
    for a in &p.args {
        text_field("an argument", a, ARG_MAX)?;
    }
    if p.env.len() > ENV_MAX {
        return Err(invalid(format!("at most {ENV_MAX} environment variables")));
    }
    for (k, v) in &p.env {
        if !valid_env_key(k) {
            return Err(invalid(format!(
                "environment variable name {k:?} must match [A-Za-z_][A-Za-z0-9_]*"
            )));
        }
        // The value is never echoed back: it may be a secret.
        text_field(&format!("the value of {k}"), v, ENV_VALUE_MAX)?;
    }
    Ok(())
}

fn check_custom(p: &HarnessProfile) -> AppResult<()> {
    if p.command.is_none() {
        return Err(invalid("a custom harness needs a command"));
    }
    if let Some(m) = &p.idle_marker {
        text_field("idle marker", m, MARKER_MAX)?;
    }
    if p.dialog_markers.len() > MARKERS_MAX {
        return Err(invalid(format!("at most {MARKERS_MAX} dialog markers")));
    }
    for m in &p.dialog_markers {
        text_field("a dialog marker", m, MARKER_MAX)?;
    }
    if let Some(q) = p.quiet_ms {
        let (lo, hi) = QUIET_MS_RANGE;
        if !(lo..=hi).contains(&q) {
            return Err(invalid(format!("quiet_ms must be between {lo} and {hi}")));
        }
    }
    if let Some(pat) = &p.option_pattern {
        text_field("option pattern", pat, PATTERN_MAX)?;
        crate::terminal::dialog::OptionPattern::new(pat)
            .map_err(|e| invalid(format!("option pattern: {e}")))?;
    }
    if p.dialog_hints.len() > MARKERS_MAX {
        return Err(invalid(format!("at most {MARKERS_MAX} dialog hints")));
    }
    for h in &p.dialog_hints {
        text_field("a dialog hint", h, MARKER_MAX)?;
    }
    Ok(())
}

/// A dsh profile name: `^[a-z0-9][a-z0-9_-]{0,63}$`.
pub fn valid_dsh_profile(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// Trimmed, blank dropped.
fn clean_list(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect()
}

/// A validated, normalized copy of `p` (trimmed texts, blanks dropped;
/// custom-only fields reset for claude and codex kinds).
pub fn validated(p: &HarnessProfile) -> AppResult<HarnessProfile> {
    if !valid_id(&p.id) {
        return Err(invalid(format!(
            "harness id {:?} must match ^[a-z0-9][a-z0-9-]{{0,31}}$",
            p.id
        )));
    }
    let label = p.label.trim().to_string();
    if label.is_empty() {
        return Err(invalid("the harness needs a label"));
    }
    text_field("label", &label, LABEL_MAX)?;
    let command = trimmed(&p.command);
    if let Some(c) = &command {
        check_command(c)?;
    }
    let model = trimmed(&p.model);
    if let Some(m) = &model {
        check_model(m)?;
    }
    let custom = p.kind == HarnessKind::Custom;
    let clean = HarnessProfile {
        id: p.id.clone(),
        label,
        kind: p.kind,
        command,
        args: p.args.clone(),
        env: p.env.clone(),
        model,
        builtin: false,
        idle_marker: if custom {
            trimmed(&p.idle_marker)
        } else {
            None
        },
        dialog_markers: if custom {
            clean_list(&p.dialog_markers)
        } else {
            Vec::new()
        },
        stop_mode: if custom { p.stop_mode } else { StopMode::Quiet },
        quiet_ms: if custom { p.quiet_ms } else { None },
        option_pattern: trimmed(&p.option_pattern).filter(|_| custom),
        dialog_hints: if custom {
            clean_list(&p.dialog_hints)
        } else {
            Vec::new()
        },
        dsh_profile: trimmed(&p.dsh_profile).filter(|_| p.kind == HarnessKind::DeepseekHarness),
    };
    if let Some(name) = &clean.dsh_profile {
        if !valid_dsh_profile(name) {
            return Err(invalid(format!(
                "dsh profile {name:?} must match ^[a-z0-9][a-z0-9_-]{{0,63}}$"
            )));
        }
    }
    check_lists(&clean)?;
    if custom {
        check_custom(&clean)?;
    }
    Ok(clean)
}

#[cfg(test)]
#[path = "profile_tests.rs"]
mod tests;
