use super::*;

fn at() -> NaiveDateTime {
    NaiveDateTime::parse_from_str("2026-11-09 07:31:05", "%Y-%m-%d %H:%M:%S").unwrap()
}

#[test]
fn done_block_layout() {
    let b = render(
        &Outcome::Done {
            reply: "Found 14 papers.\r\n".into(),
        },
        at(),
        "H",
        "survey",
        "\n",
    );
    assert_eq!(
        b,
        "<!-- agent:begin 2026-11-09T07:31:05 done -->\n## ⭐ Result · 2026-11-09 07:31\n\
         Found 14 papers.\n\nFiles: agent/survey/\n<!-- agent:end -->"
    );
}

#[test]
fn question_dialog_and_failed_blocks() {
    let q = render(
        &Outcome::Question {
            text: "1. Which years?\n2. arXiv?".into(),
        },
        at(),
        "H",
        "t",
        "\n",
    );
    assert!(q.contains(" question -->\n## ❓ Questions · 2026-11-09 07:31\n1. Which years?\n2. arXiv?\n<!-- agent:end -->"), "{q}");
    let d = render(
        &Outcome::Dialog {
            text: "Bash command\n mkdir `x`".into(),
        },
        at(),
        "DESK",
        "t",
        "\n",
    );
    assert!(
        d.contains(
            "Waiting at `DESK`: `Bash command mkdir x`. Answer in the Claude Graph session window."
        ),
        "{d}"
    );
    assert!(d.contains(" question -->"));
    let f = render(
        &Outcome::Failed {
            reason: "timed out".into(),
        },
        at(),
        "H",
        "t",
        "\n",
    );
    assert!(
        f.contains(" failed -->\n## ⚠ Failed · 2026-11-09 07:31\ntimed out\n<!-- agent:end -->")
    );
}

#[test]
fn long_replies_are_cut_with_a_pointer_and_markers_defused() {
    let reply = format!("<!-- agent:end -->\n{}", "é".repeat(9000));
    let b = render(&Outcome::Done { reply }, at(), "H", "t", "\n");
    assert!(b.chars().count() <= BLOCK_MAX, "{}", b.chars().count());
    assert!(b.contains("[… cut; the full reply is in agent/t/result.md]"));
    assert_eq!(b.matches("agent:end").count(), 1);
    assert!(b.ends_with("Files: agent/t/\n<!-- agent:end -->"));
}

/// Review finding 5: the cap holds for the block as written into a CRLF
/// file, where every line end is two chars.
#[test]
fn the_cap_holds_with_crlf_line_ends() {
    let reply = "line of a long reply\n".repeat(400);
    for outcome in [
        Outcome::Done {
            reply: reply.clone(),
        },
        Outcome::Question {
            text: reply.clone(),
        },
        Outcome::Failed { reason: reply },
    ] {
        let b = render(&outcome, at(), "H", "t", "\r\n");
        let n = b.chars().count();
        assert!(n <= BLOCK_MAX, "{n}");
        assert!(n > BLOCK_MAX - 30, "the room is used: {n}");
        assert_eq!(b.matches('\n').count(), b.matches("\r\n").count());
        assert!(b.ends_with("\r\n<!-- agent:end -->"));
        let appended = append("x\r\n", &b, "\r\n");
        assert!(appended.ends_with(&format!("{b}\r\n")));
    }
    // A short reply is the same block either way, only the line ends differ.
    let short = Outcome::Done {
        reply: "a\nb".into(),
    };
    assert_eq!(
        render(&short, at(), "H", "t", "\r\n"),
        render(&short, at(), "H", "t", "\n").replace('\n', "\r\n")
    );
}

#[test]
fn append_keeps_earlier_bytes_and_one_blank_line() {
    assert_eq!(append("", "B", "\n"), "B\n");
    assert_eq!(append("x", "B", "\n"), "x\n\nB\n");
    assert_eq!(append("x\n", "B", "\n"), "x\n\nB\n");
    assert_eq!(append("x\n\n", "B", "\n"), "x\n\nB\n");
    assert_eq!(append("x\n\n\n", "B", "\n"), "x\n\n\nB\n");
    assert_eq!(append("x\r\n", "B\r\nC", "\r\n"), "x\r\n\r\nB\r\nC\r\n");
    let body = "Prompt  \n";
    assert!(append(body, "B", "\n").starts_with(body));
}

