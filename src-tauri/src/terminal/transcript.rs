//! Claude Code's own transcript (`~/.claude/projects/<cwd>/<session>.jsonl`,
//! path from the hook payloads' `transcript_path`). The app reads its tail,
//! only when the file grew, to see a tool call waiting for the user (an
//! assistant `tool_use` with no `tool_result` yet: AskUserQuestion's exact
//! questions, or the tool behind a permission prompt) and the turn's final
//! assistant text. Verified 2026-11-08 (docs/CLAUDE_CLI_NOTES.md): the
//! `tool_use` entry is written before the dialog waits for input.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Bytes read from the end of the file.
const TAIL_BYTES: u64 = 512 * 1024;
/// AskUserQuestion, the tool whose call is itself a dialog.
pub const ASK_TOOL: &str = "AskUserQuestion";

/// A tool call without its result.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTool {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// The tail of a transcript JSONL, re-read when the file length changes.
#[derive(Debug)]
pub struct Transcript {
    path: PathBuf,
    len: u64,
    entries: Vec<Value>,
}

/// `path` looks like a transcript the app may read: absolute, `.jsonl`.
pub fn acceptable(path: &Path) -> bool {
    path.is_absolute() && path.extension().is_some_and(|e| e == "jsonl")
}

impl Transcript {
    pub fn new(path: PathBuf) -> Option<Self> {
        acceptable(&path).then_some(Self {
            path,
            len: 0,
            entries: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-read the tail if the file changed; read errors leave the old tail.
    pub fn refresh(&mut self) {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return;
        };
        if meta.len() == self.len || !meta.is_file() {
            return;
        }
        match read_tail(&self.path, meta.len()) {
            Ok(text) => {
                self.entries = parse_lines(&text, meta.len() > TAIL_BYTES);
                self.len = meta.len();
            }
            Err(e) => log::debug!("reading {}: {e}", self.path.display()),
        }
    }

    pub fn pending_tool(&self) -> Option<PendingTool> {
        pending_tool(&self.entries)
    }

    pub fn last_assistant_text(&self) -> Option<String> {
        last_assistant_text(&self.entries)
    }
}

fn read_tail(path: &Path, len: u64) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(TAIL_BYTES).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// JSON objects of `text`'s complete lines (the first one dropped when the
/// read started mid-file).
pub fn parse_lines(text: &str, cut: bool) -> Vec<Value> {
    let complete = match text.rfind('\n') {
        Some(i) => &text[..i],
        None => return Vec::new(),
    };
    complete
        .lines()
        .skip(usize::from(cut))
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .filter(Value::is_object)
        .collect()
}

fn role(entry: &Value) -> Option<&str> {
    entry
        .pointer("/message/role")
        .and_then(Value::as_str)
        .or_else(|| entry.get("type").and_then(Value::as_str))
}

fn blocks(entry: &Value) -> &[Value] {
    entry
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The last tool call whose result is not in the entries after it.
pub fn pending_tool(entries: &[Value]) -> Option<PendingTool> {
    let mut answered: Vec<&str> = Vec::new();
    for entry in entries.iter().rev() {
        match role(entry) {
            Some("user") => answered.extend(
                blocks(entry)
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                    .filter_map(|b| b.get("tool_use_id").and_then(Value::as_str)),
            ),
            Some("assistant") => {
                let uses: Vec<&Value> = blocks(entry)
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .collect();
                if uses.is_empty() {
                    // A text reply after the call: the call was answered.
                    if blocks(entry)
                        .iter()
                        .any(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    {
                        return None;
                    }
                    continue;
                }
                return uses.into_iter().rev().find_map(|b| {
                    let id = b.get("id").and_then(Value::as_str)?;
                    (!answered.contains(&id)).then(|| PendingTool {
                        id: id.to_string(),
                        name: b
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        input: b.get("input").cloned().unwrap_or(Value::Null),
                    })
                });
            }
            _ => {}
        }
    }
    None
}

/// Text of the last assistant message (its text blocks, joined).
pub fn last_assistant_text(entries: &[Value]) -> Option<String> {
    entries.iter().rev().find_map(|e| {
        if role(e) != Some("assistant") {
            return None;
        }
        let text: Vec<&str> = blocks(e)
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect();
        (!text.is_empty()).then(|| text.join("\n\n"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(content: Value) -> Value {
        json!({"type": "assistant", "message": {"role": "assistant", "content": content}})
    }

    fn ask() -> Value {
        assistant(
            json!([{"type": "tool_use", "id": "t1", "name": "AskUserQuestion",
            "input": {"questions": [{"question": "A or B?", "header": "AB", "multiSelect": false,
            "options": [{"label": "A", "description": "a"}, {"label": "B", "description": "b"}]}]}}]),
        )
    }

    #[test]
    fn a_tool_use_without_result_is_pending() {
        let entries = vec![
            json!({"type": "ai-title"}),
            ask(),
            json!({"type": "tools-used"}),
        ];
        let p = pending_tool(&entries).unwrap();
        assert_eq!((p.id.as_str(), p.name.as_str()), ("t1", ASK_TOOL));
        assert_eq!(p.input["questions"][0]["options"][1]["label"], "B");
        let mut done = entries.clone();
        done.push(
            json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "B"}]}}),
        );
        assert_eq!(pending_tool(&done), None);
        done.push(assistant(
            json!([{"type": "text", "text": "You chose **B**."}]),
        ));
        assert_eq!(
            last_assistant_text(&done).as_deref(),
            Some("You chose **B**.")
        );
        assert_eq!(pending_tool(&[]), None);
    }

    #[test]
    fn tail_reading_skips_partial_and_bad_lines() {
        let text = "{\"a\":1}\n{bad\n[1]\n{\"b\":2}\n{\"c\":";
        assert_eq!(parse_lines(text, false).len(), 2);
        assert_eq!(parse_lines(text, true).len(), 1);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        assert!(Transcript::new(PathBuf::from("rel.jsonl")).is_none());
        assert!(Transcript::new(tmp.path().join("x.txt")).is_none());
        let mut t = Transcript::new(path.clone()).unwrap();
        t.refresh();
        assert_eq!(t.pending_tool(), None);
        std::fs::write(&path, format!("{}\n", ask())).unwrap();
        t.refresh();
        assert_eq!(t.pending_tool().unwrap().name, ASK_TOOL);
    }
}
