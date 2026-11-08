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
const TAIL_BYTES: u64 = 2 * 1024 * 1024;
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
/// Each entry keeps the file offset its line starts at.
#[derive(Debug)]
pub struct Transcript {
    path: PathBuf,
    len: u64,
    entries: Vec<(u64, Value)>,
}

/// Path text compared per platform (Windows paths are case-insensitive).
fn comparable(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// `path` looks like a transcript the app may read: absolute, `.jsonl`, no
/// `..`, not a network or device path, and inside one of `roots` (the
/// user's home folder, the app's session folders).
pub fn acceptable(path: &Path, roots: &[PathBuf]) -> bool {
    use std::path::Component;
    let shape = path.is_absolute()
        && path.extension().is_some_and(|e| e == "jsonl")
        && !path.components().any(|c| matches!(c, Component::ParentDir))
        && !crate::paths::is_unc_or_device(&path.to_string_lossy());
    if !shape {
        return false;
    }
    let p = comparable(path);
    roots.iter().any(|r| {
        let r = comparable(r);
        let r = r.trim_end_matches('/');
        !r.is_empty() && p.starts_with(&format!("{r}/"))
    })
}

impl Transcript {
    /// A transcript at `path` when [`acceptable`] under `roots`.
    pub fn new(path: PathBuf, roots: &[PathBuf]) -> Option<Self> {
        if !acceptable(&path, roots) {
            log::warn!("ignoring transcript path {}", path.display());
            return None;
        }
        Some(Self {
            path,
            len: 0,
            entries: Vec::new(),
        })
    }

    /// The file length (where the next entry will start).
    pub fn file_len(&self) -> u64 {
        std::fs::metadata(&self.path).map_or(self.len, |m| m.len())
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
                let start = meta.len().saturating_sub(TAIL_BYTES);
                self.entries = parse_lines_at(&text, start, meta.len() > TAIL_BYTES);
                self.len = meta.len();
            }
            Err(e) => log::debug!("reading {}: {e}", self.path.display()),
        }
    }

    pub fn pending_tool(&self) -> Option<PendingTool> {
        pending_tool(&self.values())
    }

    fn values(&self) -> Vec<Value> {
        self.entries.iter().map(|(_, v)| v.clone()).collect()
    }

    /// The last assistant text of the current turn: entries from offset
    /// `since` (the turn's prompt), else after the last typed prompt.
    pub fn last_assistant_text_since(&self, since: Option<u64>) -> Option<String> {
        let entries: Vec<Value> = match since {
            Some(at) => self
                .entries
                .iter()
                .filter(|(o, _)| *o >= at)
                .map(|(_, v)| v.clone())
                .collect(),
            None => {
                let all = self.values();
                let from = turn_start(&all);
                all[from..].to_vec()
            }
        };
        last_assistant_text(&entries)
    }
}

/// A user entry the user typed (text, not tool results).
fn is_typed_prompt(entry: &Value) -> bool {
    if role(entry) != Some("user") {
        return false;
    }
    match entry.pointer("/message/content") {
        Some(Value::String(_)) => true,
        Some(Value::Array(blocks)) => blocks
            .iter()
            .any(|b| b.get("type").and_then(Value::as_str) == Some("text")),
        _ => false,
    }
}

/// Index of the first entry of the last turn (after the last typed prompt).
fn turn_start(entries: &[Value]) -> usize {
    entries
        .iter()
        .rposition(is_typed_prompt)
        .map_or(0, |i| i + 1)
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
    parse_lines_at(text, 0, cut)
        .into_iter()
        .map(|(_, v)| v)
        .collect()
}

