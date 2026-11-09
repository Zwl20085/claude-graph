//! Claim and finish as pure text transformations (docs/AGENT-PROTOCOL.md
//! section 7). Each takes the file's current text and returns the new text,
//! or why it must not be written. Both re-check `kind: agent` on the text
//! they are given, so a tip that stopped being an agent tip is never written.

use std::collections::HashSet;

use chrono::NaiveDateTime;
use serde_yaml::Value;

use super::block::{self, Outcome};
use super::eligible::{decide, Decision, Runner};
use super::hash::{body_hash, normalise};
use super::patch::{set_agent, with_values};
use super::split::split;
use super::tip::{parse, AgentState, Tip, PROTOCOL_V};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    /// The text is not a readable tip.
    Unreadable(String),
    /// Not (any more) an agent tip.
    NotAgent,
    /// Claim: the tip is not eligible any more (the reason).
    NotEligible(String),
    /// Finish: the claim is not ours any more (another run or host took it).
    NotOurs(String),
    /// The patch could not be verified.
    Patch(String),
}

impl OpError {
    pub fn describe(&self) -> String {
        match self {
            OpError::Unreadable(e) => format!("unreadable: {e}"),
            OpError::NotAgent => "not an agent tip".into(),
            OpError::NotEligible(e) => format!("not eligible any more: {e}"),
            OpError::NotOurs(e) => format!("claim lost: {e}"),
            OpError::Patch(e) => e.clone(),
        }
    }
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn agent_tip(id: &str, text: &str) -> Result<Tip, OpError> {
    let tip = parse(id, text).map_err(|e| OpError::Unreadable(e.describe()))?;
    if !tip.is_agent() {
        return Err(OpError::NotAgent);
    }
    Ok(tip)
}

/// Who claims: this machine, the run id, the time.
#[derive(Debug, Clone)]
pub struct Claimant<'a> {
    pub host: &'a str,
    pub run: &'a str,
    pub at: NaiveDateTime,
}

/// The run ids a claim is decided with ([`Runner`] without the host).
#[derive(Debug, Clone, Copy)]
pub struct Known<'a> {
    pub live: &'a HashSet<String>,
    pub claimed: &'a HashSet<String>,
}

/// Claimed text and the seen body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub text: String,
    pub seen_body: String,
    pub title: String,
}

/// Claim: `state: running`, `host`, `run`, `at` (and `v: 1` when absent);
/// `hash` stays. The tip must still be eligible.
pub fn claim(
    id: &str,
    text: &str,
    who: &Claimant<'_>,
    known: &Known<'_>,
) -> Result<Claimed, OpError> {
    let tip = agent_tip(id, text)?;
    let me = Runner {
        host: who.host,
        live: known.live,
        claimed: known.claimed,
    };
    if let Decision::Skip(why) = decide(&tip, &me) {
        return Err(OpError::NotEligible(why));
    }
    let old = tip.agent.clone().unwrap_or_default().raw;
    let mut updates = Vec::new();
    if !old.contains_key("v") {
        updates.push(("v", Value::Number(PROTOCOL_V.into())));
    }
    updates.extend([
        ("state", s(AgentState::Running.as_str())),
        ("host", s(who.host)),
        ("run", s(who.run)),
        ("at", s(&block::stamp_time(who.at))),
    ]);
    let agent = with_values(&old, &updates);
    let out = set_agent(text, &agent, &tip.body).map_err(|e| OpError::Patch(e.describe()))?;
    Ok(Claimed {
        text: out,
        seen_body: tip.body,
        title: tip.title,
    })
}

/// A dry run of the claim patch: `Err` when the `agent` map of `text` cannot
/// be changed provably safely (scan reports such a tip instead of trying).
pub fn patchable(text: &str, tip: &Tip) -> Result<(), OpError> {
    let old = tip.agent.clone().unwrap_or_default().raw;
    let probe = with_values(
        &old,
        &[
            ("v", Value::Number(PROTOCOL_V.into())),
            ("state", s(AgentState::Running.as_str())),
            ("host", s("probe")),
            ("run", s("probe")),
            ("at", s("probe")),
            ("hash", s("probe")),
            ("results", s("probe")),
        ],
    );
    set_agent(text, &probe, &tip.body)
        .map(|_| ())
        .map_err(|e| OpError::Patch(e.describe()))
}

/// The tip is an agent tip whose claim is `who`'s host and run (it needs
/// only the frontmatter: `tip` may be parsed from it alone).
pub fn check_ours(tip: &Tip, who: &Claimant<'_>) -> Result<(), OpError> {
    if !tip.is_agent() {
        return Err(OpError::NotAgent);
    }
    let agent = tip.agent.clone().unwrap_or_default();
    let (host, run) = (agent.host(), agent.run());
    if host.as_deref() != Some(who.host) || run.as_deref() != Some(who.run) {
        return Err(OpError::NotOurs(format!(
            "now host {} run {}",
            host.unwrap_or_default(),
            run.unwrap_or_default()
        )));
    }
    Ok(())
}

/// Finished text and the body as left (the next round's expected body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub text: String,
    pub body: String,
    /// The user edited the body during the run (hash set to the seen body's).
    pub edited_meanwhile: bool,
}

/// Finish: append the outcome block; `state`, `at`, `results`, `hash` per
/// the rule of section 7.2. `expected_body`: the seen body, or the body this
/// run left at its last write. The claim must still be this host's run.
pub fn finish(
    id: &str,
    text: &str,
    who: &Claimant<'_>,
    expected_body: &str,
    outcome: &Outcome,
) -> Result<Finished, OpError> {
    let tip = agent_tip(id, text)?;
    check_ours(&tip, who)?;
    let agent = tip.agent.clone().unwrap_or_default();
    let newline = split(text).map_or("\n", |p| p.newline());
    let rendered = block::render(outcome, who.at, who.host, id, newline);
    let body = block::append(&tip.body, &rendered, newline);
    let edited = normalise(&tip.body) != normalise(expected_body);
    let hash = if edited {
        body_hash(expected_body)
    } else {
        body_hash(&body)
    };
    let updates = [
        ("state", s(outcome.state().as_str())),
        ("hash", s(&hash)),
        ("at", s(&block::stamp_time(who.at))),
        ("results", s(&block::results_rel(id))),
    ];
    let new_agent = with_values(&agent.raw, &updates);
    let out = set_agent(text, &new_agent, &body).map_err(|e| OpError::Patch(e.describe()))?;
    Ok(Finished {
        text: out,
        body,
        edited_meanwhile: edited,
    })
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
