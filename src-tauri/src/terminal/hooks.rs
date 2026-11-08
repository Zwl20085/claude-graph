//! The session's `hooks.json` (passed with `--settings`) and `hooks.log`.
//!
//! Claude Code runs the hook commands, which append one line per event to
//! `hooks.log`: `start` (SessionStart), `submit` (UserPromptSubmit: a typed
//! prompt was accepted), `stop` (Stop, end of every reply) and `notify`
//! (Notification: it waits for the user). When the app binary writes the
//! line (`"<app>" --notify "<hooks.log>" --event <word> --stdin`) the word is
//! followed by the hook's stdin payload as one line of JSON (`transcript_path`,
//! the Stop's `last_assistant_message`, …; docs/CLAUDE_CLI_NOTES.md); the
//! `echo` fallback writes the word only. [`HookLog::poll`] returns the lines
//! added since the last poll.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::contract::AppResult;
use crate::prompt::display_dir;

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

/// `"<app>" --notify "<log>" --event <word> --stdin`: the app binary appends
/// the word and the hook's stdin JSON (quoted forward-slash paths work in
/// cmd.exe and sh alike).
fn notify(word: &str, log: &str, exe: &str) -> Value {
    let command = format!("\"{exe}\" --notify \"{log}\" --event {word} --stdin");
    json!([{ "hooks": [{ "type": "command", "command": command }] }])
}

/// The settings JSON for a session whose hooks.log is `log`. `exe`: the app
/// binary that records the payloads; `None`, a path that is not a file, or
/// a path holding a quote gives the `echo` form (words only).
pub fn hooks_json(log: &Path, exe: Option<&Path>) -> Value {
    let log = display_dir(log);
    let exe = exe
        .filter(|e| e.is_file())
        .map(display_dir)
        .filter(|e| !e.contains('"') && !log.contains('"'));
    let hook = |word: &str| match &exe {
        Some(exe) => notify(word, &log, exe),
        None => echo(word, &log),
    };
    json!({ "hooks": {
        "SessionStart": hook("start"),
        "UserPromptSubmit": hook("submit"),
        "Stop": hook("stop"),
        "Notification": hook("notify"),
    }})
}

/// The bytes of `path` from `offset` (empty when missing or not longer).
fn read_from(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    if f.metadata()?.len() <= offset {
        return Ok(Vec::new());
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Reads hooks.log incrementally (from the end of the complete lines read).
#[derive(Debug)]
pub struct HookLog {
    path: PathBuf,
    offset: u64,
}

impl HookLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path, offset: 0 }
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

    /// [`HookLog::poll`] with each line's payload (the text after its word).
    pub fn poll_lines(&mut self) -> AppResult<Vec<HookLine>> {
        let bytes = read_from(&self.path, self.offset)?;
        let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else {
            return Ok(Vec::new());
        };
        self.offset += u64::try_from(end + 1).unwrap_or(u64::MAX);
        let text = String::from_utf8_lossy(&bytes[..=end]);
        Ok(text
            .lines()
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
    /// A line without a payload.
    pub fn bare(hook: Hook) -> Self {
        HookLine {
            hook,
            payload: None,
        }
    }

    /// The payload as a JSON object.
    pub fn json(&self) -> Option<Value> {
        let v: Value = serde_json::from_str(self.payload.as_deref()?).ok()?;
        v.is_object().then_some(v)
    }

    /// A string field of the payload.
    pub fn field(&self, key: &str) -> Option<String> {
        self.json()?.get(key)?.as_str().map(str::to_string)
    }

    /// The reply's final text: Claude Code's Stop `last_assistant_message`,
    /// Codex's notify `last-assistant-message`.
    pub fn final_text(&self) -> Option<String> {
        self.field("last_assistant_message")
            .or_else(|| self.field("last-assistant-message"))
    }

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
        let v = hooks_json(Path::new("/tmp/s1/hooks.log"), None);
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
    fn hooks_json_uses_the_app_binary_when_it_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("app.exe");
        std::fs::write(&exe, b"x").unwrap();
        let v = hooks_json(Path::new("/tmp/s1/hooks.log"), Some(&exe));
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(
            cmd.ends_with("--notify \"/tmp/s1/hooks.log\" --event stop --stdin"),
            "{cmd}"
        );
        assert!(cmd.starts_with('"') && !cmd.contains('\\'));
        let missing = tmp.path().join("gone.exe");
        let v = hooks_json(Path::new("/tmp/s1/hooks.log"), Some(&missing));
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.starts_with("echo "));
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
    fn payload_fields_and_final_text() {
        let stop = HookLine {
            hook: Hook::Stop,
            payload: Some(
                r#"{"transcript_path":"C:/t.jsonl","last_assistant_message":"Which?"}"#.into(),
            ),
        };
        assert_eq!(stop.final_text().as_deref(), Some("Which?"));
        assert_eq!(stop.field("transcript_path").as_deref(), Some("C:/t.jsonl"));
        assert!(stop.stops_turn("anything"));
        let codex = HookLine {
            hook: Hook::Stop,
            payload: Some(r#"{"last-assistant-message":"ok"}"#.into()),
        };
        assert_eq!(codex.final_text().as_deref(), Some("ok"));
        assert_eq!(HookLine::bare(Hook::Stop).final_text(), None);
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
        assert!(!HookLine::bare(Hook::Submit).stops_turn("x"));
    }
}
