# Claude Graph — architecture brief

Claude Graph is a desktop app for engineering work that is too big for one agent
session. The user draws a directed acyclic graph of **nodes**. Each node is one
Claude Code task described by a fixed template (goal, acceptance criterion,
inputs from upstream nodes, deliverables, model, resource tags). The app runs the
graph through the **local `claude` CLI** that is already installed and logged in
on the machine. No API key, no account handling: billing and login are the
user's own Claude Code setup.

Everything a run produces lives **inside the graph's own folder**, so opening the
folder later tells the whole history of that graph.

## 1. Stack (decided)

| Layer | Choice | Why |
|---|---|---|
| Shell | Tauri 2 (Rust backend, WebView frontend) | Proven on this machine (Vindictive repo), small binary, Rust owns process control |
| Frontend | React 18 + TypeScript + Vite 6 + `@xyflow/react` v12 + Tailwind + zustand | React Flow is the most mature node editor; blueprint look is pure CSS on custom nodes |
| Tests | `cargo test` (scheduler, store, parser, prompt) + `vitest` (graph helpers, mapping) | Logic lives in pure modules so both sides test without a window |
| CI | GitHub Actions: cargo test + vitest + `cargo check` on windows-latest and ubuntu-latest | |
| Licence | MIT | |

Node 18 is the floor; Vite 6 is kept for that reason. Tailwind major version is
chosen by the scaffold agent from the version-compatibility report.
Scaffold pins (2026-11-07): React 18.3, Tailwind 3.4 (PostCSS), Vite 6.4,
`@vitejs/plugin-react` 4.7, vitest 3.2, `@xyflow/react` 12.12, zustand 5.

## 2. The graph folder (single source of truth)

```
<graph-dir>/
  graph.json                 # nodes, edges, positions, settings. Written by the app only.
  nodes/<node-id>/
    brief.md                 # the node template rendered from graph.json; the agent reads this
    deliverables/            # the agent MUST write its outputs here
  runs/<run-id>/             # run-id = UTC timestamp, e.g. 20261107T101500Z
    run.json                 # per-node status, timings, session ids, cost. Updated live.
    <node-id>/
      prompt.md              # exact text sent to claude (stdin)
      events.jsonl           # raw stream-json lines, untouched
      result.md              # final assistant text
      summary.json           # {session_id, cost_usd, duration_ms, num_turns, is_error, status}
  GRAPH_LOG.md               # append-only human log: one line per run per node (app-written)
```

`graph.json` schema (version 1):

```jsonc
{
  "version": 1,
  "name": "hello-pipeline",
  "settings": {
    "max_parallel": 2,                    // global concurrency
    "resources": { "cpu": 1, "memory": 1, "gpu": 1 },   // capacity per tag
    "stop_on_failure": false,
    "default_model": "sonnet",
    "default_permission_mode": "acceptEdits",
    "claude_bin": null                    // null = find on PATH; else explicit path
  },
  "nodes": [ { ...Node } ],
  "edges": [ { "id": "e1", "from": "node-a", "to": "node-b" } ]
}
```

`Node`:

```jsonc
{
  "id": "survey",                         // slug, unique, used as folder name
  "title": "Literature survey",
  "position": { "x": 100, "y": 200 },
  "goal": "One sentence. What done looks like.",
  "acceptance": "Checkable criterion.",
  "scope_in": "Files, dirs, URLs in play.",
  "scope_out": "Explicitly out of bounds.",
  "already_known": "What the author already ruled out.",
  "deliverables": [ { "path": "survey.md", "description": "Markdown table of 20 papers" } ],
  "do_nots": ["Do not install packages", "Do not touch tests"],
  "model": "sonnet" | "opus" | "haiku" | "inherit",
  "resources": ["cpu"],                   // any of cpu, memory, gpu, exclusive, or free text tags
  "permission_mode": "acceptEdits" | "default" | "plan" | "bypassPermissions" | "inherit",
  "allowed_tools": ["Read", "Write", "Bash(python:*)"],   // optional
  "max_turns": 50,                        // optional
  "working_dir": null,                    // null = graph dir
  "continue_session_of": null,            // upstream node id whose session is resumed
  "extra_context": ""                     // free text appended to the brief
}
```

