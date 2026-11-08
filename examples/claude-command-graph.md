---
description: Run a Claude Graph folder node by node with the claude-graph MCP tools
argument-hint: <absolute path of the graph folder>
---

Run the Claude Graph in $ARGUMENTS with the claude-graph MCP tools (server `claude-graph`, set up as
in docs/cli-addon.md).

1. Call `graph_open` with dir = the folder above. Keep the `run_id` it returns.
2. Call `graph_next` with dir and run_id. It gives you the brief of the next ready node or nodes.
3. Do the work the brief describes, here in this session. Write every deliverable into the node's
   deliverables folder.
4. Call `graph_report` with node_id, status "done" and a summary of what you did and where the
   results are. Downstream nodes read that summary.
5. Go back to step 2 until `graph_next` says the run is done. Then show me the `graph_status`
   table.

Only work on nodes that `graph_next` handed you. If a brief is unclear, ask me here. If you still
cannot proceed, report status "blocked" with your question. Once I have answered, call `graph_next`
with retry set to that node id. If you cannot do a node, report "failed" with the reason.
