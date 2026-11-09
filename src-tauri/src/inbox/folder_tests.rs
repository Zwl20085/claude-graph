use std::cell::Cell;
use std::collections::HashSet;

use chrono::NaiveDate;

use super::*;
use crate::inbox::block::Outcome;
use crate::inbox::hash::body_hash;
use crate::inbox::ops::{self, Claimant, Known, OpError};
use crate::inbox::tip::parse;
use crate::inbox::writeback::{claim_file, finish_file};

fn who(run: &str) -> Claimant<'_> {
    Claimant {
        host: "DESK",
        run,
        at: NaiveDate::from_ymd_opt(2026, 11, 9)
            .unwrap()
            .and_hms_opt(7, 31, 0)
            .unwrap(),
    }
}

/// No live and no recorded run ids.
fn known() -> Known<'static> {
    let empty: &'static HashSet<String> = Box::leak(Box::new(HashSet::new()));
    Known {
        live: empty,
        claimed: empty,
    }
}

fn temps(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect()
}

#[test]
fn lists_only_markdown_tips_directly_in_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    for name in ["b.md", "a.MD", ".hidden.md", "~lock.md", "notes.txt", ".md"] {
        fs::write(d.join(name), "x").unwrap();
    }
    fs::create_dir_all(d.join("agent").join("x")).unwrap();
    fs::write(d.join("agent").join("x").join("result.md"), "x").unwrap();
    fs::create_dir(d.join("dir.md")).unwrap();
    let ids: Vec<String> = list(d).unwrap().into_iter().map(|e| e.id).collect();
    assert_eq!(ids, ["a", "b"]);
}

#[test]
fn stamp_is_mtime_and_length() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    fs::write(&f, "12345").unwrap();
    let s = stamp(&f).unwrap();
    assert!(s.ends_with("-5"), "{s}");
    assert!(s.split('-').next().unwrap().parse::<u128>().unwrap() > 0);
}

#[test]
fn a_concurrent_edit_is_reread_and_the_patch_reapplied() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    fs::write(&f, "---\nkind: agent\n---\nDo x\n").unwrap();
    let calls = Cell::new(0);
    let out = update_with(
        &f,
        |text| {
            calls.set(calls.get() + 1);
            ops::claim("t", text, &who("r1"), &known()).map(|c| (c.text.clone(), c))
        },
        &|attempt| {
            if attempt == 0 {
                // The board saves an edit while the runner writes.
                fs::write(&f, "---\nkind: agent\ncolor: red\n---\nDo x, and y\n").unwrap();
            }
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(out.seen_body, "Do x, and y\n");
    let text = fs::read_to_string(&f).unwrap();
    assert!(
        text.contains("color: red") && text.contains("state: running"),
        "{text}"
    );
    assert!(temps(tmp.path()).is_empty());
}

#[test]
fn a_stamp_that_keeps_changing_gives_up_after_five_tries() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    fs::write(&f, "---\nkind: agent\n---\nx\n").unwrap();
    let n = Cell::new(0);
    let r = update_with(
        &f,
        |text| ops::claim("t", text, &who("r1"), &known()).map(|c| (c.text.clone(), c)),
        &|_| {
            n.set(n.get() + 1);
            fs::write(
                &f,
                format!("---\nkind: agent\n---\nx{}\n", "y".repeat(n.get())),
            )
            .unwrap();
        },
    );
    assert!(matches!(r, Err(WriteError::Conflict)));
    assert_eq!(n.get(), STAMP_TRIES);
    assert!(temps(tmp.path()).is_empty());
}

#[test]
fn edit_between_claim_and_finish_keeps_the_tip_queued() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    fs::write(&f, "---\nkind: agent\ntitle: T\n---\nDo x\n").unwrap();
    let c = claim_file(&f, "t", &who("r1"), &known()).unwrap();
    let claimed = fs::read_to_string(&f).unwrap();
    fs::write(&f, claimed.replace("Do x\n", "Do x\nAlso y.\n")).unwrap();
    let done = Outcome::Done { reply: "ok".into() };
    let fin = finish_file(&f, "t", &who("r1"), &c.seen_body, &done).unwrap();
    assert!(fin.edited_meanwhile);
    let tip = parse("t", &fs::read_to_string(&f).unwrap()).unwrap();
    assert_eq!(
        tip.agent.as_ref().unwrap().hash(),
        Some(body_hash("Do x\n"))
    );
    assert!(tip.body.starts_with("Do x\nAlso y.\n\n<!-- agent:begin"));
}

#[test]
fn a_deleted_tip_is_never_recreated() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    fs::write(&f, "---\nkind: agent\n---\nDo x\n").unwrap();
    let c = claim_file(&f, "t", &who("r1"), &known()).unwrap();
    fs::remove_file(&f).unwrap();
    let err = finish_file(
        &f,
        "t",
        &who("r1"),
        &c.seen_body,
        &Outcome::Done { reply: "ok".into() },
    )
    .unwrap_err();
    assert!(err.is_gone(), "{}", err.describe());
    assert!(!f.exists());
    assert!(temps(tmp.path()).is_empty());
    // Deleted between the temp write and the stamp check: also dropped.
    fs::write(&f, "---\nkind: agent\n---\nDo x\n").unwrap();
    let r = update_with(
        &f,
        |text| ops::claim("t", text, &who("r2"), &known()).map(|c| (c.text.clone(), c)),
        &|_| fs::remove_file(&f).unwrap(),
    );
    assert!(matches!(r, Err(WriteError::Gone)));
    assert!(!f.exists());
    assert!(temps(tmp.path()).is_empty());
}

