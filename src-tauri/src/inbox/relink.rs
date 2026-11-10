//! Local paths in text the runner writes into a tip and into `result.md`.
//!
//! The agent works in a round folder of the runner's machine; a path to it
//! means nothing on the owner's other devices and shows the Windows user
//! name. [`relink`] (a pure text function, linear in the text: every scan
//! from a path start is bounded) finds absolute paths and rewrites those
//! under a known folder, the most specific known folder first:
//!
//! | The path is under | It becomes |
//! |---|---|
//! | this round's deliverables folder, a copied file | `agent/<tip id>/<name of the copy>` |
//! | … a folder of copied files (the whole path) | `agent/<tip id>/<folder>` |
//! | … the folder itself | `agent/<tip id>/` |
//! | … a file the copy refused | `<file> (not copied)` |
//! | … any other name | `<name>` (no claim) |
//! | … a `..` way out of it | [`ELSEWHERE`] |
//! | the round folder, the app data folder, or any `claude-graph` data layout | [`ELSEWHERE`] |
//! | the tips folder | the path relative to it (the folder itself: [`TIPS_FOLDER`]) |
//! | the home folder | `~/<rest>` |
//! | anything else | unchanged |
//!
//! A path starts with a drive (`C:\`, `c%3A/`, Git Bash `/c/`, WSL
//! `/mnt/c/`, `/C:/`), `~/`, or a variable (`%NAME%`, `$env:NAME`, `$HOME`),
//! after an optional `file://` (`localhost/`) or `\\?\` / `\\.\` prefix; a
//! drive never follows a letter, digit or `:`, and `~/`, `/c/` never follow
//! a word character or `/` (URLs are not paths). Folders are compared per
//! component, case-insensitively, each component in its long or 8.3 form,
//! with any separators (`/`, `\`, doubled, `%2F`, `%5C`) and `./` between;
//! a component must end at a name boundary. File names after the
//! deliverables folder are matched against the known file list. Afterwards
//! a backstop ([`super::relink_backstop`]) hides what odd spellings left.

use std::path::PathBuf;

use super::relink_scan::{floor, head_at, is_trailing, seps, spells, Index};

/// What a local path that is not a deliverable becomes.
pub const ELSEWHERE: &str = "(a folder on the runner's machine)";
/// What the tips folder itself becomes.
pub const TIPS_FOLDER: &str = "(the tips folder)";

/// First of the private-use code points that stand for invalid byte
/// sequences of a text while it is rewritten (see [`sentinel`]).
const SENTINEL_BASE: u32 = 0xF0000;
/// How many sentinels there are (Supplementary Private Use Area-A).
pub const SENTINEL_COUNT: usize = 0xFFFE;

/// The sentinel for the `i`-th invalid byte sequence of a text (`i` below
/// [`SENTINEL_COUNT`]). The matchers skip sentinels inside names and
/// separators, so an invalid byte cannot hide a path; the caller puts the
/// original bytes back for every sentinel still in the output.
pub fn sentinel(i: usize) -> char {
    let i = u32::try_from(i.min(SENTINEL_COUNT - 1)).unwrap_or(0);
    char::from_u32(SENTINEL_BASE + i).unwrap_or('\u{F0000}')
}

/// The index of a sentinel char.
pub fn sentinel_index(c: char) -> Option<usize> {
    let v = u32::from(c);
    let count = u32::try_from(SENTINEL_COUNT).unwrap_or(0);
    (SENTINEL_BASE..SENTINEL_BASE + count)
        .contains(&v)
        .then(|| (v - SENTINEL_BASE) as usize)
}

/// One deliverable of the round: its path under the deliverables folder and
/// what it is called in the results folder (`None`: refused by the copy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedFile {
    pub source: String,
    pub target: Option<String>,
}

impl CopiedFile {
    pub fn copied(source: &str, target: &str) -> Self {
        Self {
            source: source.to_string(),
            target: Some(target.to_string()),
        }
    }

    pub fn refused(source: &str) -> Self {
        Self {
            source: source.to_string(),
            target: None,
        }
    }
}

/// Where the round's files are, and what became of them. Each folder may be
/// given in several spellings (as configured, resolved, 8.3, `%APPDATA%`);
/// `~` spellings of folders under `home` are added by [`relink`].
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub tip_id: String,
    /// This round's deliverables folder.
    pub deliverables: Vec<PathBuf>,
    /// Folders that must not show: this round's folder, the app data folder.
    pub private: Vec<PathBuf>,
    /// The tips folder.
    pub tips: Vec<PathBuf>,
    /// The user's home folder.
    pub home: Vec<PathBuf>,
    pub files: Vec<CopiedFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Deliverables,
    Private,
    Tips,
    Home,
}

