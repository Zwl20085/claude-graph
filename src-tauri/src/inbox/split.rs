//! Split a tip file into frontmatter and body and join it back, byte for
//! byte. Tolerated: a leading BOM, CRLF or LF line ends. A fence is exactly
//! `---` and its line end (no trailing spaces or tabs, protocol section 9);
//! the closing fence may also end the file. The body is everything after the
//! closing fence line (docs/AGENT-PROTOCOL.md section 4).

/// The parts of a tip file; `head + frontmatter + fence + body` is the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts<'a> {
    /// Optional BOM plus the opening `---` line with its line end.
    pub head: &'a str,
    /// The YAML text between the fences (each line with its line end).
    pub frontmatter: &'a str,
    /// The closing `---` line with its line end (none at end of file).
    pub fence: &'a str,
    pub body: &'a str,
}

impl Parts<'_> {
    /// The file with the frontmatter and body replaced.
    pub fn join(&self, frontmatter: &str, body: &str) -> String {
        [self.head, frontmatter, self.fence, body].concat()
    }

    /// `\r\n` when the opening fence ends that way, else `\n`.
    pub fn newline(&self) -> &'static str {
        if self.head.ends_with("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }
}

fn is_fence(line: &str) -> bool {
    // Exactly `---` and its line end, as the board's core/frontmatter.rs reads
    // it (protocol section 9); a closing fence may also end the file.
    matches!(line, "---\n" | "---\r\n" | "---")
}

/// Lines with their line ends (the last one may have none).
fn lines_with_ends(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
}

/// `None` when the file has no frontmatter (no opening fence on the first
/// line, or no closing fence).
pub fn split(text: &str) -> Option<Parts<'_>> {
    let start = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let rest = &text[start..];
    let first = lines_with_ends(rest).next()?;
    if !is_fence(first) || !first.ends_with('\n') {
        return None;
    }
    let head_len = start + first.len();
    let mut offset = head_len;
    for line in lines_with_ends(&text[head_len..]) {
        if is_fence(line) {
            return Some(Parts {
                head: &text[..head_len],
                frontmatter: &text[head_len..offset],
                fence: &text[offset..offset + line.len()],
                body: &text[offset + line.len()..],
            });
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lf_crlf_and_bom_and_joins_back() {
        for text in [
            "---\ntitle: a\nkind: agent\n---\nbody\n",
            "\u{feff}---\r\ntitle: a\r\n---\r\n\r\nbody\r\n",
            "---\n---\n",
            "---\ntitle: x\n---",
        ] {
            let p = split(text).unwrap_or_else(|| panic!("{text:?}"));
            assert_eq!(p.join(p.frontmatter, p.body), text);
        }
        let p = split("\u{feff}---\r\ntitle: a\r\n---\r\nbody").unwrap();
        assert_eq!(
            (p.frontmatter, p.body, p.newline()),
            ("title: a\r\n", "body", "\r\n")
        );
        let p = split("---\ntitle: x\n---").unwrap();
        assert_eq!((p.fence, p.body), ("---", ""));
    }

    #[test]
    fn no_frontmatter_is_none() {
        assert!(split("title: a\n").is_none());
        assert!(split("---\ntitle: a\n").is_none());
        assert!(split("--- \n").is_none());
        assert!(split("").is_none());
        assert!(split("----\nx\n---\n").is_none());
    }

    #[test]
    fn fences_with_trailing_blanks_or_lone_cr_are_not_frontmatter() {
        for text in [
            "--- \ntitle: a\n---\nbody\n",
            "---\ntitle: a\n---  \r\nbody\n",
            "---\t\ntitle: a\n---\nbody\n",
            "---\ntitle: a\n---\t\n",
            "---\rtitle: a\r---\rbody\r",
            " ---\ntitle: a\n---\n",
        ] {
            assert!(split(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn body_keeps_later_fences() {
        let p = split("---\na: 1\n---\nx\n---\ny\n").unwrap();
        assert_eq!(p.body, "x\n---\ny\n");
    }
}
