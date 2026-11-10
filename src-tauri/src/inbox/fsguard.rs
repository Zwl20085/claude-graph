//! File-system guards for the folders the Inbox writes into or runs in:
//! a folder that must be a plain directory (no symbolic link, no junction,
//! no other reparse point), a source file read without following links and
//! refused when hard linked, a target replaced through a fresh temp file and
//! a rename (so a hard link at the target name is replaced, never written
//! through), and a tree check before a folder is deleted.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

/// `FILE_ATTRIBUTE_REPARSE_POINT`.
#[cfg(windows)]
const REPARSE_POINT: u32 = 0x400;

/// A symbolic link, a junction or any other reparse point (Windows).
pub fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        meta.file_type().is_symlink() || meta.file_attributes() & REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// `path` exists and is a plain directory (not a link, junction or other
/// reparse point). `Err` says why not.
pub fn require_plain_dir(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if is_link(&meta) {
        return Err(format!("{} is a link or junction", path.display()));
    }
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    Ok(())
}

/// Every entry under `root` (and `root` itself) is a plain directory or a
/// regular file: no link, junction or other reparse point anywhere.
pub fn plain_tree(root: &Path) -> Result<(), String> {
    require_plain_dir(root)?;
    let entries = fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", root.display()))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if is_link(&meta) {
            return Err(format!("{} is a link or junction", path.display()));
        }
        if meta.is_dir() {
            plain_tree(&path)?;
        } else if !meta.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
    }
    Ok(())
}

fn refused(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_string())
}

/// Open `path` for reading without following a link or junction at its
/// name; refused unless it is a regular file with exactly one link (a hard
/// link would make the copy someone else's file).
pub fn open_source(path: &Path) -> io::Result<File> {
    let before = fs::symlink_metadata(path)?;
    if is_link(&before) {
        return Err(refused("a link or junction"));
    }
    let file = no_follow(OpenOptions::new().read(true)).open(path)?;
    same_file(&before, &file)?;
    let (regular, links) = file_facts(&file)?;
    if !regular {
        return Err(refused("not a regular file"));
    }
    if links != 1 {
        return Err(refused("a hard link (the file has other names)"));
    }
    Ok(file)
}

#[cfg(windows)]
fn no_follow(opts: &mut OpenOptions) -> &mut OpenOptions {
    use std::os::windows::fs::OpenOptionsExt as _;
    // FILE_FLAG_OPEN_REPARSE_POINT: the link itself is opened, never its target.
    opts.custom_flags(0x0020_0000)
}

/// Not Windows: the name is checked not to be a link before the open, and
/// the opened file is checked to be the one that was looked at.
#[cfg(not(windows))]
fn no_follow(opts: &mut OpenOptions) -> &mut OpenOptions {
    opts
}

/// (a regular file and no reparse point, its link count) of an open file.
#[cfg(windows)]
fn file_facts(file: &File) -> io::Result<(bool, u32)> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value (plain
    // integers); the handle is the open file's and lives for this call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let meta = file.metadata()?;
    let regular = meta.is_file() && info.dwFileAttributes & REPARSE_POINT == 0;
    Ok((regular, info.nNumberOfLinks))
}

/// Windows opens the name itself (no following), so nothing to compare.
#[cfg(windows)]
fn same_file(_before: &Metadata, _file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn same_file(before: &Metadata, file: &File) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let after = file.metadata()?;
    if (before.dev(), before.ino()) == (after.dev(), after.ino()) {
        Ok(())
    } else {
        Err(refused("the file changed while it was opened"))
    }
}

#[cfg(unix)]
fn file_facts(file: &File) -> io::Result<(bool, u32)> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = file.metadata()?;
    Ok((
        meta.is_file(),
        u32::try_from(meta.nlink()).unwrap_or(u32::MAX),
    ))
}

static TEMP_SEQ: AtomicU32 = AtomicU32::new(0);

/// Write `data` to `target` through a new temp file next to it
/// (`create_new`, flushed) and a rename over the name: whatever was at the
/// name (a hard link to another file included) is replaced, never written
/// through. The temp file is removed when anything fails.
pub fn write_replacing(target: &Path, data: &mut dyn Read) -> io::Result<()> {
    let dir = target.parent().ok_or_else(|| refused("no parent folder"))?;
    let name = target
        .file_name()
        .ok_or_else(|| refused("no file name"))?
        .to_string_lossy();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.{}-{seq}.tmp", std::process::id()));
    let written = (|| {
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        io::copy(data, &mut out)?;
        out.flush()?;
        out.sync_all()?;
        drop(out);
        fs::rename(&temp, target)
    })();
    if written.is_err() {
        if let Err(e) = fs::remove_file(&temp) {
            if e.kind() != io::ErrorKind::NotFound {
                log::warn!("removing {}: {e}", temp.display());
            }
        }
    }
    written
}

#[cfg(test)]
#[path = "fsguard_tests.rs"]
pub(crate) mod tests;