#[test]
fn a_non_agent_tip_is_never_opened_for_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("task.md");
    let text = "---\ntitle: Buy milk\nkind: task\n---\nbody\n";
    fs::write(&f, text).unwrap();
    // Read-only: any write attempt on it would fail with an i/o error instead.
    let mut perms = fs::metadata(&f).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&f, perms.clone()).unwrap();
    let before = stamp(&f).unwrap();
    let c = claim_file(&f, "task", &who("r1"), &known()).unwrap_err();
    assert!(
        matches!(c.error, WriteError::Refused(OpError::NotAgent)),
        "{}",
        c.describe()
    );
    let fin = finish_file(
        &f,
        "task",
        &who("r1"),
        "body\n",
        &Outcome::Failed { reason: "x".into() },
    )
    .unwrap_err();
    assert!(matches!(fin.error, WriteError::Refused(OpError::NotAgent)));
    assert_eq!(stamp(&f).unwrap(), before);
    assert_eq!(fs::read_to_string(&f).unwrap(), text);
    assert!(temps(tmp.path()).is_empty());
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&f, perms).unwrap();
}

#[test]
fn non_utf8_files_are_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("bin.md");
    fs::write(&f, [0xff, 0xfe, 0x00]).unwrap();
    let e = claim_file(&f, "bin", &who("r1"), &known()).unwrap_err();
    assert!(matches!(e.error, WriteError::NotUtf8));
    assert_eq!(fs::read(&f).unwrap(), [0xff, 0xfe, 0x00]);
}

#[test]
fn the_frontmatter_read_stops_at_the_closing_fence() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    // The body is not UTF-8: it is never decoded.
    let mut bytes = b"\xef\xbb\xbf---\r\nkind: task\r\n---\r\n".to_vec();
    bytes.extend_from_slice(&[0xff; 100_000]);
    fs::write(&f, &bytes).unwrap();
    assert_eq!(
        read_frontmatter(&f).unwrap(),
        Prefix::Frontmatter("\u{feff}---\r\nkind: task\r\n---\r\n".into())
    );
    fs::write(&f, "---\nkind: task\n---").unwrap();
    assert_eq!(
        read_frontmatter(&f).unwrap(),
        Prefix::Frontmatter("---\nkind: task\n---".into())
    );
    for text in [
        "no fence\n---\n",
        "---\nkind: task\n",
        "--- \nkind: task\n---\n",
        "---",
    ] {
        fs::write(&f, text).unwrap();
        assert_eq!(
            read_frontmatter(&f).unwrap(),
            Prefix::NoFrontmatter,
            "{text:?}"
        );
    }
    let long = format!("---\n{}", "k: v\n".repeat(FRONTMATTER_CAP / 5 + 10));
    fs::write(&f, long).unwrap();
    assert_eq!(read_frontmatter(&f).unwrap(), Prefix::TooLong);
    fs::write(&f, b"---\nkind: \xff\n---\nx").unwrap();
    assert_eq!(read_frontmatter(&f).unwrap(), Prefix::NotUtf8);
}

#[test]
fn a_read_only_agent_tip_is_reported_not_retried_forever() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    fs::write(&f, "---\nkind: agent\n---\nDo x\n").unwrap();
    let mut perms = fs::metadata(&f).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&f, perms.clone()).unwrap();
    let started = std::time::Instant::now();
    let e = claim_file(&f, "t", &who("r1"), &known()).unwrap_err();
    assert!(matches!(e.error, WriteError::ReadOnly), "{}", e.describe());
    assert!(e.describe().contains("tip file is read-only"));
    assert!(e.is_retryable());
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(temps(tmp.path()).is_empty());
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&f, perms).unwrap();
}

#[test]
fn a_tip_made_read_only_before_the_rename_is_not_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.md");
    let text = "---\nkind: agent\n---\nDo x\n";
    fs::write(&f, text).unwrap();
    let r = update_with(
        &f,
        |t| ops::claim("t", t, &who("r1"), &known()).map(|c| (c.text.clone(), c)),
        &|_| {
            let mut p = fs::metadata(&f).unwrap().permissions();
            p.set_readonly(true);
            fs::set_permissions(&f, p).unwrap();
        },
    );
    assert!(matches!(r, Err(WriteError::ReadOnly)));
    assert_eq!(fs::read_to_string(&f).unwrap(), text);
    assert!(temps(tmp.path()).is_empty());
    let mut p = fs::metadata(&f).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    p.set_readonly(false);
    fs::set_permissions(&f, p).unwrap();
}
