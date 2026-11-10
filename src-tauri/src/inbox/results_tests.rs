use super::*;

/// [`copy_round`] with the reply copied as it is.
fn copy(dest: &Path, reply: Option<&Path>, prompt: Option<&str>, deliv: &Path) -> CopyReport {
    copy_round(dest, reply, prompt, deliv, &|t, _| t.to_string())
}

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
    let r = copy(&dest, Some(&reply), None, &src);
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
    let r = copy(&dest, None, None, &tmp.path().join("none"));
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
    let r = copy(&dest, Some(&reply), Some("round 2 prompt\n"), &src);
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
    let r = copy(&dest, None, None, &tmp.path().join("none"));
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
    let r = copy(&dest, None, Some("p"), &tmp.path().join("none"));
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
    let r = copy(&dest, Some(&reply), None, &src);
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
    let r = copy(&dest, None, None, &not_a_dir);
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
    let r = copy(&dest, None, None, &src);
    assert!(r.copied.is_empty() && r.refused[0].contains("symbolic link"));
    // A symlinked prompt.md target is not followed.
    std::os::unix::fs::symlink("/etc/passwd", dest.join(PROMPT_NAME)).unwrap();
    let r = copy(&dest, None, Some("p"), &tmp.path().join("none"));
    assert!(r.copied.is_empty() && r.refused[0].contains("not a regular file"));
}

/// S2: a hard link at a target name (`prompt.md`, `result.md`, a
/// deliverable's name) is replaced, never written through.
#[test]
fn hard_linked_targets_are_replaced_not_written_through() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, "precious").unwrap();
    for name in [PROMPT_NAME, REPLY_NAME, "survey.md"] {
        fs::hard_link(&outside, dest.join(name)).unwrap();
    }
    let src = tmp.path().join("deliv");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("survey.md"), "new survey").unwrap();
    let reply = tmp.path().join("reply.md");
    fs::write(&reply, "the reply").unwrap();
    let r = copy(&dest, Some(&reply), Some("the prompt"), &src);
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "precious");
    assert_eq!(
        fs::read_to_string(dest.join(PROMPT_NAME)).unwrap(),
        "the prompt"
    );
    assert_eq!(
        fs::read_to_string(dest.join(REPLY_NAME)).unwrap(),
        "the reply"
    );
    assert_eq!(
        fs::read_to_string(dest.join("survey.md")).unwrap(),
        "new survey"
    );
}

/// S2: a source with other names (a hard link to a file elsewhere) is not
/// copied, whether a deliverable or the reply.
#[test]
fn hard_linked_sources_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let secret = tmp.path().join("secret.txt");
    fs::write(&secret, "secret").unwrap();
    let src = tmp.path().join("deliv");
    fs::create_dir(&src).unwrap();
    fs::hard_link(&secret, src.join("loot.md")).unwrap();
    fs::write(src.join("ok.md"), "fine").unwrap();
    let reply = tmp.path().join("reply.md");
    fs::hard_link(&secret, &reply).unwrap();
    let r = copy(&dest, Some(&reply), None, &src);
    assert_eq!(r.copied, ["ok.md"]);
    assert!(
        r.refused
            .iter()
            .any(|x| x.starts_with("loot.md: a hard link")),
        "{:?}",
        r.refused
    );
    assert!(
        r.refused
            .iter()
            .any(|x| x.starts_with("result.md: a hard link")),
        "{:?}",
        r.refused
    );
    assert!(!dest.join("loot.md").exists() && !dest.join(REPLY_NAME).exists());
}

/// S2: a junction inside the deliverables folder is reported, never walked.
#[test]
fn a_junction_in_the_deliverables_is_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::write(elsewhere.join("x.md"), "x").unwrap();
    let src = tmp.path().join("deliv");
    fs::create_dir(&src).unwrap();
    crate::inbox::fsguard::tests::dir_link(&elsewhere, &src.join("j"));
    let r = copy(&dest, None, None, &src);
    assert!(r.copied.is_empty(), "{:?}", r.copied);
    assert!(
        r.refused[0].starts_with("j: symbolic link"),
        "{:?}",
        r.refused
    );
}

