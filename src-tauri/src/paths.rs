//! Graph-folder arguments from the frontend (`dir` in every command).
//!
//! A `dir` must be an absolute local path: relative paths, UNC paths
//! (`\\server\share`) and verbatim UNC (`\\?\UNC\…`) are refused. The folder is
//! canonicalised to its long form (no 8.3 segments, no `\\?\` prefix on a plain
//! drive path), so every later check (`starts_with`, the run registry, trust
//! keys) compares one spelling.

use std::path::{Path, PathBuf};

use crate::contract::{AppError, AppResult};

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new("invalid_input", msg)
}

/// `\\server\share`, `//server/share`, `\\?\UNC\…`, `\\.\…` and other
/// device or network forms.
pub fn is_unc_or_device(p: &str) -> bool {
    let s = p.trim();
    s.starts_with(r"\\") || s.starts_with("//") || s.starts_with(r"\??\")
}

/// `\\?\C:\x` -> `C:\x`; `None` for verbatim UNC or other forms we keep as given.
pub fn strip_verbatim(path: PathBuf) -> Option<PathBuf> {
    let s = path.to_str()?;
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => Some(PathBuf::from(rest)),
        Some(_) => None,
        None => Some(path),
    }
}

/// Canonical long form of an existing path, refusing anything that resolves to
/// a network or device path (a mapped network drive canonicalises to UNC).
pub fn canonical(path: &Path) -> AppResult<PathBuf> {
    let long = std::fs::canonicalize(path)
        .map_err(|e| AppError::new("not_found", format!("{}: {e}", path.display())))?;
    let plain = strip_verbatim(long.clone()).ok_or_else(|| {
        invalid(format!(
            "{} is a network or device path; use a local folder",
            long.display()
        ))
    })?;
    if is_unc_or_device(&plain.to_string_lossy()) {
        return Err(invalid(format!(
            "{} is a network path; use a local folder",
            plain.display()
        )));
    }
    Ok(plain)
}

fn checked_absolute(dir: &str) -> AppResult<PathBuf> {
    let trimmed = dir.trim();
    if trimmed.is_empty() {
        return Err(invalid("no graph folder given"));
    }
    if is_unc_or_device(trimmed) {
        return Err(invalid(format!(
            "{trimmed} is a network or device path; use a local folder"
        )));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(invalid(format!("{trimmed} is not an absolute path")));
    }
    Ok(path)
}

/// An existing graph folder (open, save, run, …), canonicalised.
pub fn existing_dir(dir: &str) -> AppResult<PathBuf> {
    let path = checked_absolute(dir)?;
    if !path.is_dir() {
        return Err(AppError::new(
            "not_found",
            format!("{} is not an existing folder", path.display()),
        ));
    }
    canonical(&path)
}

/// A folder to create a graph in: it may not exist yet, but its parent must.
/// Returns the canonical parent joined with the final name.
pub fn dir_for_create(dir: &str) -> AppResult<PathBuf> {
    let path = checked_absolute(dir)?;
    if path.is_dir() {
        return canonical(&path);
    }
    let name = path
        .file_name()
        .ok_or_else(|| invalid(format!("{} has no folder name", path.display())))?
        .to_os_string();
    let parent = path.parent().filter(|p| p.is_dir()).ok_or_else(|| {
        AppError::new(
            "not_found",
            format!("the parent folder of {} does not exist", path.display()),
        )
    })?;
    Ok(canonical(parent)?.join(name))
}

/// The graph folder in long, absolute form, without validation (tests and
/// internal callers that already hold a checked path). Falls back to the
/// absolute path if the folder cannot be canonicalised.
pub fn abs(dir: &Path) -> PathBuf {
    let plain = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    match std::fs::canonicalize(&plain) {
        Ok(long) => strip_verbatim(long).unwrap_or(plain),
        Err(_) => plain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_verbatim_keeps_plain_drive_paths() {
        let verbatim = format!("{}{}", r"\\?\", r"C:\Users\x");
        assert_eq!(
            strip_verbatim(PathBuf::from(verbatim)),
            Some(PathBuf::from(r"C:\Users\x"))
        );
        let unc = format!("{}{}", r"\\?\", r"UNC\host\share");
        assert_eq!(strip_verbatim(PathBuf::from(unc)), None);
        assert_eq!(
            strip_verbatim(PathBuf::from("/work/g")),
            Some(PathBuf::from("/work/g"))
        );
    }

    #[test]
    fn abs_gives_an_existing_absolute_long_path() {
        let tmp = tempfile::tempdir().unwrap();
        let got = abs(tmp.path());
        assert!(got.is_absolute() && got.is_dir());
        assert!(!got.to_string_lossy().starts_with(r"\\?\"));
        assert!(!got.to_string_lossy().contains('~'), "{}", got.display());
    }

    #[test]
    fn rejects_relative_unc_and_missing() {
        for bad in [
            "",
            "  ",
            "rel/dir",
            r"\\server\share\g",
            "//server/share",
            r"\\?\UNC\h\s",
        ] {
            let err = existing_dir(bad).unwrap_err();
            assert_eq!(err.code, "invalid_input", "{bad}");
        }
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");
        assert_eq!(
            existing_dir(&missing.display().to_string())
                .unwrap_err()
                .code,
            "not_found"
        );
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            existing_dir(&file.display().to_string()).unwrap_err().code,
            "not_found"
        );
    }

    #[test]
    fn existing_and_create_forms() {
        let tmp = tempfile::tempdir().unwrap();
        let got = existing_dir(&tmp.path().display().to_string()).unwrap();
        assert_eq!(got, abs(tmp.path()));
        let new = tmp.path().join("new graph");
        let got = dir_for_create(&new.display().to_string()).unwrap();
        assert_eq!(got, abs(tmp.path()).join("new graph"));
        let deep = tmp.path().join("a").join("b");
        assert_eq!(
            dir_for_create(&deep.display().to_string())
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            dir_for_create("relative").unwrap_err().code,
            "invalid_input"
        );
    }
}