Deliverable paths are relative to `nodes/<id>/deliverables/`.

## 3. Node brief template (what the agent receives)

`render_brief(graph, node, run)` produces `brief.md` (saved) and the run prompt
(`prompt.md` = brief + run-time header). Exact shape:

```
# Node: <title>   (graph: <name>, node id: <id>)

You are one node in a graph of engineering tasks run by Claude Graph.
Graph folder: <abs path>
Your deliverables folder: <abs path>/nodes/<id>/deliverables/   (write ALL outputs here)
Working directory: <abs path or custom>

## Goal
<goal>

## Acceptance criterion
<acceptance>

## Inputs from upstream nodes
### <upstream title> (id: <upstream-id>) — status: succeeded
Deliverables:
- <abs path>/nodes/<upstream-id>/deliverables/<path> — <description>
Result summary (first 4000 chars of its result.md):
<...>
(repeat per direct upstream node; if none: "None. This is a root node.")

## Scope
In: <scope_in>
Out: <scope_out>

## Already known / ruled out
<already_known>

## Deliverables (required)
- <path> — <description>
(each path relative to the deliverables folder above)

## Do not
- <do_nots...>

## Extra context
<extra_context>

## How to finish
Write every deliverable listed above. Then end your final message with exactly:

## Deliverables
- <path> — one line on what it contains
## Status: DONE
(or `## Status: BLOCKED` followed by one paragraph on why.)
```

The backend parses the final assistant text for `## Status: DONE|BLOCKED`, and
checks that each required deliverable file exists. Missing files or BLOCKED mark
the node `failed` (with reason) even if the CLI exit code is 0.

## 4. Scheduler (Rust, pure, fully unit-tested)

- Validate: unique ids, edges reference existing nodes, no cycles (Kahn). Reject on load with a clear error.
- A run has a target set: all nodes, or "from node X" (X and its transitive dependents), or "only failed of run R" (failed nodes and their dependents). Nodes outside the target set that are upstream of a target keep their latest successful result as input (copied reference, not rerun).
- Ready = all direct upstream nodes in the target set `succeeded` (or outside target set with a prior `succeeded` result).
- Resource gate: a ready node starts only if `running.len() < max_parallel` AND for every tag in `node.resources` the capacity minus tags in use is > 0. The tag `exclusive` means capacity 1 and additionally nothing else may be running and nothing starts while it runs. Unknown tags default to capacity 1 (so two nodes tagged `femm` never overlap).
- Tie-break among ready nodes: longest downstream path first, then id ascending. Deterministic.
- Failure: failed node → every transitive dependent becomes `skipped` (reason: upstream failed). Independent branches continue unless `stop_on_failure`, in which case nothing new starts and running nodes finish.
- Cancel run: nothing new starts, running children are killed, their nodes become `cancelled`.
- States: `pending → queued → running → succeeded | failed | skipped | cancelled`.

## 5. Claude runner (Rust)

One `claude` process per node:

```
claude -p --output-format stream-json --verbose
       --model <m> --permission-mode <pm> --add-dir <graph-dir>
       [--allowedTools ...] [--max-turns N] [--resume <session-id>]
```