/// CI finding: a reply file that is missing is reported, never skipped
/// silently (a done tip would point at a result.md that never came).
#[test]
fn a_missing_reply_file_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let r = copy(
        &dest,
        Some(&tmp.path().join("gone.md")),
        None,
        &tmp.path().join("none"),
    );
    assert!(r.copied.is_empty());
    assert_eq!(r.refused, ["result.md: the node left no reply file"]);
}

/// Item C: what was not copied is named (relative names only) for the tip;
/// what was copied is listed per source for the path rewriting (item A).
#[test]
fn not_copied_files_are_named_and_copies_are_listed() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let secret = tmp.path().join("secret.txt");
    fs::write(&secret, "s").unwrap();
    let src = tmp.path().join("deliv");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("ok.md"), "fine").unwrap();
    fs::write(src.join("Result.md"), "own").unwrap();
    fs::hard_link(&secret, src.join("sub").join("loot.md")).unwrap();
    let r = copy(&dest, None, None, &src);
    assert_eq!(r.not_copied, ["sub/loot.md"]);
    assert_eq!(
        r.files,
        [
            CopiedFile::copied("Result.md", DELIVERABLE_RESULT_NAME),
            CopiedFile::copied("ok.md", "ok.md"),
            CopiedFile::refused("sub/loot.md"),
        ]
    );
    assert!(!r.reply_missing);
}

/// A missing reply is flagged, not named as "not copied" (a round that
/// failed early has none); a reply that could not be written is named.
#[test]
fn a_missing_reply_is_flagged_and_a_refused_one_named() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let none = tmp.path().join("none");
    let r = copy(&dest, Some(&tmp.path().join("gone.md")), None, &none);
    assert!(r.reply_missing && r.not_copied.is_empty());
    fs::create_dir(dest.join(REPLY_NAME)).unwrap();
    let reply = tmp.path().join("reply.md");
    fs::write(&reply, "x").unwrap();
    let r = copy(&dest, Some(&reply), None, &none);
    assert!(!r.reply_missing);
    assert_eq!(r.not_copied, [REPLY_NAME]);
}

/// Item A: the reply is written through the fix, which sees what was
/// copied before it.
#[test]
fn the_reply_is_written_through_the_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let src = tmp.path().join("deliv");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("a.md"), "a").unwrap();
    let reply = tmp.path().join("reply.md");
    fs::write(&reply, "see LOCAL/a.md").unwrap();
    let fix = |t: &str, r: &CopyReport| {
        assert_eq!(r.files, [CopiedFile::copied("a.md", "a.md")]);
        t.replace("LOCAL/", "agent/t/")
    };
    let r = copy_round(&dest, Some(&reply), None, &src, &fix);
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    assert_eq!(
        fs::read_to_string(dest.join(REPLY_NAME)).unwrap(),
        "see agent/t/a.md"
    );
}

/// M3: a results folder that cannot be resolved is named as not copied.
#[test]
fn an_unusable_results_folder_is_named_as_not_copied() {
    let tmp = tempfile::tempdir().unwrap();
    let r = copy(
        &tmp.path().join("gone"),
        None,
        Some("p"),
        &tmp.path().join("none"),
    );
    assert_eq!(r.not_copied, [RESULTS_FOLDER_NAME]);
}

/// L6: the agent's own `deliverable-result.md` and a renamed `result.md`
/// would get one name: the second is refused (and listed), never mixed up.
#[test]
fn colliding_copy_names_are_refused_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let src = tmp.path().join("deliv");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("deliverable-result.md"), "own").unwrap();
    fs::write(src.join("result.md"), "renamed").unwrap();
    let r = copy(&dest, None, None, &src);
    assert_eq!(
        r.files,
        [
            CopiedFile::copied("deliverable-result.md", DELIVERABLE_RESULT_NAME),
            CopiedFile::refused("result.md"),
        ]
    );
    assert_eq!(r.not_copied, ["result.md"]);
    assert_eq!(
        fs::read_to_string(dest.join(DELIVERABLE_RESULT_NAME)).unwrap(),
        "own"
    );
}

/// M5: the walk stops after the cap plus a margin, naming the rest once.
#[test]
fn the_walk_is_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let src = tmp.path().join("deliv");
    fs::create_dir(&src).unwrap();
    for i in 0..(WALK_MAX + 40) {
        fs::write(src.join(format!("f{i:04}.txt")), "x").unwrap();
    }
    let r = copy(&dest, None, None, &src);
    assert!(r.walk_cut);
    assert_eq!(r.files.len(), WALK_MAX);
    assert_eq!(r.copied.len(), MAX_FILES);
    assert!(r.not_copied.iter().any(|n| n == MORE_FILES_NAME));
    assert_eq!(r.not_copied.len(), WALK_MAX - MAX_FILES + 1);
}

