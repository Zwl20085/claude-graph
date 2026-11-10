//! Copy a round's files to `<tips folder>/agent/<tip id>/` (protocol 7.3):
//! the final reply as `result.md`, the round's prompt as `prompt.md`, plus
//! the node's deliverables, by name.
//! Nothing is ever deleted there. Symbolic links (as sources or targets) and
//! anything that would resolve outside the results folder are refused and
//! reported; so are files beyond the size and count caps.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::fsguard;
use super::relink::CopiedFile;
use super::relink::{sentinel, sentinel_index, SENTINEL_COUNT};

/// Most files copied per round.
pub const MAX_FILES: usize = 200;
/// Most bytes copied per round.
pub const MAX_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CopyReport {
    /// Relative paths copied.
    pub copied: Vec<String>,
    /// `path: why` for everything refused or failed (for the log).
    pub refused: Vec<String>,
    /// Names (relative, `/`) of the files that were not copied, for the tip.
    pub not_copied: Vec<String>,
    /// Each deliverable: its path under the deliverables folder and its name
    /// in the results folder (`None`: not copied).
    pub files: Vec<CopiedFile>,
    /// The reply file did not exist (normal for a round that failed early).
    pub reply_missing: bool,
    /// The walk stopped at [`WALK_MAX`] entries.
    pub walk_cut: bool,
    /// Why `result.md` is the reply byte for byte, not rewritten.
    pub reply_as_written: Option<&'static str>,
}

/// Largest reply that is rewritten; a larger one is copied as written.
pub const REWRITE_MAX: u64 = 32 * 1024 * 1024;
/// Why a reply was copied as written.
pub const AS_WRITTEN_LARGE: &str = "over 32 MiB";

/// How the results folder is named when it could not be used at all.
pub const RESULTS_FOLDER_NAME: &str = "(the results folder)";
/// How further files are named once the walk stopped.
pub const MORE_FILES_NAME: &str = "(more files)";
/// Most entries the walk lists (copied or not): the copy cap plus a margin,
/// so the report and the tip's line stay bounded.
pub const WALK_MAX: usize = MAX_FILES + 50;

/// What the reply text becomes before it is written as `result.md`, given
/// what was copied (the runner's path rewriting, [`super::relink`]).
pub type ReplyFix<'a> = &'a dyn Fn(&str, &CopyReport) -> String;

/// How the deliverables folder itself is named when it cannot be read.
const DELIVERABLES_NAME: &str = "(the deliverables folder)";

/// `<tips>/agent/<tip id>`, created; refuses a symlinked `agent` or results
/// folder and a tip id that is not one plain path component.
pub fn results_dir(tips: &Path, tip_id: &str) -> io::Result<PathBuf> {
    let plain = matches!(
        Path::new(tip_id)
            .components()
            .collect::<Vec<_>>()
            .as_slice(),
        [Component::Normal(_)]
    );
    // Windows drops a trailing dot or space of a folder name: refused, the
    // scan already skips such tips (scan::bad_id).
    if !plain || tip_id.contains(['/', '\\']) || tip_id.ends_with(['.', ' ']) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad tip id"));
    }
    let agent = tips.join("agent");
    let dir = agent.join(tip_id);
    for d in [&agent, &dir] {
        match fs::symlink_metadata(d) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is a symbolic link", d.display()),
                ))
            }
            Ok(m) if !m.is_dir() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a folder", d.display()),
                ))
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(d)?,
            Err(e) => return Err(e),
        }
    }
    let canon = fs::canonicalize(&dir)?;
    if !canon.starts_with(fs::canonicalize(tips)?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "results folder resolves outside the tips folder",
        ));
    }
    Ok(dir)
}

/// `rel` with `/` separators.
fn slashed(rel: &Path) -> String {
    rel.display().to_string().replace('\\', "/")
}

/// Regular files under `root` (relative paths); symlinks and unreadable
/// folders are reported (a missing deliverables folder is not an error).
fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>, report: &mut CopyReport) {
    if report.walk_cut {
        return;
    }
    let entries = match fs::read_dir(root.join(rel)) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound && rel.as_os_str().is_empty() => return,
        Err(e) => {
            let name = if rel.as_os_str().is_empty() {
                DELIVERABLES_NAME.to_string()
            } else {
                slashed(rel)
            };
            report.refuse(&name, &format!("cannot read the folder: {e}"));
            return;
        }
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() + report.not_copied.len() >= WALK_MAX {
            report.walk_cut = true;
            report.refuse(MORE_FILES_NAME, "over the copy cap");
            return;
        }
        let r = rel.join(e.file_name());
        match e.file_type() {
            Ok(t) if t.is_symlink() => report.refuse(&slashed(&r), "symbolic link"),
            Ok(t) if t.is_dir() => walk(root, &r, out, report),
            Ok(t) if t.is_file() => out.push(r),
            _ => report.refuse(&slashed(&r), "not a regular file"),
        }
    }
}

