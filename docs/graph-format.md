# graph.json format (version 1)

Standalone reference extracted from [ARCHITECTURE.md](ARCHITECTURE.md) §2. Types: `src/types/contract.ts` (TypeScript) and `src-tauri/src/contract.rs` (Rust). Worked example: [examples/hello-pipeline/graph.json](../examples/hello-pipeline/graph.json).

`graph.json` lives at the root of the graph folder and is written by the app only.

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
    "claude_bin": null                    // deprecated and ignored: the claude path is a per-machine app setting
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

## Loading rules

- `version` must be `1`. `name` and every node `id` are required.
- Every other node field is optional on load and gets a default: strings `""`,
  lists `[]`, `model` and `permission_mode` `"inherit"`, nullable fields `null`,
  `position` `{ "x": 0, "y": 0 }`. Missing `settings` (or any setting) takes the
  defaults shown above. The app always writes every field back.
- Node ids are unique; edge ids are unique; every edge's `from` and `to` name an
  existing node; the graph has no cycles (Kahn). A graph that breaks any of these
  is rejected on load with a clear error.
- `model: "inherit"` means `settings.default_model`; `permission_mode: "inherit"`
  means `settings.default_permission_mode`.
- Resource tags: capacity comes from `settings.resources`; unknown tags have
  capacity 1; `exclusive` means nothing else runs alongside the node.

## Other folders

Next to `graph.json` the app writes `nodes/<id>/` (brief and deliverables), `runs/<run-id>/`
(graph-mode runs) and, once assistant mode is used, `assistant/`:

```
assistant/
  session.json        scheduler session: session_id, status, turns, model, workflow_hash, last_error, updated_at
  journal.jsonl       one message per line (user instructions/answers, scheduler stages/questions/
                      decisions/notes/finals, app system/error messages), append only
  CHARTER.md          the scheduler charter (written on the first turn; edit it to change the charter)
  turns/0001/         prompt.md, events.jsonl, stderr.log, reply.md of each scheduler turn
  PLAN.md, DECISIONS.md   written by the scheduler itself
```

The scheduler writes node deliverables to the same `nodes/<id>/deliverables/` folders as graph
mode. See ARCHITECTURE.md "Assistant mode (backend)".
