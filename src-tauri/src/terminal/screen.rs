//! Terminal output as text: UTF-8 across chunk borders, escape sequences
//! removed, and [`Screen`]: what the end of the output shows (a dialog that
//! waits for the user, Claude Code's empty input box, the text typed into it).

use std::sync::{Arc, OnceLock};

use regex::Regex;

/// Screen text (lowercase, escapes removed) of a dialog that waits for the
/// user: Claude Code's workspace-trust and new-MCP-server dialogs and their
/// "Enter to confirm · Esc to cancel" footer.
pub const DIALOG_MARKERS: [&str; 7] = [
    "accessing workspace",
    "quick safety check",
    "trust this folder",
    "do you trust",
    "new mcp server",
    "enter to confirm",
    "esc to",
];
/// "esc to interrupt" (shown while Claude Code works) is not a dialog.
const ESC_TO: &str = "esc to";
const NOT_A_DIALOG_AFTER_ESC_TO: &str = " interrupt";
/// Claude Code's empty input box: the prompt glyph (`❯`, or `>` in some
/// consoles) followed by a no-break space (typed text follows a plain space,
/// a dialog's choice cursor a cursor move); and the footer hint of CLIs that
/// show it.
pub const IDLE_MARKERS: [&str; 3] = ["\u{276f}\u{a0}", ">\u{a0}", "? for shortcuts"];
/// The texts that mark a dialog and the empty input box of one CLI (lowercase,
/// normalized as [`normalize`] does). Claude Code's by default; other
/// harnesses bring their own (`crate::harness`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Markers {
    pub idle: Vec<String>,
    pub dialog: Vec<String>,
}

impl Markers {
    /// Claude Code's markers ([`IDLE_MARKERS`], [`DIALOG_MARKERS`]).
    pub fn claude() -> Arc<Markers> {
        static CLAUDE: OnceLock<Arc<Markers>> = OnceLock::new();
        CLAUDE
            .get_or_init(|| {
                Arc::new(Markers {
                    idle: IDLE_MARKERS.iter().map(|m| m.to_string()).collect(),
                    dialog: DIALOG_MARKERS.iter().map(|m| m.to_string()).collect(),
                })
            })
            .clone()
    }

    /// Markers from user or adapter texts (normalized; blanks dropped).
    pub fn of(idle: &[&str], dialog: &[&str]) -> Markers {
        let clean = |list: &[&str]| -> Vec<String> {
            list.iter()
                .map(|m| normalize(m))
                .filter(|m| !m.is_empty())
                .collect()
        };
        Markers {
            idle: clean(idle),
            dialog: clean(dialog),
        }
    }
}

/// The screen is the last this many lines of output …
pub const SCREEN_LINES: usize = 60;
/// … and at most this many bytes.
pub const SCREEN_BYTES: usize = 64 * 1024;
/// Chars of a message that must show in the input box to count as typed.
pub const VISIBLE_PREFIX: usize = 30;

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

pub(crate) fn escapes() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-9;?<>=]*[ -/]*[@-~]|\x1b.")
            .expect("valid escape regex")
    })
}

/// Lowercase text with escapes removed and every run of ASCII whitespace or
/// control characters as one space. A no-break space stays: it marks the
/// empty input box.
pub fn normalize(text: &str) -> String {
    let plain = escapes().replace_all(text, " ");
    let mut out = String::with_capacity(plain.len());
    let mut gap = false;
    for c in plain.chars() {
        if c.is_ascii_whitespace() || c.is_control() {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(' ');
        }
        gap = false;
        out.extend(c.to_lowercase());
    }
    out
}

/// The last [`SCREEN_LINES`] lines (at most [`SCREEN_BYTES`]) of `raw`.
pub fn screen_tail(raw: &str) -> &str {
    let mut start = raw.len().saturating_sub(SCREEN_BYTES);
    while !raw.is_char_boundary(start) {
        start += 1;
    }
    let tail = &raw[start..];
    match tail.rmatch_indices('\n').nth(SCREEN_LINES - 1) {
        Some((i, _)) => &tail[i + 1..],
        None => tail,
    }
}

