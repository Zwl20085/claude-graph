//! The session's `hooks.json` (passed with `--settings`) and `hooks.log`.
//!
//! Claude Code runs the hook commands, which append one word per event to
//! `hooks.log`: `start` (SessionStart), `submit` (UserPromptSubmit: a typed
//! prompt was accepted), `stop` (Stop, end of every reply) and `notify`
//! (Notification: it waits for the user). [`HookLog::poll`] returns
//! the lines added since the last poll.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::contract::AppResult;
use crate::prompt::display_dir;
use crate::store::read_optional;

pub const HOOKS_FILE: &str = "hooks.json";
pub const HOOKS_LOG: &str = "hooks.log";

/// One line of hooks.log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    Start,
    Submit,
    Stop,
    Notify,
}

impl Hook {
    fn parse(line: &str) -> Option<Hook> {
        match line.split_whitespace().next().unwrap_or("") {
            "start" => Some(Hook::Start),
            "submit" => Some(Hook::Submit),
            "stop" => Some(Hook::Stop),
            "notify" => Some(Hook::Notify),
            _ => None,
        }
    }
}

/// `echo <word>>> "<log>"`: works in cmd.exe and sh alike (verified with the
/// real CLI on Windows; forward slashes in the path).
fn echo(word: &str, log: &str) -> Value {
    json!([{ "hooks": [{ "type": "command", "command": format!("echo {word}>> \"{log}\"") }] }])
}

/// The settings JSON for a session whose hooks.log is `log`.
pub fn hooks_json(log: &Path) -> Value {
    let log = display_dir(log);
    json!({ "hooks": {
        "SessionStart": echo("start", &log),
        "UserPromptSubmit": echo("submit", &log),
        "Stop": echo("stop", &log),
        "Notification": echo("notify", &log),
    }})
}

/// Reads hooks.log incrementally (by line count).
#[derive(Debug)]
pub struct HookLog {
    path: PathBuf,
    seen: usize,
}

impl HookLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path, seen: 0 }
    }

    /// A reader that skips the lines already in the file.
    pub fn at_end(path: PathBuf) -> Self {
        let mut log = Self::new(path);
        if let Err(e) = log.poll() {
            log::warn!("reading {}: {e}", log.path.display());
        }
        log
    }

    /// Hooks appended since the last call. A missing file has no lines; a
    /// trailing line without its newline is left for the next poll.
    pub fn poll(&mut self) -> AppResult<Vec<Hook>> {
        Ok(self.poll_lines()?.into_iter().map(|l| l.hook).collect())
    }

    /// [`HookLog::poll`] with each line's payload (the text after its word:
    /// the Codex notify JSON written by `--notify`).
    pub fn poll_lines(&mut self) -> AppResult<Vec<HookLine>> {
        let text = read_optional(&self.path)?.unwrap_or_default();
        let complete = match text.rfind('\n') {
            Some(i) => &text[..=i],
            None => "",
        };
        let lines: Vec<&str> = complete.lines().collect();
        let new = lines.get(self.seen..).unwrap_or_default();
        self.seen = self.seen.max(lines.len());
        Ok(new
            .iter()
            .filter_map(|l| {
                let hook = Hook::parse(l)?;
                let payload = l
                    .trim()
                    .split_once(char::is_whitespace)
                    .map(|(_, p)| p.trim());
                Some(HookLine {
                    hook,
                    payload: payload.filter(|p| !p.is_empty()).map(str::to_string),
                })
            })
            .collect())
    }
}

/// One parsed hooks.log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookLine {
    pub hook: Hook,
    pub payload: Option<String>,
}

impl HookLine {
    /// A `stop` that ends the turn whose typed text was `typed`: no payload,
    /// a payload that is not a JSON object with `input-messages`, or one
    /// whose input messages include `typed` (whitespace-insensitive). Codex
    /// also notifies for side turns (its thread-title request), which carry
    /// other input messages; those do not end the node's turn.
    pub fn stops_turn(&self, typed: &str) -> bool {
        if self.hook != Hook::Stop {
            return false;
        }
        let Some(payload) = &self.payload else {
            return true;
        };
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => return true,
        };
        let Some(inputs) = value.get("input-messages").and_then(|v| v.as_array()) else {
            return true;
        };
        let want = squash(typed);
        inputs
            .iter()
            .filter_map(|m| m.as_str())
            .any(|m| squash(m) == want)
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_json_appends_words_with_forward_slashes() {
        let v = hooks_json(Path::new("/tmp/s1/hooks.log"));
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, "echo stop>> \"/tmp/s1/hooks.log\"");
        assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["type"], "command");
        assert!(v["hooks"]["Notification"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("echo notify>> "));
        assert_eq!(
            v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "echo submit>> \"/tmp/s1/hooks.log\""
        );
        assert!(!cmd.contains('\\'));
    }

    #[test]
    fn poll_returns_only_new_complete_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(HOOKS_LOG);
        let mut log = HookLog::new(path.clone());
        assert!(log.poll().unwrap().is_empty());
        std::fs::write(&path, "start\r\nstop \r\nst").unwrap();
        assert_eq!(log.poll().unwrap(), vec![Hook::Start, Hook::Stop]);
        assert!(log.poll().unwrap().is_empty());
        std::fs::write(&path, "start\r\nstop \r\nstop\nnotify\nweird\nsubmit\n").unwrap();
        assert_eq!(
            log.poll().unwrap(),
            vec![Hook::Stop, Hook::Notify, Hook::Submit]
        );
        assert!(HookLog::at_end(path).poll().unwrap().is_empty());
    }

    #[test]
    fn stop_payloads_tell_the_main_turn_from_side_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(HOOKS_LOG);
        let main = r#"{"type":"agent-turn-complete","input-messages":["Reply  with ok"]}"#;
        let side = r#"{"type":"agent-turn-complete","input-messages":["Generate a title.\n\nUser prompt:\nReply with ok"]}"#;
        std::fs::write(
            &path,
            format!("stop {side}\nstop {main}\nstop\nstop not json\n"),
        )
        .unwrap();
        let lines = HookLog::new(path).poll_lines().unwrap();
        assert_eq!(lines.len(), 4);
        let stops: Vec<bool> = lines
            .iter()
            .map(|l| l.stops_turn("Reply with ok"))
            .collect();
        assert_eq!(stops, [false, true, true, true]);
        assert_eq!(lines[1].payload.as_deref(), Some(main));
        let submit = HookLine {
            hook: Hook::Submit,
            payload: None,
        };
        assert!(!submit.stops_turn("x"));
    }
}
