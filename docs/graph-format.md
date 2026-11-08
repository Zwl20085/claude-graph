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
    "claude_bin": null,                   // deprecated and ignored: the claude path is a per-machine app setting
    "run_mode": "terminal",               // "terminal" (default; missing = terminal) | "headless"
    "harness": "claude"                   // default harness profile id of the nodes (missing = "claude")
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
  "permission_mode": "claude_settings" | "ask_app" | "acceptEdits" | "default" | "plan" | "dontAsk" | "bypassPermissions" | "inherit",
  "allowed_tools": ["Read", "Write", "Bash(python:*)"],   // optional
  "max_turns": 50,                        // optional
  "working_dir": null,                    // null = graph dir
  "continue_session_of": null,            // upstream node id whose session is resumed
  "extra_context": "",                    // free text appended to the brief
  "harness": null                         // harness profile id; null (or missing) = settings.harness
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
- `"claude_settings"` passes no `--permission-mode` flag: the user's own Claude Code
  settings decide. `create_graph` writes it as `settings.default_permission_mode` of new
  graphs; a graph without that setting still loads with `"acceptEdits"`. Any other
  explicit mode overrides the user's settings.
- `"ask_app"` ("Ask me in the app", headless engine): `--permission-mode default` plus the
  app's own permission prompt tool, so each permission prompt becomes a dialog in the app
  (ARCHITECTURE.md "Ask me in the app"). As `settings.default_permission_mode` it also
  applies to Assistant-tab turns; in terminal mode it means `default` (the session asks in
  its own window). Not an elevated setting (no trust consent). "Always for this graph"
  answers are kept per machine, never in graph.json: `<app data>/claude-graph/approvals.json`
  (`{ "<canonical graph folder>": [ { "tool": "Bash", "key": "<command>" }, … ] }`; key =
  the exact command / file path / url / query, else `sha256:<hex>` of the canonical JSON input).
- Resource tags: capacity comes from `settings.resources`; unknown tags have
  capacity 1; `exclusive` means nothing else runs alongside the node.
- `run_mode`: `"terminal"` runs each connected component as one interactive Claude Code
  session in its own window (ARCHITECTURE.md "Terminal mode (default)"); it ignores resource
  tags, `allowed_tools`, `max_turns`, `continue_session_of`, per-node permission modes and
  `default_model`. `"headless"` is the `claude -p` engine.

- `settings.harness` / `node.harness` name harness profiles (ARCHITECTURE.md "Harness
  profiles"): `claude`, `claude-deepseek` and `codex` are built in; other profiles are
  defined per machine in the app data (`harnesses.json`), never in graph.json. An unknown id
  loads fine and fails at run start (for the targeted nodes) with `invalid_input` "node
  <id>: harness <h> is not defined on this machine; add it under Settings → Harnesses". In
  terminal mode a chain is split into one session per run of consecutive nodes with the
  same harness; headless runs accept only claude-kind harnesses. `model` is a Claude model
  and is not sent to other harnesses (their model is set in the profile).

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

A terminal-mode run (`run.json` has `"mode": "terminal"` and a `sessions` list; older
run.json files load as `"headless"` with no sessions) also writes one folder per session:

```
runs/<run-id>/sessions/<sid>/      sid = s1, s2, … (one per connected component, split where the harness changes)
  hooks.json        --settings file: SessionStart/UserPromptSubmit/Stop/Notification hooks running
                    `"<app>" --notify "<hooks.log>" --event <word> --stdin`
  hooks.log         start / submit / stop / notify, one line per hook: the word, then the hook's
                    stdin payload as one line of JSON (transcript_path, prompt,
                    last_assistant_message, …); word only when the app binary could not be used
                    (echo fallback); Codex sessions: "stop {notify JSON}" lines
  transcript.log    raw terminal output of the session
  session.json      { sid, title, node_ids, pid, status, harness, started_at, finished_at }
                    (harness: profile id; missing in older files = "claude"; run.json's
                    `sessions[]` entries carry `harness` too)
```

and per node `runs/<run-id>/<node-id>/`:

```
  prompt.md         the brief sent
  reply.md          the node's turn as text: "> <first line of the typed message>", a blank
                    line, then the lines that appeared on the session's screen (and its
                    scrollback) from the start of typing the message to the Stop hook, as
                    a terminal emulator replaying the transcript shows them (spinners and
                    counters repainted in place are gone; the idle input box, footer and
                    box-drawing rules are left out); its last 256 KiB
  result.md         the same text as reply.md (shown in the Results tab)
  summary.json      { session, started_at, finished_at, status, reason, transcript_from,
                      transcript_to, questions, waited_for_user, … }
  questions.jsonl   the node's questions and dialogs (below)
```

When the CLI reports the turn's final text (Claude Code's Stop payload
`last_assistant_message`, Codex's `last-assistant-message`), reply.md is that exact text instead
of the screen: every message of the node (`> ` quoted, all lines) followed by its reply, so a node
whose question was answered shows the whole exchange.

`questions.jsonl` holds one JSON object per line, one per question or dialog, oldest first:

```jsonc
{ "id": "design-p1",                // <node>-p<n>, numbered per session
  "ts": "2026-11-08T02:00:59Z",
  "kind": "dialog",                  // "question" (the reply asked) | "dialog" (on screen during the turn)
  "source": "transcript",            // "transcript" | "screen" | "reply" | "events"
  "question": "Do you want option A or option B?",
  "header": "A or B",                // optional
  "options": [{ "label": "A", "detail": "Choose option A" }, { "label": "B" }],
  "selected": 0,                     // optional: highlighted option when seen
  "offer": false,                    // true: a reply question that did not pause (deliverables written)
  "answer": { "ts": "…", "via": "app", "text": "B", "index": 1 } }   // via: app | window | continue; absent until answered
```

The answer is written into its record: the file is small and rewritten whole (atomically)
when an answer arrives, so a reader never needs to join lines.

`transcript_from` / `transcript_to` are byte offsets into the session's `transcript.log`: its
length when the node's message started to be typed and when the Stop hook was seen (after up
to 1.5 s for the output to go quiet); `null` when the message was never submitted. The raw
bytes of the turn are `transcript.log[transcript_from..transcript_to]`.

The scheduler writes node deliverables to the same `nodes/<id>/deliverables/` folders as graph
mode. See ARCHITECTURE.md "Assistant mode (backend)".