/// What the end of the output shows, as normalized text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    text: String,
    markers: Arc<Markers>,
}

impl Screen {
    /// The screen of the output `raw` (only its [`screen_tail`] counts).
    pub fn of(raw: &str) -> Self {
        Self::of_all(screen_tail(raw))
    }

    /// All of `raw` (e.g. the output written since a mark).
    pub fn of_all(raw: &str) -> Self {
        Self {
            text: normalize(raw),
            markers: Markers::claude(),
        }
    }

    /// The same text judged with `markers`.
    pub fn with(self, markers: Arc<Markers>) -> Self {
        Self { markers, ..self }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// End of the last dialog marker.
    fn last_dialog(&self) -> Option<usize> {
        let t = &self.text;
        self.markers
            .dialog
            .iter()
            .map(String::as_str)
            .flat_map(|m| t.match_indices(m).map(move |(i, _)| (m, i + m.len())))
            .filter(|(m, end)| *m != ESC_TO || !t[*end..].starts_with(NOT_A_DIALOG_AFTER_ESC_TO))
            .map(|(_, end)| end)
            .max()
    }

    /// Start of the last idle marker.
    fn last_idle(&self) -> Option<usize> {
        self.markers
            .idle
            .iter()
            .filter_map(|m| self.text.rfind(m.as_str()))
            .max()
    }

    /// Any dialog marker on screen (open or not).
    pub fn has_dialog(&self) -> bool {
        self.last_dialog().is_some()
    }

    /// A dialog waits for the user: its marker comes after the last idle marker.
    pub fn dialog_open(&self) -> bool {
        match (self.last_dialog(), self.last_idle()) {
            (Some(d), Some(i)) => d > i,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Claude Code's empty input box shows, after the last dialog.
    pub fn idle(&self) -> bool {
        self.last_idle().is_some() && !self.dialog_open()
    }

    /// The first [`VISIBLE_PREFIX`] chars of `message` (normalized) show after
    /// the last empty input box, or anywhere when no box is on screen.
    pub fn input_shows(&self, message: &str) -> bool {
        let want: String = normalize(message).chars().take(VISIBLE_PREFIX).collect();
        let want = want.trim();
        if want.is_empty() {
            return false;
        }
        let from = self.last_idle().unwrap_or(0);
        self.text[from..].contains(want)
    }

    /// The first [`VISIBLE_PREFIX`] chars of `message` (normalized) show
    /// anywhere in this text.
    pub fn shows_prefix(&self, message: &str) -> bool {
        let want: String = normalize(message).chars().take(VISIBLE_PREFIX).collect();
        let want = want.trim();
        !want.is_empty() && self.text.contains(want)
    }

    /// `needle` (lowercase) is on screen.
    pub fn contains(&self, needle: &str) -> bool {
        self.text.contains(needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Excerpts of real Claude Code 2.1.319 output (ConPTY, Windows 11).
    const TRUST: &str = "\x1b[1m\x1b[3;2HAccessing\x1b[1Cworkspace:\x1b[m\x1b[7;2HQuick\x1b[1Csafety\x1b[1Ccheck:\x1b[15;2H\u{276f}\x1b[1CNo,\x1b[1Cexit\x1b[m\x1b[16;4HYes,\x1b[1CI\x1b[1Ctrust\x1b[1Cthis\x1b[1Cfolder\x1b[18;2HEnter\x1b[1Cto\x1b[1Cconfirm\x1b[1C\u{b7}\x1b[1CEsc\x1b[1Cto\x1b[1Ccancel\x1b[m";
    const MCP: &str = "\x1b[3;3HNew\x1b[1CMCP\x1b[1Cserver\x1b[1Cfound\x1b[10;3H>\x1b[1CContinue\x1b[1Cwithout\x1b[12;3HEnter\x1b[1Cto\x1b[1Cconfirm";
    const IDLE: &str = "\x1b[2;1H\u{2500}\u{2500}\x1b[m>\u{a0}\x1b[2mTry \"how does <filepath> work?\"\x1b[22m\x1b[K\r\n  \u{23f5}\u{23f5} auto mode on (shift+tab to cycle)";

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
    fn real_dialogs_stay_open_until_the_empty_input_box_follows() {
        assert!(Screen::of(TRUST).dialog_open());
        assert!(!Screen::of(TRUST).idle());
        assert!(Screen::of(&format!("{TRUST}\r\n{MCP}")).dialog_open());
        let ready = Screen::of(&format!("{TRUST}{MCP}{IDLE}"));
        assert!(!ready.dialog_open());
        assert!(ready.idle());
        assert!(Screen::of(&format!("{IDLE}{MCP}")).dialog_open());
        assert!(!Screen::of("").idle() && !Screen::of("").dialog_open());
        let busy = format!("{IDLE} \u{273b} Thinking\u{2026} (esc to interrupt)");
        assert!(!Screen::of(&busy).dialog_open());
        let fake = "Do you trust the files in this folder? Enter to confirm \u{b7} Esc to exit";
        assert!(Screen::of(fake).dialog_open());
        assert!(Screen::of(&format!("{fake}\r\n  ? for shortcuts")).idle());
    }

    #[test]
    fn input_shows_text_typed_after_the_empty_box() {
        let typed = format!(
            "{IDLE}\x1b[20;3H\u{276f} \x1b[38;2;255;255;255mReply\x1b[1Cwith the single word ok and create\r\n  no files"
        );
        let s = Screen::of(&typed);
        assert!(s.input_shows("Reply with   the single\nword ok and create no files at all"));
        assert!(!s.input_shows("Something else entirely"));
        assert!(!s.input_shows("  "));
        let cleared = Screen::of(&format!("{typed}{IDLE}"));
        assert!(!cleared.input_shows("Reply with the single word ok"));
        let model = Screen::of_all("/model opus\r\nSet model to opus");
        assert!(model.contains("set model") && model.text().starts_with("/model"));
    }

    #[test]
    fn screen_tail_keeps_the_last_lines() {
        let raw = format!("{TRUST}{}{IDLE}", "x\r\n".repeat(SCREEN_LINES));
        let tail = screen_tail(&raw);
        assert!(!tail.contains("Accessing"));
        assert!(tail.ends_with("(shift+tab to cycle)"));
        assert!(Screen::of(&raw).idle());
        let big = "\u{2500}".repeat(SCREEN_BYTES);
        assert!(screen_tail(&big).len() <= SCREEN_BYTES);
        assert_eq!(normalize("A\x1b[1Cb\r\n \u{a0}C\t"), "a b \u{a0}c");
    }

    #[test]
    fn other_markers_judge_the_same_text() {
        let codex = Arc::new(Markers::of(
            &["Ask Codex to do anything"],
            &["Trust this folder", "Folder access"],
        ));
        let dialog = "Folder access\r\nTrust this folder?\r\n\u{203a} 1. Trust and continue";
        assert!(Screen::of(dialog).with(codex.clone()).dialog_open());
        let ready = format!("{dialog}\r\n\u{203a} Ask Codex to do anything");
        let s = Screen::of(&ready).with(codex.clone());
        assert!(s.idle() && !s.dialog_open());
        assert!(
            !Screen::of(&ready).idle(),
            "Claude markers do not see Codex's box"
        );
        assert_eq!(Markers::of(&["  ", "A  B"], &[]).idle, ["a b"]);
    }

    #[test]
    fn plain_tail_drops_escapes() {
        assert_eq!(plain_tail("\x1b[1mab\x1b[1Ccd  \r\n ef", 4), "d ef");
        assert_eq!(plain_tail("x", 10), "x");
    }
}
