use super::*;
use crate::inbox::relink::ELSEWHERE;

#[test]
fn a_done_round_without_its_reply_names_it_a_failed_one_does_not() {
    let report = CopyReport {
        not_copied: vec!["big.bin".into()],
        reply_missing: true,
        ..CopyReport::default()
    };
    let done = Outcome::Done { reply: "x".into() };
    let failed = Outcome::Failed { reason: "x".into() };
    assert_eq!(not_copied(&done, &report), ["big.bin", "result.md"]);
    assert_eq!(not_copied(&failed, &report), ["big.bin"]);
}

const ROUND: &str = r"C:\Users\Owner\AppData\Roaming\claude-graph\inbox\rounds\t-1\r";

fn places() -> Places {
    let round = PathBuf::from(ROUND);
    Places {
        tip_id: "t".into(),
        deliverables: vec![round.join(r"nodes\task\deliverables")],
        private: vec![round],
        tips: Vec::new(),
        home: vec![PathBuf::from(r"C:\Users\Owner")],
        files: vec![CopiedFile::copied("a.md", "a.md")],
    }
}

/// H1: every text that goes into the tip is rewritten, the runner's too: a
/// dialog line quotes the agent's tool call, a failure reason an OS error.
#[test]
fn every_text_for_the_tip_is_rewritten() {
    let p = places();
    let deliv = format!(r"{ROUND}\nodes\task\deliverables\a.md");
    let cases = [
        (
            Outcome::Done {
                reply: format!("see {deliv}"),
            },
            Outcome::Done {
                reply: "see agent/t/a.md".into(),
            },
        ),
        (
            Outcome::Question {
                text: format!("1. Keep {deliv}?"),
            },
            Outcome::Question {
                text: "1. Keep agent/t/a.md?".into(),
            },
        ),
        (
            Outcome::Dialog {
                text: format!(r"Bash command: mkdir {ROUND}\x (Yes / No)"),
            },
            Outcome::Dialog {
                text: format!("Bash command: mkdir {ELSEWHERE} (Yes / No)"),
            },
        ),
        (
            Outcome::Failed {
                reason: format!(r"io: {ROUND}\runs\x: Access is denied"),
            },
            Outcome::Failed {
                reason: format!("io: {ELSEWHERE}: Access is denied"),
            },
        ),
        (
            Outcome::Failed {
                reason: r"working directory C:\Users\Owner\x does not exist".into(),
            },
            Outcome::Failed {
                reason: "working directory ~/x does not exist".into(),
            },
        ),
    ];
    for (input, want) in cases {
        assert_eq!(for_tip(&input, &p), want);
    }
}

/// M5: a huge text is cut before it is rewritten (the block keeps at most
/// 4000 chars of it anyway).
#[test]
fn a_huge_text_is_capped_before_rewriting() {
    let reply = format!("{}é", "x".repeat(TIP_TEXT_MAX));
    let Outcome::Done { reply: out } = for_tip(&Outcome::Done { reply }, &places()) else {
        panic!("done");
    };
    assert_eq!(out.len(), TIP_TEXT_MAX);
}
