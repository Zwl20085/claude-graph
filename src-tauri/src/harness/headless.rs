//! The headless engine (`claude -p`) on a claude-kind harness profile: the
//! profile's expanded env goes on the process (see `crate::turns`), its
//! `model` replaces the graph's default model.

use crate::contract::{Model, Node};

/// A claude-kind profile's `model` replaces the graph default (a node's own
/// model still wins): `--model` is set to it.
pub fn with_profile_model(args: Vec<String>, node: &Node, model: Option<String>) -> Vec<String> {
    let Some(model) = model.filter(|_| node.model == Model::Inherit) else {
        return args;
    };
    match args.iter().position(|a| a == "--model") {
        Some(i) if i + 1 < args.len() => args
            .iter()
            .enumerate()
            .map(|(j, a)| if j == i + 1 { model.clone() } else { a.clone() })
            .collect(),
        _ => {
            // After `-p <instruction>`, before the other flags.
            let at = args.len().min(2);
            let mut out = args[..at].to_vec();
            out.extend(["--model".to_string(), model]);
            out.extend_from_slice(&args[at..]);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_model_replaces_the_default_but_not_a_node_model() {
        let node: Node = serde_json::from_value(serde_json::json!({"id": "n"})).unwrap();
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let with = args(&["-p", "x", "--model", "sonnet", "--add-dir", "/g"]);
        assert_eq!(
            with_profile_model(with.clone(), &node, Some("m1".into())),
            args(&["-p", "x", "--model", "m1", "--add-dir", "/g"])
        );
        let without = args(&["-p", "x", "--add-dir", "/g"]);
        assert_eq!(
            with_profile_model(without.clone(), &node, Some("m1".into())),
            args(&["-p", "x", "--model", "m1", "--add-dir", "/g"])
        );
        assert_eq!(with_profile_model(without.clone(), &node, None), without);
        let opus = Node {
            model: Model::Opus,
            ..node
        };
        assert_eq!(
            with_profile_model(with.clone(), &opus, Some("m1".into())),
            with
        );
    }
}
