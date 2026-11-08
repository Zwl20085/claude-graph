# Call Claude Graph from your CLI (agent-called mode)

Usually the app starts the CLI. In this mode it works the other way round. You, or an agent, are
already in a Claude Code, Codex or dsh session. The agent calls Claude Graph to get the graph's work
one node at a time:

1. The agent asks for the next brief.
2. It does the work in its own session.
3. It reports back.
4. The app decides what is unlocked next.

The app does the scheduling (dependencies, resource tags, `max_parallel`) and keeps the record in
the graph folder, the same way a run from the window does. If the app is open on the same folder,
it follows the run live. No model, permission or tool flag is involved: the agent runs under your
own settings.

There are two ways in, and both use the same engine:

- The **MCP server** `claude-graph --mcp-graph`, for agents with an MCP client (Claude Code, Codex,
  dsh).
- The **plain CLI** `claude-graph graph …`, for agents that prefer Bash, and for people.

## Example

```
you:    run the graph in C:\work\my-graph
agent:  graph_open {dir: "C:\work\my-graph"}             → 3 nodes, run 20261108T200336Z
agent:  graph_next {dir, run_id}                         → brief of "hello" (and "world": 2 in parallel)
agent:  (writes nodes/hello/deliverables/hello.txt)
agent:  graph_report {dir, run_id, node_id: "hello", status: "done", summary: "wrote hello.txt"}
agent:  graph_next …  → "join" (its brief quotes the upstream summaries) … → done: true
```

## Set it up

In the app, open **Settings → Use from your CLI**. It shows each client's snippet with a Copy
button. The **Add** button shows the exact command or file it will run or write, and does nothing
until you confirm. `<exe>` below is the full path of `claude-graph.exe`; the panel fills it in.

| Client | Setup (verified 2026-11-08 on this machine) | Tool names |
|---|---|---|
| Claude Code 2.1.319 | `claude mcp add --scope user claude-graph -- "<exe>" --mcp-graph` (writes `~/.claude.json`; `claude mcp get claude-graph` showed it connected). Per project: `.mcp.json` `{"mcpServers": {"claude-graph": {"command": "<exe>", "args": ["--mcp-graph"]}}}` | `mcp__claude-graph__graph_next`, … |
| Codex CLI 0.167.0 | `codex mcp add claude_graph -- "<exe>" --mcp-graph`, which writes the block below to `~/.codex/config.toml` | `claude_graph` server |
| dsh 0.2.1-rc.2 | Add the entry below to `~/.dsh/cordis.patch.yml` (all profiles; the file is a YAML list, `[]` when empty), to `~/.dsh/profiles/<p>/cordis.patch.yml`, or to a file passed with `--patch <file>` | `mcp__claude-graph__…` |

Codex `config.toml` block:

```toml
[mcp_servers.claude_graph]
command = 'C:\path\to\claude-graph.exe'
args = ["--mcp-graph"]
```

dsh entry:

```yaml
- insert:
    - id: mcp-claude-graph
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: claude-graph
        transport: stdio
        command: 'C:\path\to\claude-graph.exe'
        args: ['--mcp-graph']
```

Notes:

- The Claude Code server is named `claude-graph`, not `claude_graph`. `claude_graph` is the app's
  own per-turn approval server ("Ask me in the app"), and a second server with that name would
  shadow it.
- dsh does not support MCP prompts. It does put the server's `instructions` into its system prompt.
  It starts the server twice: once for a probe, then once to serve. That is harmless, because the
  server keeps no state.

A slash command for Claude Code is in
[examples/claude-command-graph.md](../examples/claude-command-graph.md). Copy it to
`~/.claude/commands/graph.md`, then type `/graph C:\work\my-graph`.

## MCP tools (public API: names and arguments are stable)

| Tool | Arguments | Result |
|---|---|---|
| `graph_open` | `dir` (absolute), `new_run?` | Overview: graph name, nodes (id, title, one-line goal, deps, resource tags, deliverables, status), `run_id`. Resumes the folder's unfinished agent run; with `new_run: true`, ends that run first. |
| `graph_next` | `dir`, `run_id`, `max?` (1–16, default 1), `retry?` (node id) | The ready node(s): the full brief of each, marked `running`. If nothing is ready, what the run is waiting on and which nodes are blocked, each with its question. Once every node is settled, `done` and the status table. |
| `graph_report` | `dir`, `run_id`, `node_id`, `status` (`done`/`failed`/`blocked`), `summary`, `question?`, `deliverables_written?` | The node's new status and the nodes that became ready. |
| `graph_note` | `dir`, `run_id`, `node_id?`, `text` | Adds a line to the run log, which the app shows. |
| `graph_status` | `dir`, `run_id?` | Table of node statuses. Defaults to the newest agent run. |

The server's `initialize` result carries `instructions`: the loop, in 11 lines. The same text is
also available as the MCP prompt `graph_run` (argument `dir`).

A tool error comes back as a tool result with `isError: true` and the text
`Error (<code>): <message>`. Error codes:

