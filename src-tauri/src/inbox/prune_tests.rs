use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::NaiveDateTime;

use super::*;
use crate::inbox::fsguard::tests::dir_link;
use crate::inbox::graphgen::{folder_name, inbox_root, round_dir, tip_dir};

fn at(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
}

/// A round folder with a run record whose mtime is `finished`.
fn round(data: &Path, tip: &str, run: &str, finished: Option<SystemTime>) -> PathBuf {
    let dir = round_dir(&inbox_root(data), tip, run);
    let rec = dir.join("runs").join("g1");
    fs::create_dir_all(&rec).unwrap();
    fs::create_dir_all(dir.join("nodes").join("task").join("deliverables")).unwrap();
    let file = rec.join("run.json");
    fs::write(&file, "{}").unwrap();
    if let Some(t) = finished {
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }
    dir
}

fn system(t: NaiveDateTime) -> SystemTime {
    use chrono::TimeZone as _;
    chrono::Local.from_local_datetime(&t).unwrap().into()
}

struct Keep(HashSet<PathBuf>);

impl Keep {
    fn none() -> Self {
        Keep(HashSet::new())
    }
}

impl Live for Keep {
    fn in_use(&self, dir: &Path) -> bool {
        self.0.contains(dir)
    }
}

#[test]
fn rounds_older_than_the_keep_time_are_deleted() {
    let tmp = tempfile::tempdir().unwrap();
    let data = crate::paths::abs(tmp.path());
    let now = at("2026-11-30 12:00:00");
    // Started 20 days ago, finished 16 days ago: deleted.
    let old = round(
        &data,
        "t",
        "20261110-120000-aaaa",
        Some(system(at("2026-11-14 11:00:00"))),
    );
    // Started 20 days ago, finished 3 days ago (a long question): kept.
    let late = round(
        &data,
        "t",
        "20261110-130000-bbbb",
        Some(system(at("2026-11-27 12:00:00"))),
    );
    // Started 2 days ago: kept.
    let new = round(&data, "t", "20261128-120000-cccc", None);
    let report = prune(&data, now, KEEP_FOR, &Keep::none());
    assert!(!old.exists(), "{report:?}");
    assert!(late.is_dir() && new.is_dir());
    assert_eq!(report.deleted, [old]);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
}

#[test]
fn a_finish_time_in_the_future_or_before_the_start_counts_as_the_start() {
    let tmp = tempfile::tempdir().unwrap();
    let data = crate::paths::abs(tmp.path());
    let now = at("2026-11-30 12:00:00");
    // An agent pushed the record's time into the future: aged from the start.
    let pushed = round(
        &data,
        "t",
        "20261101-120000-aaaa",
        Some(system(at("2027-01-01 00:00:00"))),
    );
    let report = prune(&data, now, KEEP_FOR, &Keep::none());
    assert!(!pushed.exists(), "{report:?}");
}

#[test]
fn live_rounds_unknown_names_and_links_are_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let data = crate::paths::abs(tmp.path());
    let now = at("2026-11-30 12:00:00");
    let old = || Some(system(at("2026-11-01 12:00:00")));
    let live = round(&data, "t", "20261101-120000-aaaa", old());
    let keep = Keep([live.clone()].into_iter().collect());
    // Not a run id: not a folder the app made.
    let stranger = tip_dir(&inbox_root(&data), "t").join("my-notes");
    fs::create_dir_all(&stranger).unwrap();
    // A round holding a junction to a folder elsewhere: refused, the
    // folder elsewhere untouched.
    let linked = round(&data, "t", "20261101-130000-bbbb", old());
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep.md"), "keep").unwrap();
    dir_link(&elsewhere, &linked.join("nodes").join("j"));
    // A junction in place of a round folder: refused, never followed.
    let target = tmp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep.md"), "keep").unwrap();
    let junction = tip_dir(&inbox_root(&data), "t").join("20261101-140000-cccc");
    dir_link(&target, &junction);
    let report = prune(&data, now, KEEP_FOR, &keep);
    assert!(report.deleted.is_empty(), "{report:?}");
    assert!(live.is_dir() && stranger.is_dir() && linked.is_dir());
    assert!(elsewhere.join("keep.md").is_file() && target.join("keep.md").is_file());
    assert!(report
        .refused
        .iter()
        .any(|r| r.contains("is a link or junction")));
    assert_eq!(report.refused.len(), 2, "{:?}", report.refused);
}

#[test]
fn an_emptied_tip_folder_is_removed_and_legacy_folders_are_not_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let data = crate::paths::abs(tmp.path());
    let now = at("2026-11-30 12:00:00");
    let r = round(
        &data,
        "t",
        "20261101-120000-aaaa",
        Some(system(at("2026-11-01 13:00:00"))),
    );
    let legacy = inbox_root(&data).join(folder_name("t"));
    fs::create_dir_all(legacy.join("runs")).unwrap();
    let report = prune(&data, now, KEEP_FOR, &Keep::none());
    assert_eq!(report.deleted, std::slice::from_ref(&r));
    assert!(!r.parent().unwrap().exists(), "the empty tip folder goes");
    assert!(legacy.join("runs").is_dir(), "0.7.0 folders are left alone");
    // Nothing there at all: no error.
    let empty = tempfile::tempdir().unwrap();
    let report = prune(
        &crate::paths::abs(empty.path()),
        now,
        KEEP_FOR,
        &Keep::none(),
    );
    assert!(report.deleted.is_empty() && report.refused.is_empty());
}

/// A tip with a round starting or open is skipped as a whole (a starting
/// round creates its folder in the tip folder).
#[test]
fn a_tip_in_use_is_skipped_as_a_whole() {
    let tmp = tempfile::tempdir().unwrap();
    let data = crate::paths::abs(tmp.path());
    let now = at("2026-11-30 12:00:00");
    let old = round(
        &data,
        "t",
        "20261101-120000-aaaa",
        Some(system(at("2026-11-01 13:00:00"))),
    );
    let tip = old.parent().unwrap().to_path_buf();
    let keep = Keep([tip.clone()].into_iter().collect());
    let report = prune(&data, now, KEEP_FOR, &keep);
    assert!(report.deleted.is_empty() && old.is_dir(), "{report:?}");
}
