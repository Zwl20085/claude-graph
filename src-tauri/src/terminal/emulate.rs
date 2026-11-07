//! A node's turn as the terminal showed it: the session transcript replayed
//! into a terminal emulator ([`vt100`]), so cursor moves, erase-line and
//! repaints land on their cells exactly as in the window.
//!
//! The transcript is replayed from its start (at most its last
//! [`EMULATE_CAP`] bytes) into a screen of the session's size, resized at
//! the transcript offsets where the window was resized ([`TermSize`]), with
//! [`SCROLLBACK_ROWS`] rows of scrollback. The scrollback + screen lines are
//! taken once when the replay reaches the turn's start and once at its end;
//! the turn's text is what follows the two lists' common prefix (by line
//! position), cut before the idle input box (an empty `>` / `❯` line, or
//! Codex's `› Ask Codex to do anything` composer, among
//! the last [`CHROME_ROWS`] lines, blank screen rows not counted: the box
//! and the footer under it are not the turn), lines drawn only with
//! box-drawing characters dropped, runs of
//! blank lines collapsed and trailing blank lines trimmed.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroU16;
use std::path::Path;

/// At most this many bytes of the transcript (its end) are replayed.
pub const EMULATE_CAP: u64 = 16 * 1024 * 1024;
/// Rows of scrollback the emulator keeps.
pub const SCROLLBACK_ROWS: usize = 10_000;
/// The idle input box (an empty prompt line) is looked for among this many
/// last lines; it and everything below it (the footer) is not the turn.
const CHROME_ROWS: usize = 12;

/// Box-drawing characters (U+2500–U+257F).
fn is_box(c: char) -> bool {
    ('\u{2500}'..='\u{257f}').contains(&c)
}

/// A line drawn only with box-drawing characters (and spaces).
pub fn is_box_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| is_box(c) || c.is_whitespace())
}

/// The input box with nothing typed: `>` or `❯` alone.
fn is_empty_prompt(line: &str) -> bool {
    let t = line.trim();
    matches!(t, ">" | "\u{276f}" | "\u{203a}")
        || t.strip_prefix('\u{203a}').is_some_and(|rest| {
            crate::harness::codex::IDLE
                .iter()
                .any(|p| rest.trim() == *p)
        })
}

/// Every scrollback line, oldest first, then every screen row (trailing
/// blanks trimmed). Leaves the view at the live screen.
fn all_lines(parser: &mut vt100::Parser) -> Vec<String> {
    let screen = parser.screen_mut();
    let (rows, cols) = screen.size();
    let rows = usize::from(rows.get());
    let cols = cols.get();
    let mut out = Vec::with_capacity(screen.scrollback_len() + rows);
    let mut offset = screen.scrollback_len();
    loop {
        screen.set_scrollback(offset);
        // At `offset`, the view's first min(offset, rows) rows are
        // scrollback lines len-offset.. ; at 0 it is the live screen.
        let take = if offset == 0 { rows } else { offset.min(rows) };
        out.extend(
            screen
                .rows(0, cols)
                .take(take)
                .map(|r| r.trim_end().to_string()),
        );
        if offset == 0 {
            break;
        }
        offset -= take;
    }
    screen.set_scrollback(0);
    out
}

