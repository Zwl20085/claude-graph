//! The last pass of [`super::relink`]: what odd spellings left (UNC
//! `\\localhost\C$\Users\…`, `C:Users\…`, HTML entities, look-alike or
//! percent-encoded separators, zero-width characters, markup or line
//! breaks inside a path) is hidden by context, never by the bare name:
//!
//! - `Users` (a whole word), then a separator run, then the home folder's
//!   name (its last component, long or 8.3), ending at a name boundary:
//!   the name becomes [`ELSEWHERE`]. A separator run is up to 64 chars of
//!   separator-like characters (`/ \ ／ ＼ ∕ ⁄ ∖ ⧵`, the entities `&#92;`
//!   `&#47;` `&#x5c;` `&#x2f;` `&bsol;` `&sol;`, `%5C` `%2F`) mixed with
//!   ignorable ones (zero-width characters, `* _ ` `, whitespace and line
//!   breaks), with at least one separator-like character. Inside the name,
//!   zero-width characters, `*`, `` ` `` and line breaks are skipped.
//! - `AppData`, a separator run, `Roaming`, a separator run, `claude-graph`
//!   (whole words): the whole span becomes [`ELSEWHERE`].
//!
//! The owner's name in prose (no `Users` and separator before it) stays.

use std::path::PathBuf;

use super::relink::{components, ELSEWHERE};

const SEP_LIKE: &[char] = &['/', '\\', '／', '＼', '∕', '⁄', '∖', '⧵'];
const ENTITIES: &[&str] = &[
    "&#92;", "&#47;", "&#x5c;", "&#x2f;", "&bsol;", "&sol;", "%5c", "%2f",
];
const RUN_MAX: usize = 64;

fn zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{00ad}'
    ) || super::relink::sentinel_index(c).is_some()
}

/// Skipped between letters of a name.
fn in_name_ignorable(c: char) -> bool {
    zero_width(c) || matches!(c, '*' | '`' | '\r' | '\n')
}

/// Allowed in a separator run besides separators.
fn run_ignorable(c: char) -> bool {
    zero_width(c) || c.is_whitespace() || matches!(c, '*' | '_' | '`')
}

/// The end of a separator run at byte `i` (`None`: no separator in it).
fn sep_run(text: &str, i: usize) -> Option<usize> {
    let mut j = i;
    let mut seen = false;
    let mut count = 0;
    while j < text.len() && count < RUN_MAX {
        let rest = &text[j..];
        if let Some(e) = ENTITIES.iter().find(|e| {
            rest.get(..e.len())
                .is_some_and(|r| r.eq_ignore_ascii_case(e))
        }) {
            j += e.len();
            seen = true;
        } else {
            let Some(c) = rest.chars().next() else { break };
            if SEP_LIKE.contains(&c) {
                seen = true;
            } else if !run_ignorable(c) {
                break;
            }
            j += c.len_utf8();
        }
        count += 1;
    }
    seen.then_some(j)
}

/// A word ends at `i` (no letter or digit, and no `-`/`_`/`.` + letter).
fn boundary(text: &str, i: usize) -> bool {
    let mut it = text[i..].chars();
    match it.next() {
        Some(c) if c.is_alphanumeric() => false,
        Some('-' | '_' | '.') => !it.next().is_some_and(char::is_alphanumeric),
        _ => true,
    }
}

/// `word` (lowercase) spelled at byte `i`, ignorable chars skipped inside,
/// ending at a word boundary.
fn word_at(text: &str, i: usize, word: &str, skip: fn(char) -> bool) -> Option<usize> {
    spelled(text, i, word, skip).filter(|&j| boundary(text, j))
}

/// `word` (lowercase) spelled at byte `i`, ignorable chars skipped inside.
fn spelled(text: &str, i: usize, word: &str, skip: fn(char) -> bool) -> Option<usize> {
    let mut j = i;
    let mut want = word.chars().peekable();
    let mut first = true;
    while let Some(&w) = want.peek() {
        let c = text[j..].chars().next()?;
        if !first && skip(c) {
            j += c.len_utf8();
            continue;
        }
        if !c.to_lowercase().eq(w.to_lowercase()) {
            return None;
        }
        first = false;
        j += c.len_utf8();
        want.next();
    }
    Some(j)
}

fn starts_word(text: &str, i: usize) -> bool {
    !text[..i]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
}

/// The home folder names to hide (long and 8.3), longest first.
fn names(home: &[PathBuf]) -> Vec<String> {
    let mut v: Vec<String> = home
        .iter()
        .filter_map(|h| components(&h.display().to_string()).last().cloned())
        .filter(|n| !n.starts_with('%') && n != "~" && !n.ends_with(':'))
        .collect();
    v.sort_by_key(|n| std::cmp::Reverse(n.len()));
    v.dedup();
    v
}

/// Hide what the rewrite left (see the module doc).
pub(super) fn backstop(text: &str, home: &[PathBuf]) -> String {
    let names = names(home);
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    let mut i = 0;
    while i < text.len() {
        let rest = &lower[i..];
        let hit = if rest.starts_with("users") && starts_word(text, i) {
            users_hit(text, i, &names)
        } else if rest.starts_with("appdata") && starts_word(text, i) {
            appdata_hit(text, i)
        } else {
            None
        };
        if let Some((from, to)) = hit {
            out.push_str(&text[pos..from]);
            out.push_str(ELSEWHERE);
            pos = to;
            i = to;
            continue;
        }
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    out.push_str(&text[pos..]);
    out
}

/// `Users` + separators + a home name at `i`: the name's span. A domain
/// profile folder (`<name>.<DOMAIN>` followed by a separator) counts too;
/// `<name>.txt` at the end of a path does not.
fn users_hit(text: &str, i: usize, names: &[String]) -> Option<(usize, usize)> {
    let start = sep_run(text, i + "users".len())?;
    names.iter().find_map(|n| {
        let end = spelled(text, start, n, in_name_ignorable)?;
        (boundary(text, end) || domain_folder(text, end)).then_some((start, end))
    })
}

/// `.<DOMAIN>` (letters, digits, `-`) then a separator at byte `i`.
fn domain_folder(text: &str, i: usize) -> bool {
    let Some(rest) = text[i..].strip_prefix('.') else {
        return false;
    };
    let n = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
        .count();
    n > 0 && rest[n..].starts_with(SEP_LIKE)
}

/// `AppData` + separators + `Roaming` + separators + `claude-graph` at `i`.
fn appdata_hit(text: &str, i: usize) -> Option<(usize, usize)> {
    let a = sep_run(text, i + "appdata".len())?;
    let r = word_at(text, a, "roaming", in_name_ignorable)?;
    let b = sep_run(text, r)?;
    let end = word_at(text, b, "claude-graph", in_name_ignorable)?;
    Some((i, end))
}
