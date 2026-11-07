//! Terminal output as text: UTF-8 across chunk borders, escape sequences
//! removed, and the dialogs Claude Code shows before it accepts input.

use std::sync::OnceLock;

use regex::Regex;

/// Screen text (lowercase, escapes removed) of a dialog that waits for the user.
pub const DIALOG_MARKERS: [&str; 3] = ["enter to confirm", "do you trust", "trust this folder"];
const SCAN_TAIL: usize = 64;

/// `pending` + `chunk` as text; an incomplete trailing UTF-8 sequence stays in
/// `pending`, invalid bytes become U+FFFD.
pub fn take_utf8(pending: &mut Vec<u8>, chunk: &[u8]) -> String {
    pending.extend_from_slice(chunk);
    let bytes = std::mem::take(pending);
    let mut text = String::with_capacity(bytes.len());
    let mut rest: &[u8] = &bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                text.push_str(s);
                return text;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                text.push_str(&String::from_utf8_lossy(valid));
                match e.error_len() {
                    None => {
                        *pending = after.to_vec();
                        return text;
                    }
                    Some(n) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        rest = &after[n..];
                    }
                }
            }
        }
    }
}

/// The last `max` chars of `text` as plain text (escapes removed, whitespace
/// collapsed), for failure reasons.
pub fn plain_tail(text: &str, max: usize) -> String {
    let plain = escapes().replace_all(text, " ");
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let start = plain
        .char_indices()
        .rev()
        .nth(max.saturating_sub(1))
        .map_or(0, |(i, _)| i);
    plain[start..].to_string()
}

fn escapes() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-9;?<>=]*[ -/]*[@-~]|\x1b.")
            .expect("valid escape regex")
    })
}

/// Number of [`DIALOG_MARKERS`] that end inside `text`, which follows the
/// screen text in `tail` (updated to the last few chars).
pub fn count_dialogs(tail: &mut String, text: &str) -> u64 {
    let plain = escapes().replace_all(text, " ");
    let plain: String = plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if plain.is_empty() {
        return 0;
    }
    let joined = format!("{tail} {plain}");
    let fresh_from = tail.len() + 1;
    let found = DIALOG_MARKERS
        .iter()
        .flat_map(|m| joined.match_indices(m).map(move |(i, _)| i + m.len()))
        .filter(|end| *end > fresh_from)
        .count();
    let start = joined
        .char_indices()
        .rev()
        .nth(SCAN_TAIL)
        .map_or(0, |(i, _)| i);
    *tail = joined[start..].to_string();
    u64::try_from(found).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_across_chunks_is_held_back() {
        let bytes = "a✻b".as_bytes();
        let mut pending = Vec::new();
        assert_eq!(take_utf8(&mut pending, &bytes[..2]), "a");
        assert_eq!(pending.len(), 1);
        assert_eq!(take_utf8(&mut pending, &bytes[2..]), "✻b");
        assert!(pending.is_empty());
        assert_eq!(take_utf8(&mut pending, b"x\xffy"), "x\u{fffd}y");
    }

    #[test]
    fn dialogs_are_counted_through_escapes_and_chunk_borders() {
        let mut tail = String::new();
        let screen = "\x1b[1mQuick\x1b[1Csafety\x1b[1Ccheck\x1b[m ❯ No, exit  Yes, I \x1b[1Ctrust";
        assert_eq!(count_dialogs(&mut tail, screen), 0);
        assert_eq!(
            count_dialogs(&mut tail, " this folder \x1b]0;t\x07 Enter to"),
            1
        );
        assert_eq!(count_dialogs(&mut tail, "\x1b[1C confirm · Esc"), 1);
        assert_eq!(count_dialogs(&mut tail, "> plain prompt"), 0);
    }

    #[test]
    fn plain_tail_drops_escapes() {
        assert_eq!(plain_tail("\x1b[1mab\x1b[1Ccd  \r\n ef", 4), "d ef");
        assert_eq!(plain_tail("x", 10), "x");
    }
}
