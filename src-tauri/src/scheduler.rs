//! Pure run scheduler (docs/ARCHITECTURE.md §4). No I/O, no tokio.
//!
//! The run loop asks [`Scheduler::ready`] which nodes to start now, starts them,
//! and reports each outcome with [`Scheduler::on_finished`]. The scheduler
//! enforces `max_parallel`, per-tag capacities (unknown tag => 1), `exclusive`,
//! the longest-downstream-path-first tie-break, skip propagation and
//! `stop_on_failure`.

use std::collections::{BTreeMap, BTreeSet};

use crate::contract::{AppError, AppResult, Graph, NodeStatus, RunState, RunTarget};
use crate::graph_util::{downstream_map, reachable, topo_order, upstream_map, validate};

/// Tag meaning "nothing else runs alongside this node".
pub const EXCLUSIVE: &str = "exclusive";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Slot {
    status: NodeStatus,
    reason: Option<String>,
}

/// What [`Scheduler::cancel_node`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelEffect {
    /// The node is running: the caller must kill its process, then report
    /// `on_finished(id, Cancelled, ..)`.
    MustKill,
    /// The node had not started; it is now `cancelled` and dependents `skipped`.
    Cancelled,
    /// The node is not in the run or already finished.
    Nothing,
}

#[derive(Debug, Clone)]
pub struct Scheduler {
    upstream: BTreeMap<String, BTreeSet<String>>,
    downstream: BTreeMap<String, BTreeSet<String>>,
    tags: BTreeMap<String, BTreeSet<String>>,
    priority: BTreeMap<String, usize>,
    capacities: BTreeMap<String, u32>,
    max_parallel: usize,
    stop_on_failure: bool,
    target: BTreeSet<String>,
    slots: BTreeMap<String, Slot>,
    /// Upstream node outside the target => run id holding its latest success.
    prior_success: BTreeMap<String, String>,
    /// Follow-up turns queued or in flight per node; a held node stays `running`.
    holds: BTreeMap<String, usize>,
    halted: bool,
    cancelled: bool,
}

impl Scheduler {
    /// `prior_runs`: earlier runs of this graph, any order (sorted newest first here).
    pub fn new(graph: &Graph, target: &RunTarget, prior_runs: &[RunState]) -> AppResult<Self> {
        validate(graph)?;
        let mut runs: Vec<&RunState> = prior_runs.iter().collect();
        runs.sort_by(|a, b| b.run_id.cmp(&a.run_id));

        let upstream = upstream_map(graph);
        let downstream = downstream_map(graph);
        let target = target_set(graph, target, &runs, &downstream)?;
        let prior_success = graph
            .nodes
            .iter()
            .filter(|n| !target.contains(&n.id))
            .filter_map(|n| latest_success(&runs, &n.id).map(|r| (n.id.clone(), r)))
            .collect();
        let slots = target
            .iter()
            .map(|id| {
                let slot = Slot {
                    status: NodeStatus::Pending,
                    reason: None,
                };
                (id.clone(), slot)
            })
            .collect();
        let mut scheduler = Self {
            tags: graph
                .nodes
                .iter()
                .map(|n| (n.id.clone(), n.resources.iter().cloned().collect()))
                .collect(),
            priority: priorities(graph, &downstream)?,
            capacities: graph.settings.resources.clone(),
            max_parallel: graph.settings.max_parallel.max(1) as usize,
            stop_on_failure: graph.settings.stop_on_failure,
            upstream,
            downstream,
            target,
            slots,
            prior_success,
            holds: BTreeMap::new(),
            halted: false,
            cancelled: false,
        };
        scheduler.skip_unsatisfiable();
        Ok(scheduler)
    }

    pub fn target(&self) -> &BTreeSet<String> {
        &self.target
    }

    pub fn status(&self, id: &str) -> Option<NodeStatus> {
        self.slots.get(id).map(|s| s.status)
    }

    pub fn reason(&self, id: &str) -> Option<&str> {
        self.slots.get(id).and_then(|s| s.reason.as_deref())
    }