/// The lines of `after` past its common prefix with `before`: see the
/// module docs.
pub fn new_lines(before: &[String], after: &[String]) -> Vec<String> {
    let common = before.iter().zip(after).take_while(|(b, a)| b == a).count();
    let new = &after[common..];
    let used = new
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    let new = &new[..used];
    let chrome = new
        .iter()
        .enumerate()
        .rev()
        .take(CHROME_ROWS)
        .find(|(_, l)| is_empty_prompt(l))
        .map_or(new.len(), |(i, _)| i);
    let mut out: Vec<String> = Vec::new();
    for line in &new[..chrome] {
        if is_box_rule(line) {
            continue;
        }
        let blank = line.trim().is_empty();
        if blank && !out.last().is_some_and(|l| !l.is_empty()) {
            continue;
        }
        out.push(if blank { String::new() } else { line.clone() });
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// A terminal size from transcript byte `at` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub at: u64,
    pub cols: u16,
    pub rows: u16,
}

impl TermSize {
    fn dims(self) -> (NonZeroU16, NonZeroU16) {
        (
            NonZeroU16::new(self.rows).unwrap_or(vt100::Parser::DEFAULT_ROWS),
            NonZeroU16::new(self.cols).unwrap_or(vt100::Parser::DEFAULT_COLS),
        )
    }
}

/// The size in force at transcript byte `at`: the last of `sizes` (in `at`
/// order) starting at or before it, else the first, else 80×24.
fn size_at(sizes: &[TermSize], at: u64) -> TermSize {
    sizes
        .iter()
        .rev()
        .find(|s| s.at <= at)
        .or(sizes.first())
        .copied()
        .unwrap_or(TermSize {
            at: 0,
            cols: 80,
            rows: 24,
        })
}

/// Feed `bytes` (transcript bytes from offset `base`) to `parser`, applying
/// each size change of `sizes` at its offset.
fn replay(parser: &mut vt100::Parser, bytes: &[u8], base: u64, sizes: &[TermSize]) {
    let end = base + bytes.len() as u64;
    let mut pos = base;
    for s in sizes.iter().filter(|s| s.at >= base && s.at < end) {
        let cut = usize::try_from(s.at - base).unwrap_or(bytes.len());
        let done = usize::try_from(pos - base).unwrap_or(bytes.len());
        parser.process(&bytes[done..cut]);
        let (rows, cols) = s.dims();
        parser.set_size(rows, cols);
        pos = s.at;
    }
    let done = usize::try_from(pos - base).unwrap_or(bytes.len());
    parser.process(&bytes[done..]);
}

/// Replay `bytes` (the transcript from byte `base` on) and return the lines
/// that appeared from transcript byte `from` to the end; the screen takes
/// each of `sizes` from its offset on. See the module docs.
pub fn lines_between(bytes: &[u8], base: u64, from: u64, sizes: &[TermSize]) -> Vec<String> {
    let from = from.clamp(base, base + bytes.len() as u64);
    let split = usize::try_from(from - base).unwrap_or(bytes.len());
    let (rows, cols) = size_at(sizes, base).dims();
    let mut parser = vt100::Parser::new(rows, cols, SCROLLBACK_ROWS);
    replay(&mut parser, &bytes[..split], base, sizes);
    let before = all_lines(&mut parser);
    replay(&mut parser, &bytes[split..], from, sizes);
    let after = all_lines(&mut parser);
    new_lines(&before, &after)
}

/// Bytes `start..to` of `transcript`, `start` = `to` − [`EMULATE_CAP`] (or
/// 0), and `start`.
pub fn read_replay(transcript: &Path, to: u64) -> std::io::Result<(Vec<u8>, u64)> {
    let start = to.saturating_sub(EMULATE_CAP);
    let mut f = File::open(transcript)?;
    f.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    f.take(to - start).read_to_end(&mut bytes)?;
    Ok((bytes, start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(at: u64, cols: u16, rows: u16) -> TermSize {
        TermSize { at, cols, rows }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|l| (*l).to_string()).collect()
    }

    #[test]
    fn new_lines_follow_the_common_prefix() {
        let before = s(&["a", "b", "> ", "footer"]);
        let after = s(&[
            "a",
            "b",
            "> q",
            "",
            "",
            "\u{2500}\u{2500}",
            "answer",
            "",
            "\u{2500}\u{2500}",
            "\u{276f}\u{a0}",
            "\u{2500}\u{2500}",
            "footer",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
            "",
        ]);
        assert_eq!(new_lines(&before, &after), s(&["> q", "", "answer"]));
    }

    #[test]
    fn scrolled_lines_are_read_from_the_scrollback() {
        let text: String = (0..30).map(|i| format!("line {i}\r\n")).collect();
        let lines = lines_between(text.as_bytes(), 0, 0, &[size(0, 20, 5)]);
        assert_eq!(lines.first().map(String::as_str), Some("line 0"));
        assert_eq!(lines.last().map(String::as_str), Some("line 29"));
        assert_eq!(lines.len(), 30);
    }

    #[test]
    fn size_changes_apply_at_their_offset() {
        let head = "x\r\n";
        let wide = "0123456789";
        let text = format!("{head}{wide}");
        let sizes = [size(0, 5, 4), size(head.len() as u64, 12, 4)];
        let lines = lines_between(text.as_bytes(), 0, 0, &sizes);
        assert_eq!(lines, s(&["x", wide]));
        // From byte 3 on only the second line is new.
        let late = lines_between(text.as_bytes(), 0, 3, &sizes);
        assert_eq!(late, s(&[wide]));
    }
}
