//! A node's turn as text: the transcript span from the moment its prompt is
//! typed to its Stop hook, escapes removed and the TUI's redraws collapsed,
//! saved as `runs/<run>/<node>/reply.md` (and `result.md`, so the Results tab
//! and `read_node_result` show it). The raw bytes stay in `transcript.log`.
//!
//! Claude Code redraws its screen many times per second, so the raw span holds
//! every intermediate state. Escapes go the way [`super::screen`] matches
//! them, except that a cursor move to another row starts a new line and
//! colours / erase-line vanish (so a screen row stays one line). Then:
//! - consecutive duplicate lines (blank lines in between do not count) and
//!   lines that are a strict prefix of the next line (text growing as it is
//!   typed or streamed) are dropped;
//! - a line of at least [`WINDOW_MIN_CHARS`] chars already kept among the last
//!   [`WINDOW`] kept lines is dropped (the TUI redraws its status block and
//!   footer between the lines that change);
//! - horizontal-rule lines and runs of blank lines are removed.

use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::contract::AppResult;
use crate::run_store::RESULT_FILE;
use crate::store::write_atomic;
use crate::terminal::screen::escapes;

pub const REPLY_FILE: &str = "reply.md";
/// reply.md is cut to its last this many bytes.
pub const REPLY_CAP: usize = 256 * 1024;
/// At most this many bytes of the transcript span are read (its end).
const SPAN_READ_CAP: u64 = 16 * 1024 * 1024;
const CUT_NOTE: &str = "[… earlier output cut]\n";
/// Characters a horizontal rule of the TUI is drawn with.
const RULE_CHARS: [char; 4] = ['\u{2500}', '\u{2501}', '\u{2550}', '\u{2574}'];
/// Kept lines a repeated line is looked for in …
const WINDOW: usize = 60;
/// … if it has at least this many chars (trimmed): short lines such as `}`
/// repeat legitimately.
const WINDOW_MIN_CHARS: usize = 8;

/// One escape sequence as text: see the module docs.
fn escape_text(seq: &str) -> &'static str {
    let Some(params) = seq.strip_prefix("\x1b[") else {
        return " ";
    };
    match params.chars().last() {
        Some('H' | 'f' | 'A' | 'B' | 'E' | 'F' | 'd') => "\n",
        Some('m' | 'K') => "",
        _ => " ",
    }
}