- Prompt goes through **stdin** (long prompts, no quoting issues). cwd = working dir.
- Windows: `claude` on PATH is a `.cmd` shim; resolve the real target or spawn through `cmd /C`. The runner must work with an explicit `settings.claude_bin` and with the env override `CLAUDE_GRAPH_CLAUDE_BIN` (used by tests to point at `scripts/fake-claude.mjs`).
- stdout read line by line (tokio). Every line is appended to `events.jsonl` unchanged, parsed as JSON, and summarised into a `NodeEvent` emitted to the frontend (`assistant text delta`, `tool use <name>`, `tool result`, `result`). Malformed lines are kept and reported, never crash.
- The `result` event supplies session_id, cost, duration, num_turns, is_error → `summary.json`.
- Kill = process tree kill (Windows `taskkill /T /F /PID`, Unix kill group).
- A `check_claude` command reports found/not found, path, and version string; the UI shows a clear banner if missing.

## 6. Tauri IPC contract (frozen; both sides implement exactly this)

Commands (all return `Result<T, AppError>`; `AppError { code, message }`):

| Command | Args | Returns |
|---|---|---|
| `check_claude` | — | `{ found: bool, path: string|null, version: string|null }` |
| `create_graph` | `{ dir, name }` | `Graph` (writes graph.json + example README) |
| `open_graph` | `{ dir }` | `Graph` |
| `save_graph` | `{ dir, graph }` | `()` (validates, writes graph.json and every `nodes/<id>/brief.md`) |
| `list_recent` | — | `string[]` (graph dirs, most recent first; stored in app data) |
| `pick_folder` | `{ title }` | `string|null` (dialog plugin) |
| `start_run` | `{ dir, target: "all" | {from: id} | {failed_of: run_id} }` | `run_id` |
| `cancel_run` | `{ run_id }` | `()` |
| `cancel_node` | `{ run_id, node_id }` | `()` |
| `get_run` | `{ dir, run_id }` | `RunState` |
| `list_runs` | `{ dir }` | `RunSummary[]` |
| `read_node_result` | `{ dir, run_id, node_id }` | `{ result_md, summary, events_tail: string[] }` |
| `render_brief` | `{ graph, node_id }` | `string` (preview without saving) |
| `open_path` | `{ path }` | `()` (opener plugin) |

Events (backend → frontend, payloads are JSON):

- `run:state` — full `RunState` whenever any node status changes (small, so no diffs).
- `node:event` — `{ run_id, node_id, kind: "text"|"tool_use"|"tool_result"|"system"|"result"|"stderr"|"parse_error", text: string, ts }`.

`RunState`:

```jsonc
{ "run_id": "...", "graph_dir": "...", "started_at": "...", "finished_at": null,
  "status": "running" | "succeeded" | "failed" | "cancelled",
  "nodes": { "<id>": { "status": "...", "started_at": null, "finished_at": null,
             "session_id": null, "cost_usd": null, "num_turns": null, "reason": null } } }
```

TypeScript types for all of the above live in `src/types/contract.ts`; Rust types
in `src-tauri/src/contract.rs`. Field names are snake_case on both sides (serde
default), no renaming.

### Contract clarifications (added by the scaffold, 2026-11-07)

The brief left these open; the scaffold fixed them in both contract files.

- **Command argument names are snake_case on the wire.** Tauri 2 converts Rust
  argument names to camelCase by default; every command is declared with
  `#[tauri::command(rename_all = "snake_case")]` so `invoke` takes `run_id`,
  `node_id`, `claude_bin` exactly as in the table above.
- **`check_claude` takes one optional argument `{ claude_bin?: string | null }`.**
  Without it the check could not honour step 1 of the resolution order
  (`settings.claude_bin`), and the banner would be wrong for users who set an
  explicit path. Omitting it gives exactly the brief's behaviour.
- **`RunSummary`** (not defined in the brief):
  `{ run_id, started_at, finished_at, status: RunStatus, counts: { pending, queued, running, succeeded, failed, skipped, cancelled }, total_cost_usd: number|null }`.
- **`read_node_result`** returns `NodeResult { result_md: string|null, summary: NodeSummary|null, events_tail: string[] }`;
  `null` means the file does not exist yet. `NodeSummary` is the content of
  `summary.json`: `{ session_id, cost_usd, duration_ms, num_turns, is_error, status: NodeStatus }`
  (all nullable except `is_error` and `status`).