/// [`parse_lines`] with each line's file offset (`start`: offset of `text`).
fn parse_lines_at(text: &str, start: u64, cut: bool) -> Vec<(u64, Value)> {
    let complete = match text.rfind('\n') {
        Some(i) => &text[..=i],
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut at = start;
    for (i, line) in complete.split_inclusive('\n').enumerate() {
        let here = at;
        at += line.len() as u64;
        if i == 0 && cut {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
            if v.is_object() {
                out.push((here, v));
            }
        }
    }
    out
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

/// The last tool call of the current turn whose result is not in the
/// entries after it (entries whose calls all have results are passed over:
/// parallel calls are separate entries).
pub fn pending_tool(entries: &[Value]) -> Option<PendingTool> {
    let mut answered: Vec<&str> = Vec::new();
    let from = turn_start(entries);
    for entry in entries[from..].iter().rev() {
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
                let open = uses.into_iter().rev().find_map(|b| {
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
                if open.is_some() {
                    return open;
                }
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

    fn user(text: &str) -> Value {
        json!({"type": "user", "message": {"role": "user", "content": text}})
    }

    fn tool_use(id: &str) -> Value {
        assistant(json!([{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": id}}]))
    }

    fn result(id: &str) -> Value {
        json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": id, "content": "ok"}]}})
    }

    #[test]
    fn an_answered_parallel_call_does_not_hide_the_waiting_one() {
        // Parallel calls are separate entries: t1 waits behind the dialog,
        // t2 (written later) already has its result.
        let entries = vec![user("go"), tool_use("t1"), tool_use("t2"), result("t2")];
        assert_eq!(pending_tool(&entries).unwrap().id, "t1");
        // A call of an earlier turn is not this turn's.
        let entries = vec![tool_use("t0"), user("next"), assistant(json!([]))];
        assert_eq!(pending_tool(&entries), None);
    }

    #[test]
    fn the_fallback_text_is_the_current_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        let first = format!(
            "{}\n{}\n",
            user("one"),
            assistant(json!([{"type": "text", "text": "Which framework?"}]))
        );
        std::fs::write(&path, &first).unwrap();
        let mut t = Transcript::new(path.clone(), &[tmp.path().to_path_buf()]).unwrap();
        let since = t.file_len();
        let second = format!("{first}{}\n{}\n", user("two"), tool_use("t9"));
        std::fs::write(&path, second).unwrap();
        t.refresh();
        // The turn from `since` ended with tool calls only: no text, never
        // the previous turn's question.
        assert_eq!(t.last_assistant_text_since(Some(since)), None);
        assert_eq!(t.last_assistant_text_since(None), None);
        assert_eq!(
            t.last_assistant_text_since(Some(0)).as_deref(),
            Some("Which framework?")
        );
    }

    #[test]
    fn transcripts_outside_the_allowed_roots_are_refused() {
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\Users\me"
        } else {
            "/home/me"
        });
        let roots = [home.clone()];
        assert!(acceptable(&home.join(".claude/projects/x/s.jsonl"), &roots));
        assert!(!acceptable(&home.join("../other/s.jsonl"), &roots));
        let elsewhere = if cfg!(windows) {
            r"D:\g\s.jsonl"
        } else {
            "/srv/g/s.jsonl"
        };
        assert!(!acceptable(Path::new(elsewhere), &roots));
        assert!(!acceptable(Path::new(r"\\server\share\s.jsonl"), &roots));
        assert!(!acceptable(&home.join("s.txt"), &roots));
        if cfg!(windows) {
            assert!(acceptable(Path::new(r"c:\users\ME\p\s.jsonl"), &roots));
        }
    }

    #[test]
    fn tail_reading_skips_partial_and_bad_lines() {
        let text = "{\"a\":1}\n{bad\n[1]\n{\"b\":2}\n{\"c\":";
        assert_eq!(parse_lines(text, false).len(), 2);
        assert_eq!(parse_lines(text, true).len(), 1);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        let roots = [tmp.path().to_path_buf()];
        assert!(Transcript::new(PathBuf::from("rel.jsonl"), &roots).is_none());
        assert!(Transcript::new(tmp.path().join("x.txt"), &roots).is_none());
        let mut t = Transcript::new(path.clone(), &roots).unwrap();
        t.refresh();
        assert_eq!(t.pending_tool(), None);
        std::fs::write(&path, format!("{}\n", ask())).unwrap();
        t.refresh();
        assert_eq!(t.pending_tool().unwrap().name, ASK_TOOL);
    }
}
