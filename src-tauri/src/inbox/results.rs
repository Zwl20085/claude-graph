//! Copy a round's files to `<tips folder>/agent/<tip id>/` (protocol 7.3):
//! the final reply as `result.md`, the round's prompt as `prompt.md`, plus
//! the node's deliverables, by name.
//! Nothing is ever deleted there. Symbolic links (as sources or targets) and
//! anything that would resolve outside the results folder are refused and
//! reported; so are files beyond the size and count caps.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

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
/// target is absent or a regular file (a link or folder is never written
/// through).
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

fn copy_one(src: &Path, dest_root: &Path, canon_root: &Path, rel: &Path) -> io::Result<()> {
    fs::copy(src, target(dest_root, canon_root, rel)?).map(|_| ())
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
            .and_then(|t| fs::write(t, text.as_bytes()));
        report.note(PROMPT_NAME, written);
    }
    if let Some(r) = result_md.filter(|p| p.is_file()) {
        let copied = copy_one(r, dest, &canon_root, Path::new(REPLY_NAME));
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
        let size = fs::metadata(&src).map_or(0, |m| m.len());
        if report.copied.len() >= MAX_FILES || bytes + size > MAX_BYTES {
            report.refused.push(format!("{shown}: over the copy cap"));
            continue;
        }
        let copied = copy_one(&src, dest, canon_root, &rel);
        if copied.is_ok() {
            bytes += size;
        }
        report.note(&shown, copied);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_by_name_and_never_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let tips = tmp.path().join("tips");
        fs::create_dir(&tips).unwrap();
        let src = tmp.path().join("deliv");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("survey.md"), "new").unwrap();
        fs::write(src.join("sub").join("t.csv"), "1,2").unwrap();
        let reply = tmp.path().join("reply.md");
        fs::write(&reply, "the reply").unwrap();
        let dest = results_dir(&tips, "my-tip").unwrap();
        fs::write(dest.join("old.md"), "from round 1").unwrap();
        fs::write(dest.join("survey.md"), "old").unwrap();
        let r = copy_round(&dest, Some(&reply), None, &src);
        // Deliverables first, the reply last.
        assert_eq!(r.copied, ["sub/t.csv", "survey.md", "result.md"]);
        assert!(r.refused.is_empty(), "{:?}", r.refused);
        assert_eq!(fs::read_to_string(dest.join("survey.md")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(dest.join("old.md")).unwrap(),
            "from round 1"
        );
        assert_eq!(
            fs::read_to_string(dest.join("result.md")).unwrap(),
            "the reply"
        );
        // Missing deliverables folder: just the reply.
        let r = copy_round(&dest, None, None, &tmp.path().join("none"));
        assert!(r.copied.is_empty() && r.refused.is_empty());
    }

    /// Item 2: the round's prompt is kept as `prompt.md` (latest round's,
    /// overwritten by name like `result.md`), written before the reply; a
    /// deliverable of that name never replaces it.
    #[test]
    fn the_prompt_is_kept_next_to_the_reply() {
        let tmp = tempfile::tempdir().unwrap();
        let tips = tmp.path().join("tips");
        fs::create_dir(&tips).unwrap();
        let src = tmp.path().join("deliv");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("PROMPT.md"), "the agent's own prompt file").unwrap();
        let reply = tmp.path().join("reply.md");
        fs::write(&reply, "the reply").unwrap();
        let dest = results_dir(&tips, "t").unwrap();
        fs::write(dest.join(PROMPT_NAME), "round 1 prompt").unwrap();
        let r = copy_round(&dest, Some(&reply), Some("round 2 prompt\n"), &src);
        assert_eq!(
            r.copied,
            [DELIVERABLE_PROMPT_NAME, PROMPT_NAME, REPLY_NAME],
            "{:?}",
            r.refused
        );
        assert_eq!(
            fs::read_to_string(dest.join(PROMPT_NAME)).unwrap(),
            "round 2 prompt\n"
        );
        assert_eq!(
            fs::read_to_string(dest.join(DELIVERABLE_PROMPT_NAME)).unwrap(),
            "the agent's own prompt file"
        );
        // No prompt given (the round never ran): an earlier one is left as is.
        let r = copy_round(&dest, None, None, &tmp.path().join("none"));
        assert!(r.copied.is_empty());
        assert_eq!(
            fs::read_to_string(dest.join(PROMPT_NAME)).unwrap(),
            "round 2 prompt\n"
        );
    }

    #[test]
    fn a_prompt_target_that_is_not_a_regular_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let tips = tmp.path().join("tips");
        fs::create_dir(&tips).unwrap();
        let dest = results_dir(&tips, "t").unwrap();
        fs::create_dir(dest.join(PROMPT_NAME)).unwrap();
        let r = copy_round(&dest, None, Some("p"), &tmp.path().join("none"));
        assert!(r.copied.is_empty());
        assert!(
            r.refused[0].starts_with("prompt.md: target exists and is not a regular file"),
            "{:?}",
            r.refused
        );
    }

    #[test]
    fn bad_ids_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        for id in ["..", "a/b", "a\\b", "", "tip.", "tip ", "a.b."] {
            assert!(results_dir(tmp.path(), id).is_err(), "{id}");
        }
        fs::write(tmp.path().join("agent"), "file").unwrap();
        assert!(results_dir(tmp.path(), "x").is_err());
    }

    #[test]
    fn a_deliverable_named_result_md_never_replaces_the_reply() {
        let tmp = tempfile::tempdir().unwrap();
        let tips = tmp.path().join("tips");
        fs::create_dir(&tips).unwrap();
        let src = tmp.path().join("deliv");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("Result.MD"), "the agent's own result").unwrap();
        fs::write(src.join("sub").join("result.md"), "nested").unwrap();
        let reply = tmp.path().join("reply.md");
        fs::write(&reply, "the reply").unwrap();
        let dest = results_dir(&tips, "t").unwrap();
        let r = copy_round(&dest, Some(&reply), None, &src);
        assert_eq!(
            r.copied,
            [DELIVERABLE_RESULT_NAME, "sub/result.md", "result.md"]
        );
        assert_eq!(
            fs::read_to_string(dest.join("result.md")).unwrap(),
            "the reply"
        );
        assert_eq!(
            fs::read_to_string(dest.join(DELIVERABLE_RESULT_NAME)).unwrap(),
            "the agent's own result"
        );
    }

    #[test]
    fn an_unreadable_deliverables_path_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let tips = tmp.path().join("tips");
        fs::create_dir(&tips).unwrap();
        // A file where the deliverables folder should be: read_dir fails, not NotFound.
        let not_a_dir = tmp.path().join("deliv");
        fs::write(&not_a_dir, "x").unwrap();
        let dest = results_dir(&tips, "t").unwrap();
        let r = copy_round(&dest, None, None, &not_a_dir);
        assert!(r.copied.is_empty());
        assert!(
            r.refused[0].contains("cannot read the folder"),
            "{:?}",
            r.refused
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let tips = tmp.path().join("tips");
        fs::create_dir_all(tips.join("agent")).unwrap();
        std::os::unix::fs::symlink(&outside, tips.join("agent").join("x")).unwrap();
        assert!(results_dir(&tips, "x").is_err());
        let src = tmp.path().join("d");
        fs::create_dir(&src).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("p")).unwrap();
        let dest = results_dir(&tips, "y").unwrap();
        let r = copy_round(&dest, None, None, &src);
        assert!(r.copied.is_empty() && r.refused[0].contains("symbolic link"));
        // A symlinked prompt.md target is not followed.
        std::os::unix::fs::symlink("/etc/passwd", dest.join(PROMPT_NAME)).unwrap();
        let r = copy_round(&dest, None, Some("p"), &tmp.path().join("none"));
        assert!(r.copied.is_empty() && r.refused[0].contains("not a regular file"));
    }
}
