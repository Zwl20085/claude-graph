//! Low-level matching for [`super::relink`]: separators, names, path
//! starts, and a per-text index (next delimiter, `claude-graph` layout
//! markers) so that one pass over a text stays linear. Every scan from a
//! path start is bounded by [`MAX_PATH`] bytes.

/// Most bytes one path is followed for (separator runs, names, spans).
pub(super) const MAX_PATH: usize = 4096;

/// Characters that end a path that is not matched against a known name.
pub(super) fn is_delim(c: char) -> bool {
    matches!(
        c,
        '`' | '\''
            | '"'
            | '<'
            | '>'
            | '|'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '*'
            | ','
            | '#'
            | '，'
            | '。'
            | '；'
            | '：'
            | '！'
            | '？'
            | '、'
            | '…'
            | '（'
            | '）'
            | '「'
            | '」'
            | '『'
            | '』'
            | '【'
            | '】'
    ) || c.is_whitespace()
}

/// Trailing characters kept outside a path (sentence punctuation, emphasis).
pub(super) fn is_trailing(c: char) -> bool {
    matches!(
        c,
        '.' | ';'
            | ':'
            | '!'
            | '?'
            | '_'
            | '*'
            | '~'
            | '。'
            | '，'
            | '；'
            | '：'
            | '！'
            | '？'
            | '、'
            | '…'
            | '）'
    )
}