/// A known folder: per component, its spellings (lowercase chars); the
/// first component's spellings also as strings (compared with a path head).
struct Root {
    heads: Vec<String>,
    parts: Vec<Vec<Vec<char>>>,
    kind: Kind,
}

/// The components of a folder spelling, normalized: the first is `c:`,
/// `~` or `%name%`; verbatim prefixes and `.` dropped; lowercase.
pub(super) fn components(path: &str) -> Vec<String> {
    let mut parts: Vec<String> = path
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != "." && *p != "?")
        .map(str::to_lowercase)
        .collect();
    if let Some(first) = parts.first_mut() {
        if first == "%home%" {
            *first = "%userprofile%".into();
        }
    }
    parts
}

/// The spellings of a component line up: equal, or one is an 8.3 name.
fn compatible(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x == y || short_of(x, y) || short_of(y, x))
}

/// `short` is an 8.3 form of `long` (Windows: up to six leading letters or
/// digits of the long name, or two and a hash, then `~N`).
fn short_of(short: &str, long: &str) -> bool {
    let Some((base, rest)) = short.split_once('~') else {
        return false;
    };
    let digits = rest.split('.').next().unwrap_or("");
    if base.is_empty() || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let plain: String = long.chars().filter(|c| c.is_alphanumeric()).collect();
    plain.starts_with(base)
        || (base.len() >= 2
            && base.is_char_boundary(2)
            && plain.starts_with(&base[..2])
            && base[2..].bytes().all(|b| b.is_ascii_hexdigit()))
}

fn roots(p: &Places) -> Vec<Root> {
    let homes: Vec<Vec<String>> = p
        .home
        .iter()
        .map(|h| components(&h.display().to_string()))
        .filter(|h| !h.is_empty())
        .collect();
    let mut spelled: Vec<(Vec<String>, Kind)> = Vec::new();
    let groups = [
        (&p.deliverables, Kind::Deliverables),
        (&p.private, Kind::Private),
        (&p.tips, Kind::Tips),
        (&p.home, Kind::Home),
    ];
    for (paths, kind) in groups {
        for path in paths {
            let parts = components(&path.display().to_string());
            if parts.is_empty() {
                continue;
            }
            if kind != Kind::Home {
                // `~` and `%userprofile%` spellings, by exact comparison with
                // a spelling of the home folder (never by the 8.3 wildcard).
                for h in homes.iter().filter(|h| parts.len() > h.len()) {
                    if h[0].starts_with('%') || parts[..h.len()] != h[..] {
                        continue;
                    }
                    for head in ["~", "%userprofile%"] {
                        let spelling = std::iter::once(head.to_string())
                            .chain(parts[h.len()..].iter().cloned())
                            .collect();
                        spelled.push((spelling, kind));
                    }
                }
            }
            spelled.push((parts, kind));
        }
    }
    let mut merged: Vec<(Vec<Vec<String>>, Kind)> = Vec::new();
    for (parts, kind) in spelled {
        let slot = merged.iter_mut().find(|(alts, k)| {
            *k == kind
                && alts.len() == parts.len()
                && compatible(
                    &alts.iter().map(|a| a[0].clone()).collect::<Vec<_>>(),
                    &parts,
                )
        });
        match slot {
            Some((alts, _)) => {
                for (alt, part) in alts.iter_mut().zip(parts) {
                    if !alt.contains(&part) {
                        alt.push(part);
                    }
                }
            }
            None => merged.push((parts.into_iter().map(|p| vec![p]).collect(), kind)),
        }
    }
    let mut out: Vec<Root> = merged
        .into_iter()
        .map(|(alts, kind)| Root {
            heads: alts[0].clone(),
            parts: alts
                .into_iter()
                .map(|a| a.into_iter().map(|s| s.chars().collect()).collect())
                .collect(),
            kind,
        })
        .collect();
    // Most specific first: more components, then longer spellings.
    out.sort_by_key(|r| {
        let len: usize = r
            .parts
            .iter()
            .map(|a| a.iter().map(Vec::len).max().unwrap_or(0))
            .sum();
        std::cmp::Reverse((r.parts.len(), len))
    });
    out
}

