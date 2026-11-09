//! When the runner handles a tip (docs/AGENT-PROTOCOL.md section 6) and what
//! the board shows for it (section 5). Pure functions of the parsed tip.

use std::collections::HashSet;

use super::hash::{body_hash, normalise};
use super::tip::{AgentState, Tip, Version};

/// The decision for one tip at one scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run it; the reason is shown in the UI.
    Eligible(&'static str),
    /// Leave it alone, with why.
    Skip(String),
}

impl Decision {
    pub fn is_eligible(&self) -> bool {
        matches!(self, Decision::Eligible(_))
    }
}

fn skip(why: impl Into<String>) -> Decision {
    Decision::Skip(why.into())
}

/// What this runner knows when it decides.
#[derive(Debug, Clone, Copy)]
pub struct Runner<'a> {
    /// This machine as written to `agent.host`.
    pub host: &'a str,
    /// Run ids this runner has alive right now.
    pub live: &'a HashSet<String>,
    /// Run ids this machine recorded when it claimed (persisted, bounded).
    pub claimed: &'a HashSet<String>,
}

/// Skip reason of a `running` claim of this host that this machine never recorded
/// (for example a OneDrive conflict copy `name-HOST.md` of a running tip).
pub const UNKNOWN_RUN: &str = "claimed by an unknown run of this machine";

/// Section 6 rules 1 to 3, first failing rule wins. A stale claim of this
/// host is re-run only for a run id this machine recorded (protocol section 9).
pub fn decide(tip: &Tip, me: &Runner<'_>) -> Decision {
    if !tip.is_agent() {
        return skip("not an agent tip");
    }
    if tip.status.as_deref() == Some("done") {
        return skip("marked done on the board");
    }
    let agent = tip.agent.clone().unwrap_or_default();
    match agent.version() {
        Version::Absent | Version::One => {}
        Version::Newer => return skip("agent.v is a newer protocol version"),
        Version::Other => return skip("agent.v is not the number 1"),
    }
    if tip.title.trim().is_empty() && normalise(&tip.body).is_empty() {
        return skip("title and body are empty");
    }
    let Some(state_text) = agent.state_text() else {
        return Decision::Eligible("new task");
    };
    match AgentState::parse(&state_text) {
        Some(AgentState::Done | AgentState::Question | AgentState::Failed) => {
            if agent.hash().as_deref() == Some(body_hash(&tip.body).as_str()) {
                skip(format!("unchanged since {state_text}"))
            } else {
                Decision::Eligible("the text changed")
            }
        }
        Some(AgentState::Running) => {
            let ours = agent.host().as_deref() == Some(me.host);
            let run = agent.run();
            let live = run.as_ref().is_some_and(|r| me.live.contains(r));
            let recorded = run.as_ref().is_some_and(|r| me.claimed.contains(r));
            match (ours, live, recorded) {
                (true, true, _) => skip("running here"),
                (true, false, true) => Decision::Eligible("stale claim of this machine"),
                (true, false, false) => skip(UNKNOWN_RUN),
                (false, _, _) => skip(format!(
                    "running on {}",
                    agent.host().unwrap_or_else(|| "another machine".into())
                )),
            }
        }
        None => skip(format!("unknown agent.state {state_text:?}")),
    }
}

/// Section 5 display state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    None,
    Running,
    Queued,
    Done,
    Question,
    Failed,
}

