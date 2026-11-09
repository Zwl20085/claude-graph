//! Change only the `agent` map of a tip's frontmatter.
//!
//! The frontmatter text is not re-serialised: the lines of the top-level
//! `agent:` key are replaced (or the key is appended), so every other byte,
//! comment and quoting style stays. The result is parsed again and checked:
//! every other key keeps its value and position, and `agent` holds exactly
//! the new map. A file this cannot be proven for is not written.

use serde_yaml::{Mapping, Value};

use super::split::split;
use super::tip::{parse_mapping, AGENT_KEY};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    NoFrontmatter,
    BadYaml(String),
    /// The re-parsed frontmatter differs from what was intended.
    Unverified(String),
}

impl PatchError {
    pub fn describe(&self) -> String {
        match self {
            PatchError::NoFrontmatter => "no frontmatter".into(),
            PatchError::BadYaml(e) => format!("frontmatter is not YAML: {e}"),
            PatchError::Unverified(e) => format!("refusing to write, patch not verified: {e}"),
        }
    }
}

/// `old` with `updates` applied: existing keys keep their position, new keys
/// are appended in the order given.
pub fn with_values(old: &Mapping, updates: &[(&str, Value)]) -> Mapping {
    let mut out: Mapping = old
        .iter()
        .map(|(k, v)| {
            let new = updates
                .iter()
                .find(|(u, _)| k.as_str() == Some(*u))
                .map(|(_, nv)| nv.clone());
            (k.clone(), new.unwrap_or_else(|| v.clone()))
        })
        .collect();
    for (key, value) in updates {
        if !old.contains_key(*key) {
            out.insert(Value::String((*key).to_string()), value.clone());
        }
    }
    out
}

/// A top-level `agent:` key line (`agent:` then end, space or tab).
fn is_agent_key_line(line: &str) -> bool {
    line.strip_prefix("agent:")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '\r', '\n']))
}

fn is_continuation(line: &str) -> bool {
    line.starts_with([' ', '\t']) || line.trim().is_empty()
}

/// Byte range of the `agent` key's lines in `fm` (trailing blank lines excluded).
fn agent_span(fm: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    let mut start = None;
    let mut end = 0;
    for line in fm.split_inclusive('\n') {
        match start {
            None if is_agent_key_line(line) => {
                start = Some(offset);
                end = offset + line.len();
            }
            Some(_) if is_continuation(line) => {
                if !line.trim().is_empty() {
                    end = offset + line.len();
                }
            }
            Some(s) => return Some((s, end)),
            None => {}
        }
        offset += line.len();
    }
    start.map(|s| (s, end))
}

fn render_agent(agent: &Mapping, newline: &str) -> Result<String, PatchError> {
    let mut top = Mapping::new();
    top.insert(
        Value::String(AGENT_KEY.into()),
        Value::Mapping(agent.clone()),
    );
    let yaml = serde_yaml::to_string(&top).map_err(|e| PatchError::BadYaml(e.to_string()))?;
    let yaml = if yaml.ends_with('\n') {
        yaml
    } else {
        yaml + "\n"
    };
    Ok(if newline == "\n" {
        yaml
    } else {
        yaml.replace('\n', newline)
    })
}

fn keys_without_agent(m: &Mapping) -> Vec<&Value> {
    m.keys().filter(|k| k.as_str() != Some(AGENT_KEY)).collect()
}

fn verify(old: &Mapping, new: &Mapping, agent: &Mapping) -> Result<(), PatchError> {
    let bad = |what: &str| Err(PatchError::Unverified(what.to_string()));
    if keys_without_agent(old) != keys_without_agent(new) {
        return bad("other keys changed order");
    }
    for k in keys_without_agent(old) {
        if old.get(k) != new.get(k) {
            return bad("another key changed value");
        }
    }
    let pos = |m: &Mapping| m.keys().position(|k| k.as_str() == Some(AGENT_KEY));
    let expected_pos = pos(old).unwrap_or(old.len());
    if pos(new) != Some(expected_pos) {
        return bad("agent key moved");
    }
    if new.get(AGENT_KEY) != Some(&Value::Mapping(agent.clone())) {
        return bad("agent map differs");
    }
    Ok(())
}