/// A known file: lowercase source chars, source, target.
struct Known {
    lower: Vec<char>,
    source: String,
    target: Option<String>,
}

/// The file list, longest first, and the folders that hold copied files.
struct Files {
    known: Vec<Known>,
    folders: Vec<Vec<char>>,
}

fn files(list: &[CopiedFile]) -> Files {
    let mut known: Vec<Known> = list
        .iter()
        .map(|f| Known {
            lower: f.source.to_lowercase().chars().collect(),
            source: f.source.clone(),
            target: f.target.clone(),
        })
        .collect();
    known.sort_by_key(|k| std::cmp::Reverse(k.lower.len()));
    let mut folders: Vec<Vec<char>> = known
        .iter()
        .filter(|k| k.target.is_some())
        .flat_map(|k| {
            let s: String = k.lower.iter().collect();
            let cut: Vec<usize> = s.match_indices('/').map(|(i, _)| i).collect();
            cut.into_iter()
                .map(move |i| s[..i].chars().collect::<Vec<char>>())
        })
        .collect();
    folders.sort_by_key(|f| std::cmp::Reverse(f.len()));
    folders.dedup();
    Files { known, folders }
}

/// Everything one call needs.
struct Ctx<'a> {
    text: &'a str,
    p: &'a Places,
    roots: Vec<Root>,
    /// Each distinct first component and the roots (in order) it starts.
    by_head: Vec<(String, Vec<usize>)>,
    files: Files,
    idx: Index,
}

fn by_head(roots: &[Root]) -> Vec<(String, Vec<usize>)> {
    let mut v: Vec<(String, Vec<usize>)> = Vec::new();
    for (k, r) in roots.iter().enumerate() {
        for h in &r.heads {
            match v.iter_mut().find(|(x, _)| x == h) {
                Some((_, ks)) if !ks.contains(&k) => ks.push(k),
                Some(_) => {}
                None => v.push((h.clone(), vec![k])),
            }
        }
    }
    v
}

/// A replacement: the byte where it ends and the new text.
type Hit = (usize, String);

impl Ctx<'_> {
    /// The end of `root` spelled from byte `i` (after its head), at a
    /// component boundary.
    fn root_end(&self, root: &Root, mut i: usize) -> Option<usize> {
        for alts in &root.parts[1..] {
            let n = seps(self.text, i);
            if n == 0 {
                return None;
            }
            i += n;
            i = alts.iter().find_map(|a| spells(self.text, i, a))?;
        }
        Some(i)
    }

    /// The span after byte `t` without its trailing punctuation.
    fn kept(&self, t: usize) -> usize {
        let end = self.idx.span_end(self.text, t);
        t + self.text[t..end].trim_end_matches(is_trailing).len()
    }

    fn normalized(&self, from: usize, to: usize) -> String {
        self.text[from..to]
            .split(['/', '\\'])
            .filter(|c| !c.is_empty() && *c != ".")
            .collect::<Vec<_>>()
            .join("/")
    }

    /// After this round's deliverables folder (ending at byte `r`).
    fn after_deliverables(&self, r: usize) -> Hit {
        let base = format!("agent/{}/", self.p.tip_id);
        let n = seps(self.text, r);
        if n == 0 {
            return (r, base);
        }
        let t = r + n;
        let end = self.idx.span_end(self.text, t);
        if dot_dot(&self.text[t..end]) {
            return (end, ELSEWHERE.to_string());
        }
        for k in &self.files.known {
            if let Some(e) = spells(self.text, t, &k.lower) {
                let out = match &k.target {
                    Some(target) => format!("{base}{target}"),
                    None => format!("{} (not copied)", k.source),
                };
                return (e, out);
            }
        }
        for f in &self.files.folders {
            let Some(e) = spells(self.text, t, f) else {
                continue;
            };
            // The folder only as the whole path (a name under it that was
            // not copied is not pointed at).
            let m = seps(self.text, e);
            if self.kept(e + m) == e + m {
                let slash = if m > 0 { "/" } else { "" };
                return (e + m, format!("{base}{}{slash}", self.normalized(t, e)));
            }
        }
        let kept = self.kept(t);
        let name = self.text[t..kept].split('#').next().unwrap_or("");
        let name = strip_position(name);
        if name.is_empty() {
            return (t, base);
        }
        (t + name.len(), self.normalized(t, t + name.len()))
    }

    /// After a known folder of `kind` (ending at byte `r`).
    fn after_root(&self, kind: Kind, r: usize) -> Hit {
        if kind == Kind::Deliverables {
            return self.after_deliverables(r);
        }
        let n = seps(self.text, r);
        let (t, kept) = if n > 0 {
            (r + n, self.kept(r + n))
        } else {
            (r, r)
        };
        if kind == Kind::Private || self.idx.layout_in(t, kept.max(t)) {
            return (kept, ELSEWHERE.to_string());
        }
        let tail = self.normalized(t, kept);
        let out = match (kind, tail.is_empty()) {
            (Kind::Tips, true) => TIPS_FOLDER.to_string(),
            (Kind::Tips, false) => tail,
            (_, true) => "~".to_string(),
            (_, false) => format!("~/{tail}"),
        };
        (kept, out)
    }

    /// The rewrite of a path starting at byte `i`, if it is a known one.
    fn rewrite_at(&self, i: usize, prev: Option<char>) -> Option<Hit> {
        let head = head_at(self.text, i, prev)?;
        let mut tried: Vec<usize> = Vec::new();
        for (h, ks) in &self.by_head {
            if !head.is(self.text, h) {
                continue;
            }
            tried.extend(ks);
        }
        // Most specific first, as the roots are sorted.
        tried.sort_unstable();
        tried.dedup();
        for k in tried {
            let root = &self.roots[k];
            // A variable-rooted home path (`$HOME/proj`) stays as written:
            // it shows no name, and a copied command keeps working.
            if root.kind == Kind::Home && head.is_var() {
                continue;
            }
            if let Some(r) = self.root_end(root, head.end) {
                return Some(self.after_root(root.kind, r));
            }
        }
        // Any other `claude-graph` data folder (another machine or user).
        let kept = self.kept(i);
        self.idx
            .layout_in(i, kept)
            .then(|| (kept, ELSEWHERE.to_string()))
    }
}

