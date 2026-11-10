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
    let r = copy_round(&dest, Some(&reply), Some("the prompt"), &src);
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
    let r = copy_round(&dest, Some(&reply), None, &src);
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
    let r = copy_round(&dest, None, None, &src);
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
    let r = copy_round(
        &dest,
        Some(&tmp.path().join("gone.md")),
        None,
        &tmp.path().join("none"),
    );
    assert!(r.copied.is_empty());
    assert_eq!(r.refused, ["result.md: the node left no reply file"]);
}