- **Timestamps** (`started_at`, `finished_at`, `ts`) are RFC 3339 UTC strings
  (`chrono::DateTime<Utc>` in Rust, `string` in TS).
- **Optional node fields** are `null` when unset, never omitted, when the app
  writes graph.json (`allowed_tools`, `max_turns`, `working_dir`, `continue_session_of`,
  `settings.claude_bin`). On load, missing fields take defaults (docs/graph-format.md).
- **`Model` and `PermissionMode` are closed enums** with exactly the values listed
  in §2 (`settings.default_model` uses the same `Model` type, so `inherit` there
  means "no `--model` flag"). The CLI's `dontAsk` mode is not in the enum.
- **Event names** are exported as constants: `EVENT_RUN_STATE = "run:state"`,
  `EVENT_NODE_EVENT = "node:event"` (both files).
- **AppError codes** in use: `not_implemented`, `io`, `json`, `invalid_graph`,
  `not_found`, `claude_not_found`, `run_active`, `internal`; the TS wrapper adds
  `ipc` for transport failures that are not an `AppError`.
- **`claude` lookup** lives in `src-tauri/src/claude_bin.rs` (`resolve`,
  `ClaudeCommand::command()`); the runner must spawn through it. On Windows the
  PATH search tries PATHEXT extensions only, so npm's extension-less `claude`
  shell script is skipped and `claude.cmd` is run as `cmd /C`.

## 7. Frontend (blueprint look)

- Canvas: dark grey (`#1e1e1e`) with a fine dot grid; MiniMap and Controls bottom-left.
- Node: rounded card, 1px border, header band coloured by model (opus violet, sonnet blue, haiku green, inherit grey) with the title and a small model chip; body shows goal (two lines, ellipsis), resource tags as small pills, deliverable count. One target handle on the left edge, one source handle on the right edge, drawn as filled circles (blueprint pins). Edges are bezier with a subtle glow; the edge animates while the upstream node is running.
- Status ring on the node border: pending grey, queued amber, running animated blue, succeeded green, failed red, skipped dashed grey, cancelled dark amber.
- Right panel (inspector): the template form for the selected node (every field in §2), live-rendered brief preview tab, and a "Results" tab listing past runs of that node with result.md rendered.
- Bottom panel: live log of the selected node for the active run (text, tool use lines), auto-scroll, copy.
- Top bar: graph name, Open / New / Save, Run all / Run from selected / Rerun failed, Cancel, settings popover (max_parallel, resource capacities, default model, stop_on_failure, claude path), claude-found banner.
- Adding an edge runs cycle detection first; a cycle is refused with a toast.
- Autosave 800 ms after any change; Ctrl+S saves immediately. Unsaved marker in the title.
- All graph logic (cycle check, topo preview, status→style mapping, graph↔ReactFlow mapping) is in `src/lib/*.ts`, pure, tested with vitest.

## 8. Fake claude for tests and demos

`scripts/fake-claude.mjs` reads the prompt from stdin, emits a plausible
stream-json sequence (system init, two assistant messages with one tool use, a
result with a session id), writes each required deliverable it finds in the
prompt as a stub file, and exits 0. `CLAUDE_GRAPH_CLAUDE_BIN=node scripts/fake-claude.mjs`
lets `cargo test` run an end-to-end graph without the real CLI. If the prompt
contains the text `FAKE_FAIL` (put it in a node's `extra_context`) it ends with
`## Status: BLOCKED` and still exits 0, to test failure propagation. `--version`
prints `2.1.0 (fake-claude)`; env `FAKE_CLAUDE_DELAY_MS` slows the stream for cancel tests.

## 9. Out of scope for v0.1

Subgraphs, loops/conditionals, per-node MCP config, editing brief.md by hand and
syncing back, remote execution, auto-layout. Each is a documented future item.