| Code | Meaning |
|---|---|
| `invalid_input` | Bad argument, relative or UNC path, or node not in a reportable state |
| `not_found` | No graph.json, unknown run or node |
| `run_active` | A run started from the app is still running in the folder |
| `run_cancelled` | The run was stopped in the app, or replaced by a newer `graph_open` |
| `run_finished` | The run is already finished |
| `busy` | The lock was held for more than 10 s |

## Plain CLI

```
claude-graph graph open   <dir> [--new]
claude-graph graph next   <dir> [--run <id>] [--max <n>] [--retry <node>]
claude-graph graph report <dir> <node> --status done|failed|blocked
                          [--summary <text> | --summary-file <path>] [--question <text>]
                          [--written <path>]… [--run <id>]
claude-graph graph note   <dir> <text…> [--node <id>] [--run <id>]
claude-graph graph status <dir> [--run <id>]
common: --json (machine output; errors as {"error": {code, message}}), --client <name>
```

`--run` defaults to the folder's newest agent run. A relative `<dir>` is resolved against the
current directory.

| Exit code | Meaning |
|---|---|
| 0 | OK |
| 1 | Error, with the message on stderr |
| 2 | Usage error |
| 3 | `next`: nothing is ready yet |
| 4 | `next`: every node is settled |

The release `.exe` is a Windows GUI-subsystem program. When stdout is not redirected, it attaches
to the parent console. Output into a pipe, which is what agents use, works as is. PowerShell does
not wait for a GUI-subsystem program, so a person typing in PowerShell should pipe the output, for
example `claude-graph graph status . | Out-String`. Git Bash and cmd scripts wait normally.

## Rules the engine applies

- **Ready.** A node is ready when it is `pending` and every direct upstream node has `succeeded`.
  Ready nodes are handed out longest-downstream-path first, then by id.
- **Limits.** At most `max_parallel` nodes are `running` at once. Each resource tag holds at most
  its capacity (unknown tags hold 1). An `exclusive` node runs alone, and while it waits for the
  floor to clear, nothing of lower priority starts.
- **No double hand-out.** A node is never handed out twice. The only way back is `retry`, which is
  allowed only for a node reported failed or blocked. A retried node and its skipped dependents
  become `pending` again, and the retried node comes first. Its new brief quotes the earlier report
  under "Previous attempt in this run". A retry also reopens a run that ended `failed`.
- **`done`.** The app checks the node's required deliverables itself: each one must exist and must
  have been written since the hand-out, with 2 s of slack. If any is missing, the node becomes
  `failed`, with the reason `deliverables missing: a, b`.
- **`failed`.** Every transitive dependent that has not started becomes `skipped`. With
  `stop_on_failure`, every pending node becomes `skipped`.
- **`blocked`.** The node becomes `waiting` and holds no slot. Its dependents stay `pending`, so
  they are paused, not skipped. With `question`, the question is written to `questions.jsonl`. A
  waiting node can later be reported `done` directly, or retried.
- **End of run.** When no node is pending, running or waiting, the run becomes `succeeded` (every
  node succeeded) or `failed`.
- **Briefs.** A brief is the terminal-mode brief, which has no `## Status:` protocol. It inlines the
  upstream nodes' replies from this run, fenced as data, and ends with "Report back" for the
  caller's channel. It is written to `nodes/<id>/brief.md` and `runs/<run>/<id>/prompt.md`.
- **One run per folder.** `graph_open` resumes the unfinished agent run. It refuses while a run
  started from the app is `running`. While an agent run is active, the app refuses "Run all".
- **No trust prompt.** A graph's elevated settings (permission modes, allowed tools) are not used,
  so there is no trust prompt: the agent runs with its own permissions.

## Files

An agent run writes the same files as a terminal-mode run, plus these:

```
runs/<run>/run.json         a RunState with "mode": "terminal", plus
                            "engine": "agent", "agent": { client, channel: "mcp"|"cli", last_seen }
runs/<run>/agent_log.jsonl  { ts, node_id?, kind: run|handout|report|note, text } per line
runs/<run>/.agent.lock      present only during one call (a lock older than 30 s counts as stale)
runs/.agent-open.lock       present only while graph_open picks or creates a run
runs/<run>/<node>/          prompt.md, reply.md = result.md (the summary, plus "Question: …"),
                            summary.json (session "agent", status, reason, duration_ms, questions,
                            deliverables_written, client), questions.jsonl (blocked with a question)
GRAPH_LOG.md                one line per settled report (cost "-")
```

## The app while an agent drives

- The window watches the open folder. Once a second it checks the newest agent run's run.json (its
  mtime and size) and its run log, then pushes `run:state`, `node:event` and `agent_run` events, so
  the canvas follows within about 1 s. There is no loopback hub: the 1 s poll alone meets the 2 s
  target, and no extra listener or token is needed.
- The top bar shows a chip, "Driven by <client> in your CLI". Hover it to see the agent's latest
  run note. **Stop** asks first, then marks the run cancelled. The agent learns this on its next
  call (`run_cancelled`). The top bar's Cancel does the same.
- The Messages tab of an agent run shows the node's questions read-only, with "Answer in your CLI".
  The conversation lives in your CLI, and the app never types into it.
- When the app opens a folder, its sweep of stale runs leaves agent runs alone. Only Stop, or a new
  `graph_open` with `new_run`, ends an agent run.
