use super::*;

fn q(reply: &str) -> Outcome {
    classify(reply)
}

fn labels(o: &Outcome) -> Vec<&str> {
    o.choices.iter().map(|c| c.label.as_str()).collect()
}

#[test]
fn plain_report_is_done() {
    let o = q("I wrote the survey to deliverables/survey.md.\n\nIt covers 12 papers.");
    assert_eq!(o, Outcome::done());
}

#[test]
fn trailing_question_mark_is_a_question() {
    let o = q("I set up the project.\n\nWhich framework should I use?");
    assert_eq!(o.kind, OutcomeKind::Question);
    assert_eq!(o.question.as_deref(), Some("Which framework should I use?"));
    assert!(o.choices.is_empty());
}

#[test]
fn numbered_choices_after_the_question() {
    let o = q("Setup done.\nWhich framework should I use?\n1. vitest\n2. jest");
    assert_eq!(o.kind, OutcomeKind::Question);
    assert_eq!(labels(&o), ["vitest", "jest"]);
}

#[test]
fn lettered_and_bracketed_choices() {
    let o = q("Pick one:\na) keep the API\nb) break it");
    assert_eq!(labels(&o), ["keep the API", "break it"]);
    let o = q("How should I proceed?\n[A] rewrite\n[B] patch");
    assert_eq!(labels(&o), ["rewrite", "patch"]);
    let o = q("Options:\n**A)** fast path\n**B)** safe path");
    assert_eq!(o.kind, OutcomeKind::Question);
    assert_eq!(labels(&o), ["fast path", "safe path"]);
}

#[test]
fn bold_option_bullets_with_detail_lines() {
    let o = q("Two ways forward.\n\nWhich do you prefer?\n- **Option A**: inline cache\n  faster, more memory\n- **Option B**: no cache");
    assert_eq!(labels(&o), ["Option A: inline cache", "Option B: no cache"]);
    assert_eq!(o.choices[0].detail.as_deref(), Some("faster, more memory"));
}

#[test]
fn choices_listed_above_the_question() {
    let o = q("I found two candidates:\n1. tokio\n2. async-std\nShould I go with the first?");
    assert_eq!(o.question.as_deref(), Some("Should I go with the first?"));
    assert_eq!(labels(&o), ["tokio", "async-std"]);
}

#[test]
fn stems_without_question_mark() {
    for line in [
        "Let me know if you want the tests too.",
        "Please confirm the target directory.",
        "Would you like me to continue",
        "Your call: keep or drop the flag",
    ] {
        let o = q(&format!("Report written.\n{line}"));
        assert_eq!(o.kind, OutcomeKind::Question, "{line}");
    }
}

#[test]
fn statement_with_interrogative_word_is_not_a_question() {
    let o = q("Tests pass.\nWhich is why I removed the old fixture.");
    assert_eq!(o.kind, OutcomeKind::Done);
}

#[test]
fn offer_pauses() {
    let o = q("All three files are written.\n\nWant me to also write tests?");
    assert_eq!(o.kind, OutcomeKind::Question);
    assert_eq!(o.question.as_deref(), Some("Want me to also write tests?"));
}

#[test]
fn completion_marker_after_the_question_wins() {
    let o = q("Should I also bump the version? Not needed here.\n\nDone.");
    assert_eq!(o.kind, OutcomeKind::Done);
    let o = q("Do we need X? No.\n## Deliverables\n- a.md\n## Status: DONE");
    assert_eq!(o.kind, OutcomeKind::Done);
    let o = q("Is it fast? Yes.\nAll deliverables written to the folder.");
    assert_eq!(o.kind, OutcomeKind::Done);
}

#[test]
fn question_inside_code_block_does_not_count() {
    let o = q(
        "Here is the helper:\n```rust\nlet x = a?;\nfn ask() -> bool { which? }\n```\nIt compiles.",
    );
    assert_eq!(o.kind, OutcomeKind::Done);
}

#[test]
fn question_in_deliverables_list_does_not_count() {
    let o = q("Finished.\n## Deliverables\n- notes.md (what is open?)\n- faq.md");
    assert_eq!(o.kind, OutcomeKind::Done);
}

#[test]
fn quoted_or_inline_code_questions_do_not_count() {
    let o = q("The page now asks \"Are you sure?\" before deleting.");
    assert_eq!(o.kind, OutcomeKind::Done);
    let o = q("Renamed `is_ready?` to `ready`.");
    assert_eq!(o.kind, OutcomeKind::Done);
}

#[test]
fn url_query_is_not_a_question() {
    let o = q("Docs are at https://example.com/search?q=vt100&page=2 now.");
    assert_eq!(o.kind, OutcomeKind::Done);
}

#[test]
fn early_question_outside_the_tail_does_not_count() {
    let body = (1..=10)
        .map(|i| format!("Step {i} done."))
        .collect::<Vec<_>>();
    let o = q(&format!("Why was it slow? A lock.\n{}", body.join("\n")));
    assert_eq!(o.kind, OutcomeKind::Done);
}

#[test]
fn blocked_status_with_question_is_a_question() {
    let o =
        q("Could not finish.\n## Status: BLOCKED\nMay I continue without web access? (yes / no)");
    assert_eq!(o.kind, OutcomeKind::Question);
    assert!(o.question.unwrap().starts_with("May I continue"));
}

#[test]
fn markdown_wrapped_question_and_long_labels() {
    let long = "x".repeat(200);
    let o = q(&format!("**Which one?**\n1. {long}\n2. short"));
    assert_eq!(o.question.as_deref(), Some("Which one?"));
    assert_eq!(o.choices[0].label.chars().count(), LABEL_MAX);
    assert_eq!(o.choices[1].label, "short");
}

#[test]
fn empty_reply_is_done_and_excerpt_is_capped() {
    assert_eq!(q("").kind, OutcomeKind::Done);
    assert_eq!(q("   \n\n").kind, OutcomeKind::Done);
    let long = "y".repeat(EXCERPT_MAX + 50);
    assert_eq!(excerpt(&long).chars().count(), EXCERPT_MAX);
}
