//! Copy a round's files to `<tips folder>/agent/<tip id>/` (protocol 7.3):
//! the final reply as `result.md` plus the node's deliverables, by name.
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

fn copy_one(src: &Path, dest_root: &Path, canon_root: &Path, rel: &Path) -> io::Result<()> {
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
    fs::copy(src, &dest).map(|_| ())
}

/// The reply's name in the results folder.
pub const REPLY_NAME: &str = "result.md";
/// What a deliverable named `result.md` is copied as (the reply keeps the name).
pub const DELIVERABLE_RESULT_NAME: &str = "deliverable-result.md";

/// Copy every file under `deliverables`, then `result_md` (as `result.md`,
/// when given, last so it is always the reply) into `dest`, which
/// [`results_dir`] made. A top-level deliverable named `result.md` is copied
/// as `deliverable-result.md`. The reply does not count against the caps.
pub fn copy_round(dest: &Path, result_md: Option<&Path>, deliverables: &Path) -> CopyReport {
    let mut report = CopyReport::default();
    let canon_root = match fs::canonicalize(dest) {
        Ok(c) => c,
        Err(e) => {
            report.refused.push(format!("{}: {e}", dest.display()));
            return report;
        }
    };
    let mut rels = Vec::new();
    walk(deliverables, Path::new(""), &mut rels, &mut report.refused);
    let mut files: Vec<(PathBuf, PathBuf, bool)> = rels
        .into_iter()
        .map(|r| {
            let target = if r.as_os_str().eq_ignore_ascii_case(REPLY_NAME) {
                PathBuf::from(DELIVERABLE_RESULT_NAME)
            } else {
                r.clone()
            };
            (deliverables.join(&r), target, false)
        })
        .collect();
    if let Some(r) = result_md.filter(|p| p.is_file()) {
        files.push((r.to_path_buf(), PathBuf::from(REPLY_NAME), true));
    }
    let mut bytes = 0u64;
    for (src, rel, is_reply) in files {
        let shown = rel.display().to_string().replace('\\', "/");
        let size = fs::metadata(&src).map_or(0, |m| m.len());
        if !is_reply && (report.copied.len() >= MAX_FILES || bytes + size > MAX_BYTES) {
            report.refused.push(format!("{shown}: over the copy cap"));
            continue;
        }
        match copy_one(&src, dest, &canon_root, &rel) {
            Ok(()) => {
                bytes += size;
                report.copied.push(shown);
            }
            Err(e) => report.refused.push(format!("{shown}: {e}")),
        }
    }
    report
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
        let r = copy_round(&dest, Some(&reply), &src);
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
        let r = copy_round(&dest, None, &tmp.path().join("none"));
        assert!(r.copied.is_empty() && r.refused.is_empty());
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
        let r = copy_round(&dest, Some(&reply), &src);
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
        let r = copy_round(&dest, None, &not_a_dir);
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
        let r = copy_round(&dest, None, &src);
        assert!(r.copied.is_empty() && r.refused[0].contains("symbolic link"));
    }
}