/// The largest char boundary of `text` at or below `i`.
pub(super) fn floor(text: &str, i: usize) -> usize {
    let mut i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A separator at byte `i`: `/`, `\`, or `%2F` / `%5C`; its length.
pub(super) fn sep_at(text: &str, i: usize) -> usize {
    let b = text.as_bytes();
    match b.get(i) {
        Some(b'/' | b'\\') => 1,
        Some(b'%') if b.len() >= i + 3 => {
            let hex = &b[i + 1..i + 3];
            if hex.eq_ignore_ascii_case(b"2f") || hex.eq_ignore_ascii_case(b"5c") {
                3
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// Bytes from `i` that are separators and `./` components (bounded).
pub(super) fn seps(text: &str, i: usize) -> usize {
    let limit = (i + MAX_PATH).min(text.len());
    let mut j = i;
    while j < limit {
        let k = skip_sentinels(text, j);
        if k > j && sep_at(text, k) > 0 {
            j = k;
        }
        let n = sep_at(text, j);
        if n > 0 {
            j += n;
        } else if text.as_bytes()[j] == b'.' && sep_at(text, j + 1) > 0 {
            j += 1;
        } else {
            break;
        }
    }
    j - i
}

/// One char at byte `i` (a `%XX` ASCII escape decoded) and its length.
pub(super) fn char_at(text: &str, i: usize) -> Option<(char, usize)> {
    let b = text.as_bytes();
    if b.get(i) == Some(&b'%') && b.len() >= i + 3 {
        if let Ok(v) = u8::from_str_radix(text.get(i + 1..i + 3)?, 16) {
            if v.is_ascii() && v != 0 {
                return Some((v as char, 3));
            }
        }
    }
    text.get(i..)?.chars().next().map(|c| (c, c.len_utf8()))
}

/// Byte `i` moved past sentinels (invalid bytes of the original text).
pub(super) fn skip_sentinels(text: &str, mut i: usize) -> usize {
    while let Some(c) = text.get(i..).and_then(|t| t.chars().next()) {
        if super::relink::sentinel_index(c).is_none() {
            break;
        }
        i += c.len_utf8();
    }
    i
}

/// What follows byte `i` would continue a name.
pub(super) fn continues(text: &str, i: usize) -> bool {
    let mut it = text.get(i..).unwrap_or("").chars();
    match it.next() {
        Some(c) if c.is_alphanumeric() => true,
        Some('-' | '_' | '.' | '~' | '\'') => it.next().is_some_and(char::is_alphanumeric),
        _ => false,
    }
}

/// The end of `want` (lowercase chars; `/` = a separator run) spelled at
/// byte `i`, ending at a whole-name boundary.
pub(super) fn spells(text: &str, i: usize, want: &[char]) -> Option<usize> {
    let mut j = i;
    for &w in want {
        j = skip_sentinels(text, j);
        if w == '/' {
            let n = seps(text, j);
            if n == 0 {
                return None;
            }
            j += n;
            continue;
        }
        let (c, len) = char_at(text, j)?;
        let same = if c.is_ascii() && w.is_ascii() {
            c.eq_ignore_ascii_case(&w)
        } else {
            c.to_lowercase().eq(w.to_lowercase())
        };
        if !same {
            return None;
        }
        j += len;
    }
    (!continues(text, j)).then_some(j)
}

/// The start of an absolute path at byte `i`.
pub(super) struct Head {
    /// A drive (`c:`) or `~`.
    fixed: Option<&'static str>,
    /// Otherwise a variable's name (byte range in the text).
    var: (usize, usize),
    /// Byte after it.
    pub end: usize,
}

impl Head {
    /// A variable (`%NAME%`, `$NAME`), not a drive or `~`.
    pub(super) fn is_var(&self) -> bool {
        self.fixed.is_none()
    }

    /// The head is the folder spelling's first component `h` (`c:`, `~`,
    /// `%name%`; `HOME` counts as `%userprofile%`).
    pub(super) fn is(&self, text: &str, h: &str) -> bool {
        if let Some(f) = self.fixed {
            return f == h;
        }
        let raw = &text[self.var.0..self.var.1];
        match h.strip_prefix('%').and_then(|x| x.strip_suffix('%')) {
            Some(inner) => {
                inner.eq_ignore_ascii_case(raw)
                    || (inner == "userprofile" && raw.eq_ignore_ascii_case("home"))
            }
            None => false,
        }
    }
}

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

const DRIVES: [&str; 26] = [
    "a:", "b:", "c:", "d:", "e:", "f:", "g:", "h:", "i:", "j:", "k:", "l:", "m:", "n:", "o:", "p:",
    "q:", "r:", "s:", "t:", "u:", "v:", "w:", "x:", "y:", "z:",
];

fn drive(letter: u8, end: usize) -> Head {
    Head {
        fixed: Some(DRIVES[usize::from(letter.to_ascii_lowercase() - b'a')]),
        var: (0, 0),
        end,
    }
}

/// A drive at `j`: a letter and `:` (or `%3A`) followed by a separator.
fn drive_at(text: &str, j: usize) -> Option<Head> {
    let b = text.as_bytes();
    let letter = *b.get(j)?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let colon = match b.get(j + 1) {
        Some(b':') => 1,
        Some(b'%')
            if b.get(j + 2..j + 4)
                .is_some_and(|h| h.eq_ignore_ascii_case(b"3a")) =>
        {
            3
        }
        _ => return None,
    };
    let end = j + 1 + colon;
    (sep_at(text, end) > 0).then(|| drive(letter, end))
}

/// `/c/…` (Git Bash) or `/mnt/c/…` (WSL) at `j`.
fn bash_drive_at(text: &str, j: usize) -> Option<Head> {
    let b = text.as_bytes();
    let at = if text
        .get(j..j + 5)
        .is_some_and(|s| s.eq_ignore_ascii_case("/mnt/"))
    {
        j + 5
    } else if b.get(j) == Some(&b'/') {
        j + 1
    } else {
        return None;
    };
    let letter = *b.get(at)?;
    (letter.is_ascii_alphabetic() && sep_at(text, at + 1) > 0).then(|| drive(letter, at + 1))
}

/// An environment variable name at `j` (ASCII letters, digits, `_`).
fn var_name(text: &str, j: usize) -> usize {
    text.as_bytes()
        .get(j..)
        .unwrap_or(&[])
        .iter()
        .take(64)
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
        .count()
}

/// `%NAME%`, `$env:NAME`, `$NAME` or `${NAME}` at `j`, as `%name%` (`HOME`
/// is `%userprofile%`).
fn var_at(text: &str, j: usize) -> Option<Head> {
    let b = text.as_bytes();
    let (start, close) = match b.get(j)? {
        b'%' => (j + 1, Some(b'%')),
        b'$' if text
            .get(j..j + 5)
            .is_some_and(|s| s.eq_ignore_ascii_case("$env:")) =>
        {
            (j + 5, None)
        }
        b'$' if b.get(j + 1) == Some(&b'{') => (j + 2, Some(b'}')),
        b'$' => (j + 1, None),
        _ => return None,
    };
    let n = var_name(text, start);
    if n == 0 || !b[start].is_ascii_alphabetic() && b[start] != b'_' {
        return None;
    }
    let mut end = start + n;
    if let Some(c) = close {
        if b.get(end) != Some(&c) {
            return None;
        }
        end += 1;
    }
    Some(Head {
        fixed: None,
        var: (start, start + n),
        end,
    })
}

/// A path start at byte `i` (with `prev`, the char before it): an optional
/// `file://` (`localhost/`) and `\\?\` / `\\.\` prefix, then a drive (also
/// `/c/`, `/mnt/c/`, `/C:/`), `~/`, or a variable. A drive or variable
/// never follows a letter, digit or `:`; `~` and `/c/` never follow a word
/// character or `/` (so URLs are not paths).
pub(super) fn head_at(text: &str, i: usize, prev: Option<char>) -> Option<Head> {
    // `Path:C:\…`: a drive may follow a `:` that ends a word.
    let label = prev == Some(':')
        && text[..i]
            .chars()
            .rev()
            .nth(1)
            .is_some_and(|c| c.is_ascii_alphabetic());
    let after_word = !label && prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == ':');
    let after_path = prev.is_some_and(|c| word(c) || c == '/' || c == ':');
    let mut j = i;
    if text
        .get(j..j + 5)
        .is_some_and(|s| s.eq_ignore_ascii_case("file:"))
    {
        let slashes = text.as_bytes()[j + 5..]
            .iter()
            .take(3)
            .take_while(|c| **c == b'/')
            .count();
        if slashes >= 2 {
            j += 5 + slashes;
            if text
                .get(j..j + 10)
                .is_some_and(|s| s.eq_ignore_ascii_case("localhost/"))
            {
                j += 10;
            }
        }
    }
    if let Some(p) = text.get(j..j + 4) {
        if matches!(p, r"\\?\" | r"\\.\" | "//?/" | "//./") {
            j += 4;
        }
    }
    // `/C:/…` (a file URI's path): only where a path may start.
    if (j > i || !after_path)
        && text.as_bytes().get(j) == Some(&b'/')
        && drive_at(text, j + 1).is_some()
    {
        j += 1;
    }
    let prefixed = j > i;
    if let Some(h) = drive_at(text, j) {
        return (prefixed || !after_word).then_some(h);
    }
    if after_path && !prefixed {
        return None;
    }
    if let Some(h) = bash_drive_at(text, j) {
        return Some(h);
    }
    if text.as_bytes().get(j) == Some(&b'~') && sep_at(text, j + 1) > 0 {
        return Some(Head {
            fixed: Some("~"),
            var: (0, 0),
            end: j + 1,
        });
    }
    if after_word {
        return None;
    }
    var_at(text, j)
}