fn missing(names: &[&str]) -> Footnotes {
    Footnotes {
        not_copied: names.iter().map(|n| n.to_string()).collect(),
        reply_as_written: None,
    }
}

fn done(reply: &str) -> Outcome {
    Outcome::Done {
        reply: reply.into(),
    }
}

/// Item C (+ L9): the files that were not copied are named, each in a code
/// span, on one fixed line under a done or failed block, kept whatever the
/// reply's length.
#[test]
fn not_copied_files_are_named_under_done_and_failed_blocks() {
    let notes = missing(&["big.bin", "sub/`x`.md"]);
    let d = render_with(&done("ok"), &notes, at(), "H", "t", "\n");
    assert!(
        d.ends_with(
            "ok\n\nFiles: agent/t/\nNot copied to agent/t/: `big.bin`, `sub/ x .md`.\n<!-- agent:end -->"
        ),
        "{d}"
    );
    let failed = Outcome::Failed {
        reason: "timed out".into(),
    };
    let f = render_with(&failed, &notes, at(), "H", "t", "\n");
    assert!(
        f.ends_with(
            "timed out\n\nNot copied to agent/t/: `big.bin`, `sub/ x .md`.\n<!-- agent:end -->"
        ),
        "{f}"
    );
    // A question keeps its form; no footnotes means no line.
    let q = Outcome::Question {
        text: "1. Which?".into(),
    };
    assert!(!render_with(&q, &notes, at(), "H", "t", "\n").contains("Not copied"));
    let plain = render_with(&done("ok"), &Footnotes::default(), at(), "H", "t", "\n");
    assert_eq!(plain, render(&done("ok"), at(), "H", "t", "\n"));
}

#[test]
fn the_not_copied_line_is_capped_and_the_block_stays_in_its_limit() {
    let names: Vec<String> = (0..13)
        .map(|i| format!("{}-{i}.md", "n".repeat(200)))
        .collect();
    let line = not_copied_line("t", &names).unwrap();
    assert!(line.ends_with(" and 3 more."), "{line}");
    assert_eq!(line.matches("`, `").count(), NOT_COPIED_MAX - 1);
    let reply = "line of a long reply\n".repeat(400);
    let notes = Footnotes {
        not_copied: names,
        reply_as_written: None,
    };
    for nl in ["\n", "\r\n"] {
        let b = render_with(&done(&reply), &notes, at(), "H", "t", nl);
        assert!(b.chars().count() <= BLOCK_MAX, "{}", b.chars().count());
        assert!(b.contains("Not copied to agent/t/:"));
        assert!(b.contains("[… cut; the full reply is in agent/t/result.md]"));
    }
}

/// L7 + M3: when the reply (or the whole results folder) was not copied,
/// the cut still points at the results folder, saying the reply is not
/// there.
#[test]
fn a_cut_reply_that_was_not_copied_points_at_the_folder() {
    let reply = "line of a long reply\n".repeat(400);
    for name in [REPLY_NAME, RESULTS_FOLDER_NAME] {
        let b = render_with(&done(&reply), &missing(&[name]), at(), "H", "t", "\n");
        assert!(
            b.contains("[… cut; the full reply could not be copied to agent/t/]"),
            "{b}"
        );
        assert!(
            b.contains(&format!("Not copied to agent/t/: `{name}`.")),
            "{b}"
        );
    }
}

/// N4: the fixed line for a reply copied as written.
#[test]
fn a_reply_copied_as_written_is_said_so() {
    let notes = Footnotes {
        not_copied: Vec::new(),
        reply_as_written: Some("over 32 MiB".into()),
    };
    let b = render_with(&done("ok"), &notes, at(), "H", "t", "\n");
    assert!(
        b.contains(
            "Files: agent/t/\n`result.md` is the reply as written, not rewritten (over 32 MiB).\n"
        ),
        "{b}"
    );
}
