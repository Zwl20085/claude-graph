use std::fs;
use std::io::Read as _;
use std::path::Path;

use super::*;

/// A directory junction (Windows, no admin needed) or a symlink (unix) at
/// `link` pointing to `target`.
pub(crate) fn dir_link(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "mklink /J {}", link.display());
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn plain_dirs_pass_and_links_files_and_missing_ones_do_not() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("d");
    fs::create_dir(&dir).unwrap();
    assert!(require_plain_dir(&dir).is_ok());
    let file = tmp.path().join("f");
    fs::write(&file, "x").unwrap();
    assert!(require_plain_dir(&file)
        .unwrap_err()
        .contains("is not a folder"));
    assert!(require_plain_dir(&tmp.path().join("none")).is_err());
    let link = tmp.path().join("j");
    dir_link(&dir, &link);
    let why = require_plain_dir(&link).unwrap_err();
    assert!(why.contains("is a link or junction"), "{why}");
    assert!(is_link(&fs::symlink_metadata(&link).unwrap()));
    assert!(!is_link(&fs::symlink_metadata(&dir).unwrap()));
}

#[test]
fn a_hard_linked_source_is_refused_and_a_plain_one_is_read() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a.md");
    fs::write(&a, "data").unwrap();
    let mut text = String::new();
    open_source(&a).unwrap().read_to_string(&mut text).unwrap();
    assert_eq!(text, "data");
    fs::hard_link(&a, tmp.path().join("b.md")).unwrap();
    let err = open_source(&a).unwrap_err().to_string();
    assert!(err.contains("hard link"), "{err}");
    assert!(open_source(tmp.path()).is_err(), "a folder is not a source");
}

#[test]
fn a_hard_linked_target_is_replaced_not_written_through() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "precious").unwrap();
    let dest = tmp.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let target = dest.join("prompt.md");
    fs::hard_link(&outside, &target).unwrap();
    write_replacing(&target, &mut "new".as_bytes()).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "precious");
    // No temp file left behind.
    let names: Vec<String> = fs::read_dir(&dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, ["prompt.md"]);
}

#[test]
fn plain_trees_pass_and_a_link_anywhere_inside_is_named() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("round");
    fs::create_dir_all(root.join("nodes").join("task")).unwrap();
    fs::write(root.join("nodes").join("task").join("x.md"), "x").unwrap();
    assert!(plain_tree(&root).is_ok());
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep.md"), "keep").unwrap();
    dir_link(&elsewhere, &root.join("nodes").join("j"));
    let why = plain_tree(&root).unwrap_err();
    assert!(why.contains("is a link or junction"), "{why}");
    assert!(elsewhere.join("keep.md").is_file());
}

/// CI (windows-latest): a scanner briefly holding the target (or the temp
/// file) open without delete sharing makes the rename fail with a sharing
/// violation; the write waits a moment and succeeds instead of losing the
/// file.
#[cfg(windows)]
#[test]
fn a_target_held_open_briefly_is_still_replaced() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("result.md");
    fs::write(&target, "old").unwrap();
    // share_mode(0): no read, write or delete sharing, as a scanner may.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&target)
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        drop(held);
    });
    write_replacing(&target, &mut "new".as_bytes()).unwrap();
    release.join().unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
}

/// A target that stays locked: the write gives up after the bounded retry
/// with the error, and leaves no temp file.
#[cfg(windows)]
#[test]
fn a_target_locked_for_good_fails_without_a_temp_file() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("result.md");
    fs::write(&target, "old").unwrap();
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&target)
        .unwrap();
    let t0 = std::time::Instant::now();
    assert!(write_replacing(&target, &mut "new".as_bytes()).is_err());
    assert!(t0.elapsed() < std::time::Duration::from_secs(5));
    let names: Vec<String> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, ["result.md"]);
}
