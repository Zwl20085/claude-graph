//! Plain-text renderings of the engine's results: what the CLI prints by
//! default and what the MCP tools return to the model.

use crate::contract_agent::{AgentNext, AgentOverview, AgentReport, AgentStatus};

use super::run_word;

fn list(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
}

pub fn overview(o: &AgentOverview) -> String {
    let verb = if o.attached {
        "Resumed unfinished"
    } else {
        "Started"
    };
    let mut out = format!(
        "Graph \"{}\" in {}\n{verb} agent run {} ({}). At most {} node(s) at a time.\n\nNodes \
         (dependency order):\n",
        o.graph,
        o.dir,
        o.run_id,
        run_word(o.run_status),
        o.max_parallel
    );
    for n in &o.nodes {
        out.push_str(&format!("- {} [{}] {}", n.id, n.status.as_str(), n.title));
        if !n.goal.is_empty() && n.goal != n.title {
            out.push_str(&format!(" — {}", n.goal));
        }
        out.push('\n');
        if !n.deps.is_empty() {
            out.push_str(&format!("    after: {}\n", n.deps.join(", ")));
        }
        if !n.resources.is_empty() {
            out.push_str(&format!("    resources: {}\n", n.resources.join(", ")));
        }
        if !n.deliverables.is_empty() {
            out.push_str(&format!(
                "    deliverables: {}\n",
                n.deliverables.join(", ")
            ));
        }
    }
    out.push_str("\nNext: call graph_next to get the first brief.\n");
    out
}

pub fn status(s: &AgentStatus) -> String {
    let mut out = format!(
        "Run {} ({}), driven by {}\n",
        s.run_id,
        run_word(s.run_status),
        s.client
    );
    let width = s.rows.iter().map(|r| r.id.len()).max().unwrap_or(4).max(4);
    for r in &s.rows {
        out.push_str(&format!(
            "{:<width$}  {:<9}  {}",
            r.id,
            r.status.as_str(),
            r.title
        ));
        if let Some(reason) = r.reason.as_deref().filter(|x| !x.is_empty()) {
            out.push_str(&format!("  ({reason})"));
        }
        out.push('\n');
    }
    out
}

pub fn next(n: &AgentNext) -> String {
    if n.done {
        let summary = n.summary.as_ref().map(status).unwrap_or_default();
        return format!("All nodes are settled; the run is finished.\n{summary}");
    }
    let mut out = String::new();
    if n.nodes.is_empty() {
        out.push_str("Nothing is ready to start yet.\n");
        if !n.waiting_on.is_empty() {
            out.push_str(&format!(
                "Running (report these first): {}\n",
                list(&n.waiting_on)
            ));
        }
    } else {
        out.push_str(&format!(
            "Run {}: {} node(s) handed out. Do each brief, then report it.\n",
            n.run_id,
            n.nodes.len()
        ));
        if !n.waiting_on.is_empty() {
            out.push_str(&format!("Still running: {}\n", list(&n.waiting_on)));
        }
    }
    for b in &n.blocked {
        out.push_str(&format!(
            "Blocked: {} — {}\n  Ask the user; once answered, call graph_next with retry \"{}\".\n",
            b.node_id, b.question, b.node_id
        ));
    }
    for b in &n.nodes {
        out.push_str(&format!(
            "\n===== node {} ({}) =====\nBrief file: {}\nDeliverables folder: {}\n\n{}",
            b.node_id, b.title, b.brief_path, b.deliverables_dir, b.brief
        ));
        if !b.brief.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

pub fn report(r: &AgentReport) -> String {
    let mut out = format!("Node {} is now {}", r.node_id, r.status.as_str());
    if let Some(reason) = &r.reason {
        out.push_str(&format!(" ({reason})"));
    }
    out.push_str(".\n");
    if r.done {
        out.push_str(
            "Every node is settled; the run is finished. Call graph_status for the table.\n",
        );
    } else if r.ready.is_empty() {
        out.push_str("Nothing new is ready; call graph_next to see what the run waits on.\n");
    } else {
        out.push_str(&format!(
            "Ready now: {}. Call graph_next.\n",
            list(&r.ready)
        ));
    }
    out
}