/// `dest_root/rel`, once its folder is inside the results folder and the
/// target is absent or a regular file (a link or folder is refused). It is
/// then replaced through a temp file and a rename
/// ([`fsguard::write_replacing`]), so a hard link there is never written
/// through.
fn target(dest_root: &Path, canon_root: &Path, rel: &Path) -> io::Result<PathBuf> {
    let dest = dest_root.join(rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
        if !fs::canonicalize(parent)?.starts_with(canon_root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "outside the results folder",
            ));
        }
    }
    if let Ok(m) = fs::symlink_metadata(&dest) {
        if !m.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "target exists and is not a regular file",
            ));
        }
    }
    Ok(dest)
}

/// A source opened for copying ([`fsguard::open_source`]: no link followed,
/// a hard-linked file refused) and its size.
fn open(src: &Path) -> io::Result<(fs::File, u64)> {
    let file = fsguard::open_source(src)?;
    let size = file.metadata()?.len();
    Ok((file, size))
}

fn copy_one(mut file: fs::File, dest_root: &Path, canon_root: &Path, rel: &Path) -> io::Result<()> {
    fsguard::write_replacing(&target(dest_root, canon_root, rel)?, &mut file)
}

/// The reply's name in the results folder.
pub const REPLY_NAME: &str = "result.md";
/// What a deliverable named `result.md` is copied as (the reply keeps the name).
pub const DELIVERABLE_RESULT_NAME: &str = "deliverable-result.md";
/// The round's prompt (the task text its brief was built from).
pub const PROMPT_NAME: &str = "prompt.md";
/// What a deliverable named `prompt.md` is copied as.
pub const DELIVERABLE_PROMPT_NAME: &str = "deliverable-prompt.md";

/// A top-level deliverable's name in the results folder: `result.md` and
/// `prompt.md` are the runner's own, so those deliverables are renamed.
fn deliverable_target(rel: &Path) -> PathBuf {
    let s = rel.as_os_str();
    if s.eq_ignore_ascii_case(REPLY_NAME) {
        PathBuf::from(DELIVERABLE_RESULT_NAME)
    } else if s.eq_ignore_ascii_case(PROMPT_NAME) {
        PathBuf::from(DELIVERABLE_PROMPT_NAME)
    } else {
        rel.to_path_buf()
    }
}

/// Copy every file under `deliverables`, then write `prompt` (as
/// `prompt.md`, when given), then the reply read from `result_md` and passed
/// through `fix` (as `result.md`, when given, last so it is always the
/// reply) into `dest`, which [`results_dir`] made. Top-level deliverables
/// named `result.md` / `prompt.md` are copied as `deliverable-result.md` /
/// `deliverable-prompt.md`. The prompt and the reply do not count against
/// the caps; both replace an earlier round's file of the same name (as
/// every file here, protocol 7.3).
pub fn copy_round(
    dest: &Path,
    result_md: Option<&Path>,
    prompt: Option<&str>,
    deliverables: &Path,
    fix: ReplyFix<'_>,
) -> CopyReport {
    let mut report = CopyReport::default();
    let canon_root = match fs::canonicalize(dest) {
        Ok(c) => c,
        Err(e) => {
            report.refuse(RESULTS_FOLDER_NAME, &e.to_string());
            return report;
        }
    };
    copy_deliverables(dest, &canon_root, deliverables, &mut report);
    if let Some(text) = prompt {
        let written = target(dest, &canon_root, Path::new(PROMPT_NAME))
            .and_then(|t| fsguard::write_replacing(&t, &mut text.as_bytes()));
        report.note(PROMPT_NAME, written);
    }
    if let Some(r) = result_md {
        let written = read_reply(r, &mut report).and_then(|bytes| {
            let text = if bytes.len() as u64 > REWRITE_MAX {
                report.reply_as_written = Some(AS_WRITTEN_LARGE);
                bytes
            } else {
                rewrite_bytes(&bytes, fix, &report)
            };
            target(dest, &canon_root, Path::new(REPLY_NAME))
                .and_then(|t| fsguard::write_replacing(&t, &mut text.as_slice()))
        });
        report.note(REPLY_NAME, written);
    }
    report
}