    /// Run id that holds the latest successful result of an upstream node outside the target.
    pub fn prior_run_of(&self, id: &str) -> Option<&str> {
        self.prior_success.get(id).map(String::as_str)
    }

    pub fn running(&self) -> Vec<String> {
        self.ids_with(NodeStatus::Running)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Nodes to start now. They are marked `running`; ready nodes that do not fit
    /// the resource gate are marked `queued`.
    pub fn ready(&mut self) -> Vec<String> {
        if self.halted {
            return Vec::new();
        }
        let mut candidates: Vec<String> = self
            .slots
            .iter()
            .filter(|(_, s)| matches!(s.status, NodeStatus::Pending | NodeStatus::Queued))
            .map(|(id, _)| id.clone())
            .filter(|id| self.deps_satisfied(id))
            .collect();
        candidates.sort_by(|a, b| self.priority[b].cmp(&self.priority[a]).then(a.cmp(b)));

        let mut started = Vec::new();
        let mut reserved = false;
        for id in candidates {
            let gate = if reserved {
                Gate::Blocked
            } else {
                self.gate(&id)
            };
            let status = match gate {
                Gate::Fits => {
                    started.push(id.clone());
                    NodeStatus::Running
                }
                Gate::Blocked => NodeStatus::Queued,
                Gate::WaitsForExclusive => {
                    // An exclusive node waits for the floor to clear; nothing
                    // of lower priority may start meanwhile (no starvation).
                    reserved = true;
                    NodeStatus::Queued
                }
            };
            self.set(&id, status, None);
        }
        started
    }

    /// Scheduler for follow-up turns in a run that already finished: slots come
    /// from `run.json`, nothing new ever starts. Slots still `running`, `queued`
    /// or `pending` (a run left behind by an app exit or crash) become
    /// `cancelled` with reason `interrupted`; no process of theirs is alive.
    pub fn resume(graph: &Graph, run: &RunState) -> AppResult<Self> {
        let mut scheduler = Self::new(graph, &RunTarget::All, &[])?;
        scheduler.target = run
            .nodes
            .keys()
            .filter(|id| scheduler.upstream.contains_key(*id))
            .cloned()
            .collect();
        scheduler.slots = run
            .nodes
            .iter()
            .filter(|(id, _)| scheduler.target.contains(*id))
            .map(|(id, s)| {
                let slot = match s.status {
                    NodeStatus::Pending | NodeStatus::Queued | NodeStatus::Running => Slot {
                        status: NodeStatus::Cancelled,
                        reason: Some(crate::run_store::INTERRUPTED.to_string()),
                    },
                    NodeStatus::Succeeded
                    | NodeStatus::Failed
                    | NodeStatus::Skipped
                    | NodeStatus::Cancelled => Slot {
                        status: s.status,
                        reason: s.reason.clone(),
                    },
                };
                (id.clone(), slot)
            })
            .collect();
        scheduler.halted = true;
        scheduler.cancelled = run.status == crate::contract::RunStatus::Cancelled;
        Ok(scheduler)
    }

    /// A follow-up message was queued for a running node: it stays `running`
    /// (dependents wait) until [`Scheduler::release`] brings the count back to 0.
    pub fn hold(&mut self, id: &str) -> bool {
        if self.status(id) != Some(NodeStatus::Running) {
            return false;
        }
        *self.holds.entry(id.to_string()).or_default() += 1;
        true
    }

    /// A finished node gets a follow-up turn: it is `running` again and held.
    /// Dependents already skipped stay skipped.
    pub fn reopen(&mut self, id: &str) -> bool {
        match self.status(id) {
            Some(NodeStatus::Succeeded | NodeStatus::Failed | NodeStatus::Cancelled) => {
                self.set(id, NodeStatus::Running, None);
                *self.holds.entry(id.to_string()).or_default() += 1;
                true
            }
            _ => false,
        }
    }

    /// One follow-up turn of `id` ended.
    pub fn release(&mut self, id: &str) {
        if let Some(n) = self.holds.get_mut(id) {
            *n = n.saturating_sub(1);
        }
    }

    pub fn is_held(&self, id: &str) -> bool {
        self.holds.get(id).copied().unwrap_or(0) > 0
    }

    /// Record a node's end. `Failed`/`Cancelled`/`Skipped` skip every transitive
    /// dependent. While the node is held (follow-ups pending) the end is ignored,
    /// except `Cancelled`, which also drops the holds.
    pub fn on_finished(&mut self, id: &str, status: NodeStatus, reason: Option<String>) {
        if !self.slots.contains_key(id) {
            return;
        }
        if status == NodeStatus::Cancelled {
            self.holds.remove(id);
        } else if self.is_held(id) {
            return;
        }
        self.set(id, status, reason);
        match status {
            NodeStatus::Failed => {
                self.skip_dependents(id, &format!("upstream {id} failed"));
                if self.stop_on_failure {
                    self.halted = true;
                }
            }
            NodeStatus::Cancelled => {
                self.skip_dependents(id, &format!("upstream {id} was cancelled"))
            }
            NodeStatus::Skipped => self.skip_dependents(id, &format!("upstream {id} was skipped")),
            NodeStatus::Pending
            | NodeStatus::Queued
            | NodeStatus::Running
            | NodeStatus::Succeeded => {}
        }
    }

    pub fn cancel_node(&mut self, id: &str) -> CancelEffect {
        match self.status(id) {
            Some(NodeStatus::Running) => CancelEffect::MustKill,
            Some(NodeStatus::Pending | NodeStatus::Queued) => {
                self.on_finished(id, NodeStatus::Cancelled, Some("cancelled by user".into()));
                CancelEffect::Cancelled
            }
            _ => CancelEffect::Nothing,
        }
    }

    /// Nothing new starts; nodes not yet started become `cancelled`. Returns the
    /// running nodes the caller must kill.
    pub fn cancel_all(&mut self) -> Vec<String> {
        self.cancelled = true;
        self.halted = true;
        for id in self.ids_with_any(&[NodeStatus::Pending, NodeStatus::Queued]) {
            self.set(&id, NodeStatus::Cancelled, Some("run cancelled".into()));
        }
        self.running()
    }

    /// True when nothing runs and nothing can start.
    pub fn is_done(&self) -> bool {
        if !self.running().is_empty() {
            return false;
        }
        self.halted
            || !self.slots.iter().any(|(id, s)| {
                matches!(s.status, NodeStatus::Pending | NodeStatus::Queued)
                    && self.deps_satisfied(id)
            })
    }

    /// Call once `is_done()`: nodes that never became ready (only possible after
    /// `stop_on_failure` halted the run) become `skipped`.
    pub fn finalize(&mut self) {
        for id in self.ids_with_any(&[NodeStatus::Pending, NodeStatus::Queued]) {
            self.set(
                &id,
                NodeStatus::Skipped,
                Some("run stopped after a failure (stop_on_failure)".into()),
            );
        }
    }

    /// Overall status once the run is done.
    pub fn run_succeeded(&self) -> bool {
        self.slots
            .values()
            .all(|s| s.status == NodeStatus::Succeeded)
    }

    fn deps_satisfied(&self, id: &str) -> bool {
        self.upstream[id].iter().all(|up| match self.slots.get(up) {
            Some(slot) => slot.status == NodeStatus::Succeeded,
            None => self.prior_success.contains_key(up),
        })
    }

    fn gate(&self, id: &str) -> Gate {
        let running = self.running();
        if running.iter().any(|r| self.tags[r].contains(EXCLUSIVE))
            || running.len() >= self.max_parallel
        {
            return Gate::Blocked;
        }
        let tags = &self.tags[id];
        if tags.contains(EXCLUSIVE) {
            return if running.is_empty() {
                Gate::Fits
            } else {
                Gate::WaitsForExclusive
            };
        }
        let full = tags.iter().any(|tag| {
            let capacity = self.capacities.get(tag).copied().unwrap_or(1).max(1) as usize;
            running
                .iter()
                .filter(|r| self.tags[*r].contains(tag))
                .count()
                >= capacity
        });
        if full {
            Gate::Blocked
        } else {
            Gate::Fits
        }
    }

    /// Target nodes whose upstream lies outside the target without any earlier success.
    fn skip_unsatisfiable(&mut self) {
        let missing: Vec<(String, String)> = self
            .target
            .iter()
            .filter_map(|id| {
                self.upstream[id]
                    .iter()
                    .find(|up| !self.target.contains(*up) && !self.prior_success.contains_key(*up))
                    .map(|up| (id.clone(), up.clone()))
            })
            .collect();
        for (id, up) in missing {
            if self.status(&id) == Some(NodeStatus::Pending) {
                let reason = format!("upstream {up} is outside this run and never succeeded");
                self.on_finished(&id, NodeStatus::Skipped, Some(reason));
            }
        }
    }

    fn skip_dependents(&mut self, id: &str, reason: &str) {
        for dep in reachable(&self.downstream, id) {
            if matches!(
                self.status(&dep),
                Some(NodeStatus::Pending | NodeStatus::Queued)
            ) {
                self.set(&dep, NodeStatus::Skipped, Some(reason.to_string()));
            }
        }
    }

    fn set(&mut self, id: &str, status: NodeStatus, reason: Option<String>) {
        if let Some(slot) = self.slots.get_mut(id) {
            *slot = Slot { status, reason };
        }
    }

    fn ids_with(&self, status: NodeStatus) -> Vec<String> {
        self.ids_with_any(&[status])
    }

    fn ids_with_any(&self, statuses: &[NodeStatus]) -> Vec<String> {
        self.slots
            .iter()
            .filter(|(_, s)| statuses.contains(&s.status))
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Fits,
    Blocked,
    WaitsForExclusive,
}

fn target_set(
    graph: &Graph,
    target: &RunTarget,
    runs: &[&RunState],
    downstream: &BTreeMap<String, BTreeSet<String>>,
) -> AppResult<BTreeSet<String>> {
    let with_dependents = |seeds: BTreeSet<String>| -> BTreeSet<String> {
        let deps: Vec<String> = seeds
            .iter()
            .flat_map(|s| reachable(downstream, s))
            .collect();
        seeds.into_iter().chain(deps).collect()
    };
    match target {
        RunTarget::All => Ok(graph.nodes.iter().map(|n| n.id.clone()).collect()),
        RunTarget::From(id) => {
            if !downstream.contains_key(id) {
                return Err(AppError::new("not_found", format!("no node {id:?}")));
            }
            Ok(with_dependents(BTreeSet::from([id.clone()])))
        }
        RunTarget::FailedOf(run_id) => {
            let run = runs
                .iter()
                .find(|r| &r.run_id == run_id)
                .ok_or_else(|| AppError::new("not_found", format!("no run {run_id:?}")))?;
            let seeds: BTreeSet<String> = run
                .nodes
                .iter()
                .filter(|(id, s)| s.status != NodeStatus::Succeeded && downstream.contains_key(*id))
                .map(|(id, _)| id.clone())
                .collect();
            if seeds.is_empty() {
                return Err(AppError::new(
                    "not_found",
                    format!("run {run_id} has no failed, skipped or cancelled nodes"),
                ));
            }
            Ok(with_dependents(seeds))
        }
    }
}

fn latest_success(runs: &[&RunState], id: &str) -> Option<String> {
    runs.iter()
        .find(|r| r.nodes.get(id).map(|s| s.status) == Some(NodeStatus::Succeeded))
        .map(|r| r.run_id.clone())
}

/// Longest downstream path, counted in nodes (a sink has 1).
fn priorities(
    graph: &Graph,
    downstream: &BTreeMap<String, BTreeSet<String>>,
) -> AppResult<BTreeMap<String, usize>> {
    let order = topo_order(graph)?;
    let mut prio: BTreeMap<String, usize> = BTreeMap::new();
    for id in order.iter().rev() {
        let below = downstream[id].iter().map(|d| prio[d]).max().unwrap_or(0);
        prio.insert(id.clone(), below + 1);
    }
    Ok(prio)
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