/// Per-text index: the next delimiter after each byte and the positions of
/// `claude-graph` data layout markers (`appdata/roaming/claude-graph`,
/// `claude-graph/inbox`, separators of any kind, `./` allowed).
pub(super) struct Index {
    next_delim: Vec<u32>,
    markers: Vec<usize>,
}

fn find_all(lower: &str, needle: &str) -> Vec<usize> {
    lower.match_indices(needle).map(|(i, _)| i).collect()
}

impl Index {
    pub(super) fn new(text: &str) -> Self {
        let n = text.len();
        let cap = u32::try_from(n).unwrap_or(u32::MAX);
        let mut next_delim = vec![cap; n + 1];
        let mut next = cap;
        for (i, c) in text.char_indices().rev() {
            if is_delim(c) {
                next = u32::try_from(i).unwrap_or(u32::MAX);
            }
            for slot in &mut next_delim[i..i + c.len_utf8()] {
                *slot = next;
            }
        }
        let lower = text.to_ascii_lowercase();
        let graph: Vec<char> = "claude-graph".chars().collect();
        let mut markers: Vec<usize> = find_all(&lower, "claude-graph")
            .into_iter()
            .filter(|&p| {
                let after = p + graph.len();
                let s = seps(text, after);
                s > 0 && spells(text, after + s, &['i', 'n', 'b', 'o', 'x']).is_some()
            })
            .collect();
        let tail: Vec<char> = "roaming/claude-graph".chars().collect();
        markers.extend(find_all(&lower, "appdata").into_iter().filter(|&p| {
            let after = p + "appdata".len();
            let s = seps(text, after);
            s > 0 && spells(text, after + s, &tail).is_some()
        }));
        markers.sort_unstable();
        Self {
            next_delim,
            markers,
        }
    }

    /// The end of the plain path span from `i`: the next delimiter, at most
    /// [`MAX_PATH`] bytes on.
    pub(super) fn span_end(&self, text: &str, i: usize) -> usize {
        let d = self.next_delim.get(i).map_or(text.len(), |&d| d as usize);
        floor(text, d.min(i + MAX_PATH))
    }

    /// A layout marker starts in `from..to`.
    pub(super) fn layout_in(&self, from: usize, to: usize) -> bool {
        let k = self.markers.partition_point(|&m| m < from);
        self.markers.get(k).is_some_and(|&m| m < to)
    }
}