/// The reply rewritten by `fix` in one call. Each invalid UTF-8 sequence
/// stands in the text as its own sentinel (which the rewrite skips inside
/// paths) and is put back byte for byte wherever it is still there; one
/// inside a rewritten path goes with that path. A text that already holds
/// sentinel code points, or more invalid sequences than there are
/// sentinels, is rewritten part by part instead (invalid bytes kept).
fn rewrite_bytes(bytes: &[u8], fix: ReplyFix<'_>, report: &CopyReport) -> Vec<u8> {
    let mut text = String::with_capacity(bytes.len());
    let mut bad: Vec<&[u8]> = Vec::new();
    let mut clean = true;
    for chunk in bytes.utf8_chunks() {
        clean &= !chunk.valid().chars().any(|c| sentinel_index(c).is_some());
        text.push_str(chunk.valid());
        if !chunk.invalid().is_empty() {
            clean &= bad.len() < SENTINEL_COUNT;
            text.push(sentinel(bad.len()));
            bad.push(chunk.invalid());
        }
    }
    if !clean {
        let mut out = Vec::with_capacity(bytes.len());
        for chunk in bytes.utf8_chunks() {
            out.extend(fix(chunk.valid(), report).into_bytes());
            out.extend(chunk.invalid());
        }
        return out;
    }
    let fixed = fix(&text, report);
    let mut out = Vec::with_capacity(fixed.len());
    for c in fixed.chars() {
        match sentinel_index(c).and_then(|k| bad.get(k)) {
            Some(raw) => out.extend_from_slice(raw),
            None => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    out
}

/// The reply file's text ([`fsguard::open_source`] rules); a missing one is
/// reported as such (a done tip must not point at a result.md that silently
/// never came) and flagged in the report.
fn read_reply(path: &Path, report: &mut CopyReport) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    match open(path) {
        Ok((mut file, _)) => {
            io::Read::read_to_end(&mut file, &mut bytes)?;
            Ok(bytes)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            report.reply_missing = true;
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the node left no reply file",
            ))
        }
        Err(e) => Err(e),
    }
}

impl CopyReport {
    fn note(&mut self, shown: &str, result: io::Result<()>) {
        match result {
            Ok(()) => self.copied.push(shown.to_string()),
            Err(e) => {
                self.refused.push(format!("{shown}: {e}"));
                let missing_reply = shown == REPLY_NAME && self.reply_missing;
                if !missing_reply {
                    self.not_copied.push(shown.to_string());
                }
            }
        }
    }

    /// A deliverable: `source` (its own name) when not copied, `shown` (the
    /// copy's name) when copied.
    fn note_file(&mut self, source: &str, shown: &str, result: io::Result<()>) {
        match result {
            Ok(()) => self.copied.push(shown.to_string()),
            Err(e) => self.refuse(source, &e.to_string()),
        }
    }

    fn refuse(&mut self, name: &str, why: &str) {
        self.refused.push(format!("{name}: {why}"));
        self.not_copied.push(name.to_string());
    }
}

/// Every file under `deliverables`, within the caps.
fn copy_deliverables(dest: &Path, canon_root: &Path, deliverables: &Path, report: &mut CopyReport) {
    let mut rels = Vec::new();
    walk(deliverables, Path::new(""), &mut rels, report);
    let mut bytes = 0u64;
    // Names already used in the results folder (a deliverable named
    // `deliverable-result.md` and a renamed `result.md` would collide).
    let mut taken = std::collections::HashSet::new();
    for r in rels {
        let src = deliverables.join(&r);
        let rel = deliverable_target(&r);
        let shown = slashed(&rel);
        let source = slashed(&r);
        let copied = match open(&src) {
            Ok(_) if taken.contains(&shown.to_lowercase()) => {
                Err(io::Error::other("its name is taken in the results folder"))
            }
            Ok((_, size)) if report.copied.len() >= MAX_FILES || bytes + size > MAX_BYTES => {
                Err(io::Error::other("over the copy cap"))
            }
            Ok((file, size)) => copy_one(file, dest, canon_root, &rel).map(|()| {
                bytes += size;
            }),
            Err(e) => Err(e),
        };
        if copied.is_ok() {
            taken.insert(shown.to_lowercase());
        }
        report.files.push(CopiedFile {
            source: source.clone(),
            target: copied.is_ok().then(|| shown.clone()),
        });
        report.note_file(&source, &shown, copied);
    }
}

#[cfg(test)]
#[path = "results_tests.rs"]
mod tests;
