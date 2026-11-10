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

/// Most files copied per round.
pub const MAX_FILES: usize = 200;
/// Most bytes copied per round.
pub const MAX_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CopyReport {
    /// Relative paths copied.
    pub copied: Vec<String>,
    /// `path: why` for everything refused or failed.
    pub refused: Vec<String>,
}

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

/// Regular files under `root` (relative paths); symlinks and unreadable
/// folders are reported (a missing deliverables folder is not an error).
fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>, refused: &mut Vec<String>) {
    let entries = match fs::read_dir(root.join(rel)) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound && rel.as_os_str().is_empty() => return,
        Err(e) => {
            let shown = if rel.as_os_str().is_empty() {
                root.display().to_string()
            } else {
                rel.display().to_string()
            };
            refused.push(format!("{shown}: cannot read the folder: {e}"));
            return;
        }
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let r = rel.join(e.file_name());
        match e.file_type() {
            Ok(t) if t.is_symlink() => refused.push(format!("{}: symbolic link", r.display())),
            Ok(t) if t.is_dir() => walk(root, &r, out, refused),
            Ok(t) if t.is_file() => out.push(r),
            _ => refused.push(format!("{}: not a regular file", r.display())),
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
/// `prompt.md`, when given), then copy `result_md` (as `result.md`, when
/// given, last so it is always the reply) into `dest`, which [`results_dir`]
/// made. Top-level deliverables named `result.md` / `prompt.md` are copied
/// as `deliverable-result.md` / `deliverable-prompt.md`. The prompt and the
/// reply do not count against the caps; both replace an earlier round's
/// file of the same name (as every file here, protocol 7.3).
pub fn copy_round(
    dest: &Path,
    result_md: Option<&Path>,
    prompt: Option<&str>,
    deliverables: &Path,
) -> CopyReport {
    let mut report = CopyReport::default();
    let canon_root = match fs::canonicalize(dest) {
        Ok(c) => c,
        Err(e) => {
            report.refused.push(format!("{}: {e}", dest.display()));
            return report;
        }
    };
    copy_deliverables(dest, &canon_root, deliverables, &mut report);
    if let Some(text) = prompt {
        let written = target(dest, &canon_root, Path::new(PROMPT_NAME))
            .and_then(|t| fsguard::write_replacing(&t, &mut text.as_bytes()));
        report.note(PROMPT_NAME, written);
    }
    if let Some(r) = result_md.filter(|p| fs::symlink_metadata(p).is_ok()) {
        let copied =
            open(r).and_then(|(file, _)| copy_one(file, dest, &canon_root, Path::new(REPLY_NAME)));
        report.note(REPLY_NAME, copied);
    }
    report
}

impl CopyReport {
    fn note(&mut self, shown: &str, result: io::Result<()>) {
        match result {
            Ok(()) => self.copied.push(shown.to_string()),
            Err(e) => self.refused.push(format!("{shown}: {e}")),
        }
    }
}

/// Every file under `deliverables`, within the caps.
fn copy_deliverables(dest: &Path, canon_root: &Path, deliverables: &Path, report: &mut CopyReport) {
    let mut rels = Vec::new();
    walk(deliverables, Path::new(""), &mut rels, &mut report.refused);
    let mut bytes = 0u64;
    for r in rels {
        let src = deliverables.join(&r);
        let rel = deliverable_target(&r);
        let shown = rel.display().to_string().replace('\\', "/");
        let (file, size) = match open(&src) {
            Ok(opened) => opened,
            Err(e) => {
                report.note(&shown, Err(e));
                continue;
            }
        };
        if report.copied.len() >= MAX_FILES || bytes + size > MAX_BYTES {
            report.refused.push(format!("{shown}: over the copy cap"));
            continue;
        }
        let copied = copy_one(file, dest, canon_root, &rel);
        if copied.is_ok() {
            bytes += size;
        }
        report.note(&shown, copied);
    }
}

#[cfg(test)]
#[path = "results_tests.rs"]
mod tests;