/// `raw` terminal output as lines of plain text (trailing blanks trimmed).
fn plain_lines(raw: &str) -> Vec<String> {
    let plain: Cow<'_, str> =
        escapes().replace_all(raw, |c: &regex::Captures<'_>| escape_text(&c[0]));
    plain
        .replace("\r\n", "\n")
        .split(['\n', '\r'])
        .map(|l| {
            l.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| RULE_CHARS.contains(&c))
}

/// `line` (trimmed) is among the last [`WINDOW`] non-blank lines of `out`.
fn recently_kept(out: &[String], line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= WINDOW_MIN_CHARS
        && out
            .iter()
            .rev()
            .filter(|l| !l.is_empty())
            .take(WINDOW)
            .any(|l| l.trim() == t)
}

/// Drop the last non-blank lines of `out` that `line` grew from (strict
/// prefixes), with the blank lines after them.
fn drop_prefixes(out: &mut Vec<String>, line: &str) {
    while let Some(j) = out.iter().rposition(|l| !l.is_empty()) {
        if line.starts_with(out[j].as_str()) {
            out.truncate(j);
        } else {
            break;
        }
    }
}

/// Collapse the redraws of `lines`: see the module docs.
pub fn dedup_lines(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if is_rule(line) {
            continue;
        }
        if line.trim().is_empty() {
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let last = out.iter().rev().find(|l| !l.is_empty());
        if last.is_some_and(|l| l.trim() == line.trim()) || recently_kept(&out, line) {
            continue;
        }
        drop_prefixes(&mut out, line);
        out.push(line.clone());
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// The input box's echo of `prompt`: the line without the prompt glyph.
fn is_prompt_echo(line: &str, prompt: &str) -> bool {
    let t = line
        .trim_start_matches(|c: char| c.is_whitespace() || c == '>' || c == '\u{276f}')
        .trim();
    !prompt.is_empty() && t == prompt
}

/// reply.md text: `> <first line of the typed prompt>`, a blank line, then the
/// de-duplicated screen text of `raw`; cut to its last [`REPLY_CAP`] bytes.
pub fn reply_text(raw: &str, prompt: &str) -> String {
    let first = prompt.lines().next().unwrap_or("").trim();
    let head = format!("> {first}\n\n");
    let body: Vec<String> = dedup_lines(&plain_lines(raw))
        .into_iter()
        .filter(|l| !is_prompt_echo(l, first))
        .collect();
    let body = body.join("\n");
    let body = body.trim_start_matches('\n');
    let room = REPLY_CAP.saturating_sub(head.len() + CUT_NOTE.len() + 1);
    if body.len() <= room {
        return format!("{head}{body}\n");
    }
    let mut cut = body.len() - room;
    while !body.is_char_boundary(cut) {
        cut += 1;
    }
    let tail = &body[cut..];
    let tail = tail.find('\n').map_or(tail, |i| &tail[i + 1..]);
    format!("{head}{CUT_NOTE}{tail}\n")
}

/// Bytes `from..to` of `transcript` (at most the last [`SPAN_READ_CAP`]).
fn read_span(transcript: &Path, from: u64, to: u64) -> std::io::Result<String> {
    let from = from.max(to.saturating_sub(SPAN_READ_CAP));
    let mut f = File::open(transcript)?;
    f.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    f.take(to.saturating_sub(from)).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Write `reply.md` and `result.md` into `node_run_dir` from the transcript
/// span `from..to` of the turn that answered `prompt`.
pub fn write_reply(
    node_run_dir: &Path,
    transcript: &Path,
    from: u64,
    to: u64,
    prompt: &str,
) -> AppResult<()> {
    let raw = read_span(transcript, from, to)?;
    let text = reply_text(&raw, prompt);
    write_atomic(&node_run_dir.join(REPLY_FILE), text.as_bytes())?;
    write_atomic(&node_run_dir.join(RESULT_FILE), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| (*l).to_string()).collect()
    }

    #[test]
    fn duplicates_prefixes_rules_and_blank_runs_collapse() {
        let got = dedup_lines(&lines(&[
            "",
            "H",
            "He",
            "Hello",
            "",
            "Hello",
            "\u{2500}\u{2500}\u{2500}",
            "",
            "",
            "World",
            "World",
            "  >",
            "Done",
            "",
        ]));
        assert_eq!(got, lines(&["Hello", "", "World", "  >", "Done"]));
    }

    #[test]
    fn redrawn_status_lines_drop_but_short_repeats_stay() {
        let got = dedup_lines(&lines(&[
            "Context 7% | Usage 3%",
            "fn a() {",
            "}",
            "Context 7% | Usage 3%",
            "fn b() {",
            "}",
        ]));
        assert_eq!(
            got,
            lines(&["Context 7% | Usage 3%", "fn a() {", "}", "fn b() {", "}"])
        );
    }

    #[test]
    fn row_moves_split_lines_and_colours_vanish() {
        let raw = "\x1b[1mA\x1b[22m\x1b[1Cb\x1b]0;title\x07\r\n\x1b[Kend\x1b[3;1Hrow\rover";
        assert_eq!(plain_lines(raw), lines(&["A b", "end", "row", "over"]));
    }

    #[test]
    fn reply_starts_with_the_prompt_and_is_capped() {
        let raw = "> do it\r\n\x1b[2;1H> do it\r\nyou said: do it\r\nok\r\n";
        assert_eq!(
            reply_text(raw, "do it\nsecond line"),
            "> do it\n\nyou said: do it\nok\n"
        );
        let big: String = (0..40_000).map(|i| format!("line {i}\n")).collect();
        let text = reply_text(&big, "p");
        assert!(text.len() <= REPLY_CAP, "{}", text.len());
        assert!(text.starts_with("> p\n\n[… earlier output cut]\nline "));
        assert!(text.ends_with("line 39999\n"));
    }

    /// 62 lines of a real Claude Code 2.1.319 transcript (ConPTY, Windows 11):
    /// a web-search turn mid-way, with the spinner and box redraws.
    #[test]
    fn real_transcript_sample_collapses() {
        let raw = String::from_utf8_lossy(include_bytes!(
            "../../tests/fixtures/terminal_reply_sample.txt"
        ))
        .into_owned();
        let before: Vec<String> = plain_lines(&raw)
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let after = dedup_lines(&plain_lines(&raw));
        let text: Vec<&String> = after.iter().filter(|l| !l.is_empty()).collect();
        assert!(
            text.len() * 10 < before.len() * 9,
            "{} vs {}",
            text.len(),
            before.len()
        );
        let bytes = |v: &[&String]| v.iter().map(|l| l.len()).sum::<usize>();
        let before_refs: Vec<&String> = before.iter().collect();
        assert!(bytes(&text) * 10 < bytes(&before_refs) * 9);
        let footer = "Context \u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591} 7%";
        let count = |v: &[&String]| v.iter().filter(|l| l.contains(footer)).count();
        assert!(count(&before_refs) > count(&text), "redrawn footer kept");
        for w in text.windows(2) {
            assert_ne!(w[0].trim(), w[1].trim(), "duplicate kept");
            assert!(!w[1].starts_with(w[0].as_str()), "prefix kept: {:?}", w[0]);
        }
        assert!(text.iter().all(|l| !is_rule(l)));
        let joined = after.join("\n");
        assert!(joined.contains("Did 1 search in 6s"), "{joined}");
        assert!(joined.contains("1. Agent execution design (most common)."));
        assert!(!joined.contains("\x1b"));
    }
}