/// LOW 9: every reply is rewritten, a large one too; a reply that is not
/// UTF-8 has its valid parts rewritten and its invalid bytes kept as they
/// are (never made lossy).
#[test]
fn large_and_binary_replies_are_rewritten_keeping_invalid_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let none = tmp.path().join("none");
    let rewrite = |t: &str, _: &CopyReport| t.replace("LOCAL", "agent");
    let binary = tmp.path().join("bin.md");
    let bytes = [
        b'L', b'O', b'C', b'A', b'L', 0xff, 0xfe, b'L', b'O', b'C', b'A', b'L', b'\n',
    ];
    fs::write(&binary, bytes).unwrap();
    let r = copy_round(&dest, Some(&binary), None, &none, &rewrite);
    assert!(r.refused.is_empty(), "{:?}", r.refused);
    let mut want = b"agent".to_vec();
    want.extend([0xff, 0xfe]);
    want.extend(b"agent\n");
    assert_eq!(fs::read(dest.join(REPLY_NAME)).unwrap(), want);
    let big = tmp.path().join("big.md");
    let text = format!("LOCAL{}LOCAL", "x".repeat(2 * 1024 * 1024));
    fs::write(&big, &text).unwrap();
    copy_round(&dest, Some(&big), None, &none, &rewrite);
    let out = fs::read_to_string(dest.join(REPLY_NAME)).unwrap();
    assert!(out.starts_with("agent") && out.ends_with("agent"));
}

/// N3: a non-UTF-8 reply is rewritten in one call (one `Places`), with an
/// invalid byte inside a path not hiding it; the invalid bytes outside the
/// rewritten paths come out byte-exact.
#[test]
fn a_non_utf8_reply_is_rewritten_once_across_invalid_bytes() {
    use crate::inbox::relink::{relink, Places};
    use std::cell::Cell;
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let none = tmp.path().join("none");
    let places = Places {
        tip_id: "t".into(),
        home: vec![std::path::PathBuf::from(r"C:\Users\Owner")],
        ..Places::default()
    };
    let calls = Cell::new(0);
    let fix = |t: &str, _: &CopyReport| {
        calls.set(calls.get() + 1);
        relink(t, &places)
    };
    let mut bytes = br"see C:\Users\Own".to_vec();
    bytes.push(0xff);
    bytes.extend(br"er\x and ");
    bytes.extend([0xfe, 0xc3]);
    bytes.extend(b" end");
    let reply = tmp.path().join("reply.md");
    fs::write(&reply, &bytes).unwrap();
    copy_round(&dest, Some(&reply), None, &none, &fix);
    assert_eq!(calls.get(), 1);
    let mut want = b"see ~/x and ".to_vec();
    want.extend([0xfe, 0xc3]);
    want.extend(b" end");
    assert_eq!(fs::read(dest.join(REPLY_NAME)).unwrap(), want);
}

/// N4: a reply above the rewrite ceiling is copied as written and flagged.
#[test]
fn a_reply_above_the_ceiling_is_copied_as_written() {
    let tmp = tempfile::tempdir().unwrap();
    let tips = tmp.path().join("tips");
    fs::create_dir(&tips).unwrap();
    let dest = results_dir(&tips, "t").unwrap();
    let none = tmp.path().join("none");
    let rewrite = |t: &str, _: &CopyReport| t.replace("LOCAL", "agent");
    let big = tmp.path().join("big.md");
    let text = format!("LOCAL{}", "x".repeat(REWRITE_MAX as usize));
    fs::write(&big, &text).unwrap();
    let r = copy_round(&dest, Some(&big), None, &none, &rewrite);
    assert_eq!(r.reply_as_written, Some(AS_WRITTEN_LARGE));
    assert_eq!(fs::read(dest.join(REPLY_NAME)).unwrap(), text.as_bytes());
    let small = tmp.path().join("small.md");
    fs::write(&small, "LOCAL").unwrap();
    let r = copy_round(&dest, Some(&small), None, &none, &rewrite);
    assert_eq!(r.reply_as_written, None);
}
