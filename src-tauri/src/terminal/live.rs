//! The live screen of a session as a grid of rows ([`vt100`]), fed with
//! every output chunk, for the checks that need lines: is the typed text
//! still in the input area (the composer) of a CLI that draws no hook?

use std::num::NonZeroU16;

/// The terminal screen as the window shows it now (no scrollback).
pub struct LiveGrid {
    parser: vt100::Parser,
}

fn size(rows: u16, cols: u16) -> (NonZeroU16, NonZeroU16) {
    (
        NonZeroU16::new(rows).unwrap_or(vt100::Parser::DEFAULT_ROWS),
        NonZeroU16::new(cols).unwrap_or(vt100::Parser::DEFAULT_COLS),
    )
}

impl LiveGrid {
    pub fn new(rows: u16, cols: u16) -> Self {
        let (r, c) = size(rows, cols);
        Self {
            parser: vt100::Parser::new(r, c, 0),
        }
    }

    pub fn feed(&mut self, text: &str) {
        self.parser.process(text.as_bytes());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (r, c) = size(rows, cols);
        self.parser.set_size(r, c);
    }

    /// Every screen row, trailing blanks trimmed.
    pub fn rows(&self) -> Vec<String> {
        let screen = self.parser.screen();
        let cols = screen.size().1.get();
        screen
            .rows(0, cols)
            .map(|r| r.trim_end().to_string())
            .collect()
    }
}

impl Default for LiveGrid {
    fn default() -> Self {
        Self::new(crate::terminal::DEFAULT_ROWS, crate::terminal::DEFAULT_COLS)
    }
}

/// How to find a CLI's input area on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    /// The prompt glyph that starts the input line (Codex: `›`): the area is
    /// the last row starting with it (after indentation) and every row below.
    pub glyph: Option<char>,
    /// Without a glyph (or none on screen): the last this many non-blank rows.
    pub rows: usize,
    /// Texts of the empty input area (lowercase, normalized).
    pub placeholders: Vec<String>,
    /// Texts that show the CLI working on a prompt (lowercase, normalized).
    pub busy: Vec<String>,
}

/// The input area of `rows` per `c`, as text (rows joined by newlines).
pub fn composer_text(rows: &[String], c: &Composer) -> String {
    let filled: Vec<&String> = rows.iter().filter(|r| !r.trim().is_empty()).collect();
    let from_glyph = c
        .glyph
        .and_then(|g| filled.iter().rposition(|r| r.trim_start().starts_with(g)));
    let start = from_glyph.unwrap_or_else(|| filled.len().saturating_sub(c.rows.max(1)));
    filled[start..]
        .iter()
        .map(|r| r.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex() -> Composer {
        Composer {
            glyph: Some('\u{203a}'),
            rows: 3,
            placeholders: vec!["ask codex to do anything".into()],
            busy: vec!["working".into()],
        }
    }

    #[test]
    fn grid_rows_follow_cursor_moves_and_erases() {
        let mut g = LiveGrid::new(5, 20);
        g.feed("one\r\ntwo\r\n\u{203a} typed");
        g.feed("\r\x1b[2K\u{203a} Ask");
        let rows = g.rows();
        assert_eq!(&rows[..3], ["one", "two", "\u{203a} Ask"]);
        g.resize(3, 10);
        assert_eq!(g.rows().len(), 3);
    }

    /// Rows as seen with the real Codex 0.167 just after Enter.
    #[test]
    fn composer_is_from_the_last_glyph_row() {
        let rows: Vec<String> = [
            "  >_ OpenAI Codex (v0.167.0)",
            "",
            "\u{203a} Reply with the single word ok and create no files",
            "",
            "\u{2022} Working (0s \u{2022} esc to interrupt)",
            "",
            "\u{203a} Ask Codex to do anything",
            "",
            "  GPT model \u{b7} C:\\tmp",
        ]
        .map(String::from)
        .to_vec();
        let text = composer_text(&rows, &codex());
        assert!(text.starts_with("\u{203a} Ask Codex"), "{text}");
        assert!(text.ends_with("C:\\tmp"));
        let plain = Composer {
            glyph: None,
            ..codex()
        };
        let last3 = composer_text(&rows, &plain);
        assert!(last3.starts_with("\u{2022} Working"), "{last3}");
        assert_eq!(composer_text(&[], &plain), "");
    }
}
