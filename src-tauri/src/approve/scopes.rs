//! "Always for this graph" answers, persisted in
//! `<data_dir>/claude-graph/approvals.json`: `{ "<canonical graph dir>": [ {tool, key}, … ] }`.
//! Only allows are remembered. An unreadable file counts as empty (logged).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::contract::AppResult;
use crate::store::{read_json, write_json};

use super::wire::MatchKey;

pub const APPROVALS_FILE: &str = "approvals.json";

type Store = BTreeMap<String, BTreeSet<MatchKey>>;

fn key(graph_dir: &Path) -> String {
    graph_dir.display().to_string()
}

fn load(data_dir: &Path) -> Store {
    let file = data_dir.join(APPROVALS_FILE);
    if !file.is_file() {
        return Store::new();
    }
    read_json(&file).unwrap_or_else(|e| {
        log::warn!("ignoring unreadable {APPROVALS_FILE}: {e}");
        Store::new()
    })
}

/// Whether `entry` is always allowed for the graph in `graph_dir`.
pub fn allows(data_dir: &Path, graph_dir: &Path, entry: &MatchKey) -> bool {
    load(data_dir)
        .get(&key(graph_dir))
        .is_some_and(|set| set.contains(entry))
}

/// Remember `entry` for the graph in `graph_dir`.
pub fn remember(data_dir: &Path, graph_dir: &Path, entry: &MatchKey) -> AppResult<()> {
    let mut store = load(data_dir);
    let set = store.entry(key(graph_dir)).or_default();
    if !set.insert(entry.clone()) {
        return Ok(());
    }
    write_json(&data_dir.join(APPROVALS_FILE), &store)
}
