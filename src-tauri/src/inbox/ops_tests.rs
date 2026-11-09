use super::*;
use crate::inbox::tip::frontmatter_of;
use chrono::NaiveDate;

/// No live and no recorded run ids.
fn known() -> Known<'static> {
    let empty: &'static HashSet<String> = Box::leak(Box::new(HashSet::new()));
    Known {
        live: empty,
        claimed: empty,
    }
}

fn runner() -> Runner<'static> {
    let k = known();
    Runner {
        host: "DESK",
        live: k.live,
        claimed: k.claimed,
    }
}

fn at(min: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 9)
        .unwrap()
        .and_hms_opt(7, min, 0)
        .unwrap()
}

fn who(run: &str, min: u32) -> Claimant<'_> {
    Claimant {
        host: "DESK",
        run,
        at: at(min),
    }
}

const RICH: &str = "\u{feff}---\r\n\
title: \"Survey: hairpin AC loss\"\r\n\
kind: agent\r\n\
color: '#ff8800'\r\n\
unknown_key: [1, two, {three: 3}]\r\n\
paper:\r\n  doi: \"10.1109/x\"\r\n  year: 2024\r\n  authors:\r\n    - A\r\n    - B\r\n\
status: open\r\n\
agent:\r\n  v: 1\r\n  custom_sub: kept\r\n\
order: 7   # a comment\r\n\
---\r\n\
Find three recent papers.  \r\n\r\nUse IEEE.\r\n";

fn non_agent_pairs(text: &str) -> Vec<(Value, Value)> {
    frontmatter_of(text)
        .unwrap()
        .into_iter()
        .filter(|(k, _)| k.as_str() != Some("agent"))
        .collect()
}

#[test]
fn claim_then_finish_keeps_every_other_key_and_the_body() {
    let c = claim("survey", RICH, &who("r1", 31), &known()).unwrap();
    assert_eq!(
        c.seen_body,
        "Find three recent papers.  \r\n\r\nUse IEEE.\r\n"
    );
    assert_eq!(non_agent_pairs(&c.text), non_agent_pairs(RICH));
    let fm = frontmatter_of(&c.text).unwrap();
    let agent = fm.get("agent").unwrap().as_mapping().unwrap();
    let keys: Vec<&str> = agent.keys().filter_map(Value::as_str).collect();
    assert_eq!(keys, ["v", "custom_sub", "state", "host", "run", "at"]);
    assert_eq!(
        agent.get("at").and_then(Value::as_str),
        Some("2026-11-09T07:31:00")
    );
    assert!(agent.get("hash").is_none(), "claim leaves hash as it was");
    // Every byte outside the agent lines is unchanged.
    assert!(c
        .text
        .starts_with("\u{feff}---\r\ntitle: \"Survey: hairpin AC loss\"\r\n"));
    assert!(c.text.contains("order: 7   # a comment\r\n---\r\n"));

    let done = Outcome::Done {
        reply: "Found 3.".into(),
    };
    let f = finish("survey", &c.text, &who("r1", 40), &c.seen_body, &done).unwrap();
    assert!(!f.edited_meanwhile);
    assert_eq!(non_agent_pairs(&f.text), non_agent_pairs(RICH));
    assert!(
        f.body.starts_with(&c.seen_body),
        "earlier body byte-identical"
    );
    assert!(f
        .body
        .ends_with("Files: agent/survey/\r\n<!-- agent:end -->\r\n"));
    let fm = frontmatter_of(&f.text).unwrap();
    let agent = fm.get("agent").unwrap().as_mapping().unwrap();
    assert_eq!(agent.get("state").and_then(Value::as_str), Some("done"));
    assert_eq!(
        agent.get("custom_sub").and_then(Value::as_str),
        Some("kept")
    );
    assert_eq!(
        agent.get("results").and_then(Value::as_str),
        Some("agent/survey/")
    );
    assert_eq!(
        agent.get("hash").and_then(Value::as_str),
        Some(body_hash(&f.body).as_str())
    );
    // The finished tip is not eligible again until the text changes.
    let tip = parse("survey", &f.text).unwrap();
    assert!(!decide(&tip, &runner()).is_eligible());
}

#[test]
fn an_edit_during_the_run_keeps_the_seen_hash() {
    let text = "---\nkind: agent\ntitle: T\n---\nDo x\n";
    let c = claim("t", text, &who("r1", 1), &known()).unwrap();
    let edited = c.text.replace("Do x\n", "Do x\n\nand y\n");
    let f = finish(
        "t",
        &edited,
        &who("r1", 2),
        &c.seen_body,
        &Outcome::Done { reply: "ok".into() },
    )
    .unwrap();
    assert!(f.edited_meanwhile);
    assert!(f.body.starts_with("Do x\n\nand y\n\n<!-- agent:begin"));
    let tip = parse("t", &f.text).unwrap();
    assert_eq!(
        tip.agent.as_ref().unwrap().hash(),
        Some(body_hash("Do x\n"))
    );
    assert!(decide(&tip, &runner()).is_eligible());
}

#[test]
fn guards() {
    let task = "---\nkind: task\n---\nx";
    assert_eq!(
        claim("t", task, &who("r", 1), &known()).unwrap_err(),
        OpError::NotAgent
    );
    assert_eq!(
        finish(
            "t",
            task,
            &who("r", 1),
            "x",
            &Outcome::Failed { reason: "x".into() }
        )
        .unwrap_err(),
        OpError::NotAgent
    );
    let text = "---\nkind: agent\n---\nx";
    let c = claim("t", text, &who("r1", 1), &known()).unwrap();
    // Already running here with a live run: not claimable.
    let live: HashSet<String> = ["r1".to_string()].into();
    assert!(matches!(
        claim(
            "t",
            &c.text,
            &who("r2", 2),
            &Known {
                live: &live,
                claimed: &live
            }
        ),
        Err(OpError::NotEligible(_))
    ));
    // A finish of another run id is dropped.
    assert!(matches!(
        finish(
            "t",
            &c.text,
            &who("r9", 2),
            "x",
            &Outcome::Done { reply: "".into() }
        ),
        Err(OpError::NotOurs(_))
    ));
    assert!(matches!(
        claim("t", "garbage", &who("r", 1), &known()),
        Err(OpError::Unreadable(_))
    ));
}

#[test]
fn question_then_done_appends_two_blocks() {
    let text = "---\nkind: agent\n---\nPrompt\n";
    let c = claim("t", text, &who("r1", 1), &known()).unwrap();
    let q = Outcome::Question {
        text: "Which year?".into(),
    };
    let f1 = finish("t", &c.text, &who("r1", 2), &c.seen_body, &q).unwrap();
    let f2 = finish(
        "t",
        &f1.text,
        &who("r1", 3),
        &f1.body,
        &Outcome::Done { reply: "ok".into() },
    )
    .unwrap();
    assert!(!f2.edited_meanwhile);
    assert_eq!(f2.body.matches("<!-- agent:end -->").count(), 2);
    assert!(f2.body.starts_with(&f1.body));
    let tip = parse("t", &f2.text).unwrap();
    assert_eq!(tip.agent.unwrap().state_text().as_deref(), Some("done"));
}
