use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::inbox::fsguard::tests::dir_link;
use crate::inbox::graphgen::{inbox_root, tip_dir, ROUNDS_DIR};

/// A canonical data dir in a temp folder.
fn data() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let data = crate::paths::abs(tmp.path()).join("claude-graph");
    fs::create_dir(&data).unwrap();
    (tmp, data)
}

#[test]
fn a_new_round_folder_is_created_once_and_passes_the_checks() {
    let (_tmp, data) = data();
    let dir = check_before_claim(&data, "t", "r1").unwrap();
    assert!(!dir.exists(), "nothing is created before the claim");
    assert!(!inbox_root(&data).exists());
    let made = create(&data, "t", "r1").unwrap();
    assert_eq!(made, dir);
    assert!(made.is_dir());
    assert!(recheck(&data, &made).is_ok());
    // A round folder is always new.
    let again = create(&data, "t", "r1").unwrap_err();
    assert!(again.contains("already exists"), "{again}");
    let before = check_before_claim(&data, "t", "r1").unwrap_err();
    assert!(before.contains("already exists"), "{before}");
}

/// S1: a junction at any level the Inbox owns (`inbox`, `rounds`,
/// `rounds/<tip>`) is refused before the claim, at creation and right
/// before the start.
#[test]
fn a_junction_at_any_owned_level_is_refused() {
    let (tmp, data) = data();
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("CLAUDE.md"), "planted").unwrap();
    let inbox = inbox_root(&data);
    let rounds = inbox.join(ROUNDS_DIR);
    let tip = tip_dir(&inbox, "t");
    for level in [&inbox, &rounds, &tip] {
        if let Some(parent) = level.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let target = elsewhere.join("inner");
        fs::create_dir_all(&target).unwrap();
        dir_link(&target, level);
        let why = check_before_claim(&data, "t", "r1").unwrap_err();
        assert!(why.contains("is a link or junction"), "{why}");
        let why = create(&data, "t", "r1").unwrap_err();
        assert!(why.contains("is a link or junction"), "{why}");
        assert!(!target.join("r1").exists(), "nothing created through it");
        fs::remove_dir(level).unwrap();
        fs::create_dir_all(level).unwrap();
    }
}

/// S3: a swap after the folder was made (the tip's folder of rounds moved
/// away and replaced by a junction to a place with planted files) is caught
/// by the last check.
#[test]
fn a_swap_after_creation_is_caught_right_before_the_start() {
    let (tmp, data) = data();
    let dir = create(&data, "t", "r1").unwrap();
    let tip = dir.parent().unwrap().to_path_buf();
    let moved = tmp.path().join("moved");
    fs::rename(&tip, &moved).unwrap();
    fs::write(moved.join("CLAUDE.md"), "planted").unwrap();
    dir_link(&moved, &tip);
    let why = recheck(&data, &dir).unwrap_err();
    assert!(why.contains("is a link or junction"), "{why}");
}

#[test]
fn planted_files_in_a_parent_are_refused_and_a_siblings_are_not() {
    let (_tmp, data) = data();
    let r1 = create(&data, "t", "r1").unwrap();
    fs::create_dir(r1.join(".claude")).unwrap();
    fs::write(r1.join("CLAUDE.md"), "x").unwrap();
    assert!(check_before_claim(&data, "t", "r2").is_ok());
    let r2 = create(&data, "t", "r2").unwrap();
    assert!(recheck(&data, &r2).is_ok());
    let tip = r1.parent().unwrap();
    fs::write(tip.join("CLAUDE.md"), "x").unwrap();
    let why = check_before_claim(&data, "t", "r3").unwrap_err();
    assert!(
        why.contains(&format!("{} contains CLAUDE.md", tip.display())),
        "{why}"
    );
    assert!(recheck(&data, &r2).is_err());
}

#[test]
fn a_round_path_outside_the_data_dir_is_refused() {
    let (tmp, data) = data();
    let outside: &Path = tmp.path();
    let why = recheck(&data, &outside.join("x")).unwrap_err();
    assert!(why.contains("outside"), "{why}");
}