/// The whole file with the frontmatter's `agent` map set to `agent` and the
/// body replaced by `body`.
pub fn set_agent(text: &str, agent: &Mapping, body: &str) -> Result<String, PatchError> {
    let parts = split(text).ok_or(PatchError::NoFrontmatter)?;
    let fm = parts.frontmatter;
    let old = parse_mapping(fm).map_err(|e| PatchError::BadYaml(e.describe()))?;
    let block = render_agent(agent, parts.newline())?;
    let new_fm = match agent_span(fm) {
        Some((s, e)) => format!("{}{block}{}", &fm[..s], &fm[e..]),
        None if fm.is_empty() || fm.ends_with('\n') => format!("{fm}{block}"),
        None => format!("{fm}{}{block}", parts.newline()),
    };
    let new = parse_mapping(&new_fm).map_err(|e| PatchError::Unverified(e.describe()))?;
    verify(&old, &new, agent)?;
    Ok(parts.join(&new_fm, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> Mapping {
        pairs
            .iter()
            .map(|(k, v)| (Value::String((*k).into()), Value::String((*v).into())))
            .collect()
    }

    #[test]
    fn replaces_only_the_agent_lines() {
        let text = "---\ntitle: \"Quoted: yes\"   # c\nagent:\n  v: 1\n  state: done\n  zz: keep\n\ncolor: red\n---\nbody\n";
        let agent = map(&[("state", "running")]);
        let out = set_agent(text, &agent, "body\n").unwrap();
        assert_eq!(
            out,
            "---\ntitle: \"Quoted: yes\"   # c\nagent:\n  state: running\n\ncolor: red\n---\nbody\n"
        );
    }

    #[test]
    fn appends_agent_and_keeps_crlf() {
        let text = "---\r\ntitle: a\r\nkind: agent\r\n---\r\nb\r\n";
        let out = set_agent(text, &map(&[("state", "running")]), "b\r\n").unwrap();
        assert_eq!(
            out,
            "---\r\ntitle: a\r\nkind: agent\r\nagent:\r\n  state: running\r\n---\r\nb\r\n"
        );
        let empty = set_agent("---\n---\nx", &map(&[("v", "1")]), "x").unwrap();
        assert!(empty.starts_with("---\nagent:\n  v: '1'\n---\n"), "{empty}");
    }

    /// An existing `agent:` block in a CRLF file is replaced in place; every
    /// other line keeps its bytes and its `\r\n`.
    #[test]
    fn replaces_an_existing_agent_block_and_keeps_crlf() {
        let text = "\u{feff}---\r\ntitle: a\r\nkind: agent\r\nagent:\r\n  v: 1\r\n  state: done\r\n  custom: keep me\r\n\r\ncolor: red   # c\r\n---\r\nb\r\n";
        let agent = map(&[("v", "1"), ("state", "running"), ("custom", "keep me")]);
        let out = set_agent(text, &agent, "b\r\n").unwrap();
        assert_eq!(
            out,
            "\u{feff}---\r\ntitle: a\r\nkind: agent\r\nagent:\r\n  v: '1'\r\n  state: running\r\n  custom: keep me\r\n\r\ncolor: red   # c\r\n---\r\nb\r\n"
        );
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
    }

    #[test]
    fn flow_style_agent_is_replaced() {
        let text = "---\nkind: agent\nagent: {v: 1, state: done}\npaper:\n  doi: x\n---\n";
        let out = set_agent(text, &map(&[("state", "failed")]), "").unwrap();
        assert!(
            out.contains("agent:\n  state: failed\npaper:\n  doi: x\n"),
            "{out}"
        );
    }

    #[test]
    fn unprovable_patches_are_refused() {
        // A quoted "agent" key is not found as a line; appending would duplicate it.
        let text = "---\n\"agent\":\n  v: 1\nkind: agent\n---\n";
        assert!(matches!(
            set_agent(text, &map(&[("v", "1")]), ""),
            Err(PatchError::Unverified(_))
        ));
        assert_eq!(
            set_agent("no fm", &Mapping::new(), "").unwrap_err(),
            PatchError::NoFrontmatter
        );
    }

    fn probe() -> Mapping {
        let mut m = map(&[("state", "running"), ("host", "H")]);
        m.insert(Value::String("v".into()), Value::Number(1.into()));
        m
    }

    /// The reviewer's probe table, part 1: these are written, and only the
    /// `agent` map and nothing else changes.
    #[test]
    fn probe_table_written_correctly() {
        let cases = [
            (
                "block scalar mentioning agent:",
                "---\nnotes: |\n  agent: not a key\n  more\nkind: agent\n---\nbody\n",
            ),
            (
                "flow map",
                "---\nkind: agent\nagent: {v: 1, state: done}\n---\nbody\n",
            ),
            (
                "multi-line flow map",
                "---\nkind: agent\nagent: {v: 1,\n  state: done}\ncolor: red\n---\nbody\n",
            ),
            ("agent: null", "---\nkind: agent\nagent: null\n---\nbody\n"),
            ("agent: ~", "---\nkind: agent\nagent: ~\n---\nbody\n"),
            (
                "agent:<TAB>{…}",
                "---\nkind: agent\nagent:\t{v: 1}\n---\nbody\n",
            ),
            (
                "agent_x and agents keys",
                "---\nkind: agent\nagent_x: 1\nagents: [a]\n---\nbody\n",
            ),
            (
                "body with --- and agent:",
                "---\nkind: agent\n---\nbody\n---\nagent: x\n",
            ),
            (
                "column-0 comment after the block",
                "---\nkind: agent\nagent:\n  v: 1\n# a comment\ncolor: red\n---\nbody\n",
            ),
        ];
        for (what, text) in cases {
            let body = split(text).unwrap().body;
            let out = set_agent(text, &probe(), body).unwrap_or_else(|e| panic!("{what}: {e:?}"));
            let parts = split(&out).unwrap();
            assert_eq!(parts.body, body, "{what}");
            let fm = parse_mapping(parts.frontmatter).unwrap();
            assert_eq!(fm.get(AGENT_KEY), Some(&Value::Mapping(probe())), "{what}");
            let old = parse_mapping(split(text).unwrap().frontmatter).unwrap();
            for (k, v) in old.iter().filter(|(k, _)| k.as_str() != Some(AGENT_KEY)) {
                assert_eq!(fm.get(k), Some(v), "{what}: {k:?}");
            }
            if what.starts_with("column-0 comment") {
                assert!(out.contains("# a comment\ncolor: red\n"), "{out}");
            }
            if what.starts_with("block scalar") {
                assert!(
                    out.contains("notes: |\n  agent: not a key\n  more\n"),
                    "{out}"
                );
            }
        }
    }

    /// The reviewer's probe table, part 2: these are refused (never written).
    #[test]
    fn probe_table_refused() {
        let cases = [
            (
                "column-0 comment inside the block",
                "---\nkind: agent\nagent:\n  v: 1\n# note\n  state: done\n---\nbody\n",
            ),
            (
                "quoted string continuing at column 0 with agent:",
                "---\ntitle: \"first line\nagent: x\"\nkind: agent\n---\nbody\n",
            ),
            (
                "anchor inside agent used elsewhere",
                "---\nkind: agent\nagent:\n  v: &one 1\nother: *one\n---\nbody\n",
            ),
            (
                "... document end marker",
                "---\nkind: agent\n...\n---\nbody\n",
            ),
            (
                "? agent explicit key",
                "---\n? agent\n: {v: 1}\nkind: agent\n---\nbody\n",
            ),
            (
                "duplicate agent",
                "---\nkind: agent\nagent:\n  v: 1\nagent:\n  v: 1\n---\nbody\n",
            ),
            (
                "no closing fence",
                "---\nkind: agent\nagent:\n  v: 1\nbody\n",
            ),
            (
                "lone-CR line ends",
                "---\rkind: agent\ragent:\r  v: 1\r---\rbody\r",
            ),
        ];
        for (what, text) in cases {
            let body = split(text).map_or("", |p| p.body);
            assert!(
                set_agent(text, &probe(), body).is_err(),
                "{what} was written"
            );
        }
    }

    #[test]
    fn with_values_keeps_order_and_appends() {
        let old = map(&[("v", "1"), ("custom", "x"), ("state", "done")]);
        let new = with_values(
            &old,
            &[
                ("state", Value::String("running".into())),
                ("host", Value::String("H".into())),
            ],
        );
        let keys: Vec<&str> = new.keys().filter_map(Value::as_str).collect();
        assert_eq!(keys, ["v", "custom", "state", "host"]);
        assert_eq!(new.get("state").and_then(Value::as_str), Some("running"));
        assert_eq!(new.get("custom").and_then(Value::as_str), Some("x"));
    }
}