/// A `..` (or longer run of dots) component, `%2e` decoded.
fn dot_dot(raw: &str) -> bool {
    let decoded = raw.replace("%2e", ".").replace("%2E", ".");
    decoded
        .split(['/', '\\'])
        .flat_map(|c| c.split("%2f").flat_map(|c| c.split("%5c")))
        .any(|c| c.len() >= 2 && c.chars().all(|ch| ch == '.'))
}

/// `name` without a trailing `:line[:col]`.
fn strip_position(name: &str) -> &str {
    let mut s = name;
    for _ in 0..2 {
        match s.rfind(':') {
            Some(k) if k + 1 < s.len() && s[k + 1..].bytes().all(|b| b.is_ascii_digit()) => {
                s = &s[..k]
            }
            _ => break,
        }
    }
    s
}

/// Could a path start at byte `i` (char `c`): a letter only before `:`,
/// `%` (`%3A`) or as `file:`; the other starts by their first char.
fn candidate(text: &str, i: usize, c: char) -> bool {
    if c.is_ascii_alphabetic() {
        let next = text.as_bytes().get(i + 1).copied();
        return matches!(next, Some(b':' | b'%'))
            || (c.eq_ignore_ascii_case(&'f')
                && text
                    .get(i..i + 5)
                    .is_some_and(|s| s.eq_ignore_ascii_case("file:")));
    }
    matches!(c, '\\' | '/' | '~' | '%' | '$')
}

/// Rewrite the local paths in `text` (see the module doc).
pub fn relink(text: &str, p: &Places) -> String {
    let roots = roots(p);
    let ctx = Ctx {
        text,
        p,
        by_head: by_head(&roots),
        roots,
        files: files(&p.files),
        idx: Index::new(text),
    };
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    let mut i = 0;
    let mut prev: Option<char> = None;
    while i < text.len() {
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        if candidate(text, i, c) {
            if let Some((end, new)) = ctx.rewrite_at(i, prev) {
                let end = floor(text, end.max(i + c.len_utf8()));
                out.push_str(&text[pos..i]);
                out.push_str(&new);
                pos = end;
                prev = text[..end].chars().next_back();
                i = end;
                continue;
            }
        }
        prev = Some(c);
        i += c.len_utf8();
    }
    out.push_str(&text[pos..]);
    super::relink_backstop::backstop(&out, &p.home)
}

#[cfg(test)]
#[path = "relink_tests.rs"]
mod tests;