pub fn display(tip: &Tip) -> Display {
    if !tip.is_agent() {
        return Display::None;
    }
    let agent = tip.agent.clone().unwrap_or_default();
    if agent.version() == Version::Newer {
        return Display::None;
    }
    let state = agent.state_text();
    if state.as_deref() == Some("running") {
        return Display::Running;
    }
    if state.is_none() || agent.hash().as_deref() != Some(body_hash(&tip.body).as_str()) {
        return Display::Queued;
    }
    match state.as_deref() {
        Some("done") => Display::Done,
        Some("question") => Display::Question,
        Some("failed") => Display::Failed,
        _ => Display::Queued,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::tip::parse;

    const HOST: &str = "DESK";

    fn tip(agent: &str, extra: &str, body: &str) -> Tip {
        parse(
            "t",
            &format!("---\ntitle: T\nkind: agent\n{extra}{agent}---\n{body}"),
        )
        .unwrap()
    }

    fn agent(state: &str, hash: &str, host: &str, run: &str) -> String {
        format!("agent:\n  v: 1\n  state: {state}\n  hash: {hash}\n  host: {host}\n  run: {run}\n")
    }

    fn dec(tip: &Tip) -> Decision {
        decide_with(tip, &HashSet::new(), &HashSet::new())
    }

    fn decide_with(tip: &Tip, live: &HashSet<String>, claimed: &HashSet<String>) -> Decision {
        decide(
            tip,
            &Runner {
                host: HOST,
                live,
                claimed,
            },
        )
    }

    #[test]
    fn new_and_changed_tips_are_eligible() {
        assert_eq!(dec(&tip("", "", "Do x\n")), Decision::Eligible("new task"));
        let h = body_hash("Do x\n");
        for state in ["done", "question", "failed"] {
            let unchanged = tip(&agent(state, &h, HOST, "r1"), "", "Do x\n");
            assert!(!dec(&unchanged).is_eligible(), "{state}");
            let changed = tip(&agent(state, &h, HOST, "r1"), "", "Do x\n\nmore\n");
            assert!(dec(&changed).is_eligible(), "{state}");
            // Whitespace-only edits do not change the hash.
            let ws = tip(&agent(state, &h, HOST, "r1"), "", "Do x  \r\n\r\n");
            assert!(!dec(&ws).is_eligible(), "{state}");
        }
        let no_hash = tip("agent:\n  state: done\n", "", "Do x\n");
        assert!(dec(&no_hash).is_eligible());
    }

    #[test]
    fn rule_one_and_two_exclusions() {
        let task = parse("t", "---\ntitle: T\nkind: task\n---\nx").unwrap();
        assert!(!dec(&task).is_eligible());
        assert!(!dec(&tip("", "status: done\n", "x")).is_eligible());
        assert!(dec(&tip("", "status: open\n", "x")).is_eligible());
        assert!(!dec(&tip("agent:\n  v: 2\n", "", "x")).is_eligible());
        assert!(!dec(&tip("agent:\n  v: one\n", "", "x")).is_eligible());
        for v in ["2", "2.0", "'1'", "'2'", "0"] {
            let t = tip(&format!("agent:\n  v: {v}\n"), "", "x");
            assert!(!dec(&t).is_eligible(), "v {v}");
        }
        assert!(dec(&tip("agent:\n  v: 1.0\n", "", "x")).is_eligible());
        let empty = parse("t", "---\nkind: agent\ntitle: '  '\n---\n \n").unwrap();
        assert!(!dec(&empty).is_eligible());
        let title_only = parse("t", "---\nkind: agent\ntitle: Do it\n---\n").unwrap();
        assert!(dec(&title_only).is_eligible());
        let unknown = tip("agent:\n  state: paused\n", "", "x");
        assert!(!dec(&unknown).is_eligible());
    }

    #[test]
    fn running_claims() {
        let h = body_hash("x");
        let theirs = tip(&agent("running", &h, "OTHER", "r1"), "", "x");
        assert_eq!(dec(&theirs), Decision::Skip("running on OTHER".into()));
        let ours = tip(&agent("running", &h, HOST, "r1"), "", "x");
        // Not recorded by this machine (a conflict copy, a hand edit): skipped.
        assert_eq!(dec(&ours), Decision::Skip(UNKNOWN_RUN.into()));
        let r1: HashSet<String> = ["r1".to_string()].into();
        assert_eq!(
            decide_with(&ours, &HashSet::new(), &r1),
            Decision::Eligible("stale claim of this machine")
        );
        assert_eq!(
            decide_with(&ours, &r1, &r1),
            Decision::Skip("running here".into())
        );
        let other_run: HashSet<String> = ["r2".to_string()].into();
        assert!(!decide_with(&ours, &HashSet::new(), &other_run).is_eligible());
    }

    #[test]
    fn display_states() {
        let h = body_hash("x");
        assert_eq!(display(&tip("", "", "x")), Display::Queued);
        assert_eq!(
            display(&tip(&agent("done", &h, HOST, "r"), "", "x")),
            Display::Done
        );
        assert_eq!(
            display(&tip(&agent("done", &h, HOST, "r"), "", "x y")),
            Display::Queued
        );
        assert_eq!(
            display(&tip(&agent("question", &h, HOST, "r"), "", "x")),
            Display::Question
        );
        assert_eq!(
            display(&tip(&agent("failed", &h, HOST, "r"), "", "x")),
            Display::Failed
        );
        assert_eq!(
            display(&tip(&agent("running", "z", HOST, "r"), "", "x")),
            Display::Running
        );
        assert_eq!(
            display(&tip("agent:\n  v: 2\n  state: done\n", "", "x")),
            Display::None
        );
        assert_eq!(
            display(&tip("agent:\n  v: 2.0\n  state: done\n", "", "x")),
            Display::None
        );
        // A string version is never "above 1".
        assert_eq!(
            display(&tip("agent:\n  v: '2'\n", "", "x")),
            Display::Queued
        );
        assert_eq!(
            display(&tip(&agent("odd", &h, HOST, "r"), "", "x")),
            Display::Queued
        );
    }
}
