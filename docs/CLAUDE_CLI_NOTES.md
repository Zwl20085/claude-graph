# Driving `claude -p` from the app (Claude Code 2.1.x) — verified facts, 2026-11-07

Sources: https://code.claude.com/docs/en/headless.md , https://code.claude.com/docs/en/cli-reference.md ,
https://code.claude.com/docs/en/agent-sdk/typescript.md (event types).

## Invocation we use

```
claude -p "<short instruction>" --output-format stream-json --verbose \
       --model <sonnet|opus|haiku> [--permission-mode <acceptEdits|default|plan|dontAsk|bypassPermissions>] \
       --add-dir <graph-dir> [--allowedTools "Read Write Edit Bash(python:*)"] [--max-turns N] \
       [--max-budget-usd X] [--resume <session-id>]
```
- Prompt text: the positional argument is the instruction; **stdin carries data** (the full brief).
  Stdin cap 10 MB. We write the brief to stdin and **close stdin immediately** (a `-p` run can
  stay open waiting for background work if stdin never closes; idle cap 10 min).
  Positional argument we send: `Follow the node brief given on stdin exactly. Write the deliverables it names. End with the "## Deliverables" and "## Status:" sections it describes.`
- `--verbose` is required with stream-json in every documented example; always pass it.
- `--permission-mode`: since 2.1.302 a `-p` run with no mode set is Manual and **denies** anything
  that would prompt. `acceptEdits` auto-approves file writes and common fs commands; other shell
  calls still need an allow rule (`--allowedTools`). `dontAsk` denies all prompts. `bypassPermissions`
  approves everything (expose in the UI with a warning). Never use `--bare` (it ignores login).
  **The app's default (`claude_settings`, default of new graphs, fix pass 2) passes no
  `--permission-mode` flag**, so the user's own Claude Code settings (`permissions.defaultMode`,
  allow rules) decide. An explicit mode on a node or as the graph default overrides those
  settings for that run. A trial run with `acceptEdits` had its WebSearch and `gh` Bash calls
  denied headlessly; the node then ended `## Status: BLOCKED`.
- `--add-dir` paths must exist. `--model` accepts aliases sonnet/opus/haiku.
- Resume: `--resume <session_id>` works from any cwd. `--session-id <uuid>` sets one up front.
- Exit code 0 on success, non-zero on failure. SIGTERM → 143, no result. Auth failures are printed
  as the result on stdout (exact JSON unverified), so always parse the result object.

## stream-json lines (one JSON object per line on stdout)

- `{"type":"system","subtype":"init","session_id":"…","cwd":"…","model":"…","tools":[…],"permissionMode":"…","claude_code_version":"2.1.x", …}` — first line normally. Optional `plugin_errors`, `mcp_server_errors`.
- `{"type":"assistant","session_id":"…","parent_tool_use_id":null,"message":{"role":"assistant","content":[{"type":"text","text":"…"},{"type":"tool_use","id":"toolu_…","name":"Read","input":{…}}],"stop_reason":"tool_use"}}` — `parent_tool_use_id` non-null for subagent messages.
- `{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"…","content":"…","is_error":false}]}}`
- `{"type":"system","subtype":"permission_denied","tool_name":"WebSearch","tool_use_id":"toolu_…","message":"Claude requested permissions to use WebSearch, but you haven't granted it yet.","uuid":"…","session_id":"…"}` — one per refused tool call, right after the `tool_use` with that id (seen with 2.1.x, 2026-11-07). A Bash refusal adds `"decision_reason_type":"subcommandResults"` and a message naming the part that needs approval. The event carries no tool input; the app takes it from the matching `tool_use` and shows `Tool: "<first 120 chars of command/query/file_path>"` as a `permission_denied` node event. The final result repeats every refusal in `permission_denials: [{tool_name, tool_use_id, tool_input}]` (one entry per call, so a tool can repeat).
- `{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"retry_delay_ms":…,"error_status":429,"error":"rate_limit"}` — error ∈ authentication_failed, oauth_org_not_allowed, account_on_hold, billing_error, rate_limit, overloaded, invalid_request, model_not_found, server_error, max_output_tokens, cloud_credential_error, unknown.
- Final success: `{"type":"result","subtype":"success","session_id":"…","is_error":false,"result":"<final text>","stop_reason":"end_turn","num_turns":3,"duration_ms":12345,"duration_api_ms":11000,"total_cost_usd":0.04,"usage":{},"modelUsage":{},"permission_denials":[]}`
- Final error: `{"type":"result","subtype":"error_max_turns"|"error_during_execution"|"error_max_budget_usd"|"error_max_structured_output_retries","is_error":true,"errors":["…"],"session_id":"…","num_turns":…,"total_cost_usd":…}` — no `result` field.
- Other types exist (`stream_event` only with `--include-partial-messages`, `rate_limit_event`, `compact_boundary`, hook events). **Ignore unknown type/subtype, never fail on them.** Keep every raw line in events.jsonl.
- "Waiting for background work" and spinner text go to stderr, stdout stays clean JSON. If the consumer reads slowly the CLI waits up to 30 s to drain. Read stdout continuously.

## Windows

- Native installer puts `claude.exe` in `%USERPROFILE%\.local\bin\claude.exe`. npm global install gives
  `claude.cmd` shim in `%APPDATA%\npm` (common knowledge, unverified in docs).
- Rust `Command::new("claude")` finds `.exe` on PATH but not `.cmd`. Resolution order in the app:
  1. the app settings' `claude_bin` (`<data_dir>/claude-graph/settings.json`; graph.json's
  `settings.claude_bin` is ignored); 2. env `CLAUDE_GRAPH_CLAUDE_BIN` (may be `node path/to/fake-claude.mjs`, i.e. a command line, split on whitespace);
  3. `%USERPROFILE%\.local\bin\claude.exe`; 4. a PATH search (PATHEXT order).
- Spawn strategy, never `cmd /C`: an `.exe` is spawned directly. A `.cmd`/`.bat` npm shim is
  resolved to `node.exe <shim dir>\node_modules\@anthropic-ai\claude-code\cli.js` when that file
  exists (node.exe next to the shim or on PATH); else, when
  `<shim dir>\node_modules\@anthropic-ai\claude-code\bin\claude.exe` exists, that `.exe` is spawned
  directly (no cmd.exe); otherwise the `.cmd` itself is the program and Rust's std applies
  batch-file argument escaping (it errors on arguments it cannot pass safely; the node fails with
  `spawn_failed`). Verified 2026-11-07: the npm 2.1.319 shim is
  `"%dp0%\node_modules\@anthropic-ai\claude-code\bin\claude.exe" %*` (no cli.js), so such installs
  now run the bundled `claude.exe` directly; the `.cmd` fallback (only for other layouts) was
  checked not to inject with a graph folder named `g & mkdir INJECTED`.
- `--resume <id>` is only passed for ids matching `^[0-9a-fA-F-]{36}$`; `continue_session_of`
  adds `--fork-session` so the upstream's own session is not continued in place.
- Kill: `taskkill /T /F /PID <pid>` on Windows (process tree); on Unix spawn in its own process group and kill the group.
  Each child is also in a kill-on-close Job Object, closed at the end of the turn and by the OS on a crash.
- No sandbox on native Windows.

## Interactive Claude Code in a pty (terminal mode) — verified 2026-11-07, Claude Code 2.1.319, Windows 11

- `claude --settings <file.json>` loads hooks from that file. With
  `{"type":"command","command":"echo stop>> \"C:/path/hooks.log\""}` as the `Stop` hook the
  CLI appends `stop` at the end of every reply (also `-p` turns); `SessionStart` appends
  `start` once at launch, `Notification` when it waits for the user. Forward slashes in the
  path work on Windows.
- `UserPromptSubmit` runs when the CLI accepts a typed prompt, before the reply (the screen
  shows "running UserPromptSubmit hook"); with `echo submit>> "<log>"` the hook prints
  nothing, so nothing is added to the prompt. Verified 2026-11-07 (2.1.319): hooks.log reads
  `start`, `submit`, `stop` for one prompt. A slash command (`/model haiku`) fires **no**
  `submit`; the CLI answers on screen (the status line then shows the new model).
- Screen markers (2.1.319, through ConPTY): the empty input box is the prompt glyph plus a
  no-break space, `❯` U+276F + U+00A0 (`>` + U+00A0 when started from the app), followed by
  a dim placeholder (`Try "…"`). Typed text follows the glyph after a plain space. A dialog's
  choice cursor is the glyph followed by a cursor move (`ESC[1C`), which becomes a space
  once escapes are stripped. ConPTY writes most spaces as `ESC[1C`. The footer in auto mode
  reads "⏵⏵ auto mode on (shift+tab to cycle) · ← for agents"; "? for shortcuts" did not
  appear. While working: "<verb>… (esc to interrupt)" / "(running Stop hooks… 3/5)".
- Dialog texts (2.1.319): "Accessing workspace: <dir>" / "Quick safety check: Is this a
  project you created or one you trust? …" / "❯ No, exit" / "Yes, I trust this folder" /
  "Enter to confirm · Esc to cancel"; then "New MCP server found in this project: <name>" /
  "Use this MCP server" / "Use this and all future MCP servers in this project" /
  "❯ Continue without using this MCP server" / "Enter to confirm · Esc to cancel". After the
  folder is trusted (and the MCP choice made) a second session in it shows neither.
- Clearing the input box: **Ctrl+U** (0x15) deletes back to the start of the current box
  line and shows "Ctrl+Y to paste deleted text"; on an empty box it does nothing. A
  three-line paste needed more presses (one per line and one per line break), so the app
  sends 2 × lines + 2. **Esc** does not clear: the first shows "Esc again to clear"; a
  double Esc on an empty box opens the rewind menu (side effect), so it is not used.
- Enter can be lost: in the owner's trial a typed line starting with a slash command name
  (`/model-tiering Hi, …`) stayed in the box after `\r` (autocomplete took the key). The
  app's delivery presses Enter again while the text still shows (docs/ARCHITECTURE.md,
  Terminal mode, "Delivery").
- Spawned in ConPTY (portable-pty 0.9), the CLI's console first sends `ESC[6n` (cursor
  position query) and waits for the answer; the app answers `ESC[1;1R`.
- In a folder never trusted before, the workspace-trust dialog ("Quick safety check: Is this
  a project you created or one you trust?") comes first and its highlighted default is
  **"No, exit"** (Enter exits with code 1). No hook runs before it is answered. After trust
  a project MCP server can show a second dialog ("New MCP server found…", default "Continue
  without using this MCP server"), after `start`. Both end with "Enter to confirm · Esc to
  cancel".
- Typing `text\r` into the pty submits it as a prompt; the reply's Stop hook follows.
- Bracketed paste `ESC[200~line one\nline two ESC[201~` followed (300 ms later) by `\r` is
  submitted as **one** prompt (both lines in the prompt box, one reply, one `stop`).
- When the app itself runs inside a Claude Code session, the child shows "Transcript saving
  is off — inherited CLAUDE_CODE_CHILD_SESSION marker"; the app therefore removes
  `CLAUDE_CODE_CHILD_SESSION` and `CLAUDECODE` from the session process's environment.
- The real smoke test: `cargo test --test real_terminal -- --ignored --nocapture`
  (two short replies with `/model haiku`, about one to two cents): run 1 in a fresh folder
  leaves the trust dialog alone for 5 s (node `queued`, notification, nothing typed), then
  answers it like a user; run 2 in the same folder has no dialog.

## OpenAI Codex CLI in a pty (harness `codex`) — verified 2026-11-07, codex-cli 0.167.0, Windows 11

- Invocation used: `codex -C <cwd> --no-alt-screen -c notify=["<claude-graph.exe>","--notify","<session>\\hooks.log"]`
  (plus `-m <model>` when the profile sets one). The `-c` value is TOML; basic strings with
  doubled backslashes work. A boolean flag given twice is a clap error, so the adapter adds
  `--no-alt-screen` only when the profile's args do not have it.
- `notify`: after every finished turn Codex runs the program with ONE extra argument, a JSON
  object: `{"type":"agent-turn-complete","thread-id":…,"turn-id":…,"cwd":…,"client":"codex-tui","input-messages":[…],"last-assistant-message":…}`.
  Observed in a probe: on a thread's first message Codex also runs a side turn that writes
  the thread title, with its own `thread-id` and `input-messages` =
  `["Generate a concise, single-line task title … User prompt:\n<your message>"]`
  (`last-assistant-message` `{"title":"Reply ok"}`), and notifies for it as well. In the
  probe it came right after the real one; for a long turn it can come first. The app keeps
  the payload (`stop {json}` in hooks.log) and only counts a `stop` whose `input-messages`
  contain the typed message.
- Screen as rendered (vt100 rows, 120×40). Idle: banner `>_ OpenAI Codex (v0.167.0)`, the
  folder, `permissions: YOLO mode`, a list of slash commands, then the composer row
  `› Ask Codex to do anything` and a footer row
  `GPT-… medium · <folder> · ← for agents …  ⚠ 2 warnings · f2 to view`. Typed text replaces
  the placeholder in the `›` row. 0.5 s after Enter: the message stays in the history as
  `› <message>`, a row `• Working (0s • esc to interrupt)` appears (the bullet alternates
  `•`/`◦`), and the composer below shows the placeholder again. After the reply: `• ok`,
  `Worked for 2s • 22:01`, then the empty composer.
- Trust dialog: not shown on this machine (the owner's Codex config runs in "YOLO mode",
  also in a fresh temp folder), so its rendering was not observed. The dialog markers are
  the documented texts ("Folder access", "Trust this folder?", "Continue only if you trust
  these files", "Trust and continue", "Quit"; older: "Do you trust the contents of this
  directory?").
- Real run (`cargo test --test real_harness real_codex -- --ignored --nocapture`): ready,
  typed, submitted (composer check), one `stop {…}` line with
  `"input-messages":["Reply with the single word ok and create no files"],"last-assistant-message":"ok"`,
  node succeeded, reply.md holds `› Reply with …`, `• ok`, `Worked for 3s • …`. About 9 s.

## Claude Code against DeepSeek (harness `claude-deepseek`) — verified 2026-11-07

- The same interactive Claude Code (2.1.319) with the profile's env; the banner's model line
  reads `deepseek-flash[1m] with xhigh effort`. The hooks (`start`, `submit`, `stop`) work
  as with Anthropic's API. In a fresh folder the usual trust dialog and, on this machine, a
  project MCP server dialog come first (the test answers them like a user).
- Real run (`real_deepseek`, needs `DEEPSEEK_API_KEY`): succeeded, reply.md `● ok` /
  `✻ Crunched for 2s`, about 9 s.

## Permission prompt tool (`ask_app`) — verified 2026-11-07, Claude Code 2.1.319, Windows 11

Probe: `scripts/probe-prompt-tool.mjs` (a logging stdio MCP stub, env `PROBE_LOG` = log file,
`PROBE_DENY=1` answers deny), run as

```
claude -p "<ask for a shell command>" --output-format stream-json --verbose --model haiku \
       --permission-mode default --permission-prompt-tool mcp__claude_graph__approve \
       --mcp-config '{"mcpServers":{"claude_graph":{"command":"node","args":["C:/…/probe-prompt-tool.mjs"],"env":{"PROBE_LOG":"C:/…/probe.log"}}}}'
```

- `--mcp-config` takes the inline JSON string. Invalid JSON (for example unescaped Windows
  backslashes) is silently treated as a **file path** ("MCP config file not found: <cwd>\{…}"),
  so the app builds it with a JSON serializer.
- Handshake as the server sees it (one JSON object per line):
  1. `{"jsonrpc":"2.0","id":"server-discover-probe-1","method":"server/discover","params":{"_meta":{…"protocolVersion":"2026-07-28"…}}}`
     — a probe with a **string id**; answering JSON-RPC `-32601` method-not-found is fine.
  2. `initialize` with `"protocolVersion":"2025-11-25"` and `"id":0`; echoing the version works.
  3. `notifications/initialized` (no id, no reply), then `tools/list` (`"id":1`).
- `system/init` lists the server as `{"name":"claude_graph","status":"connected","source":"dynamic"}`
  in `mcp_servers`, next to the user's own servers (we do not pass `--strict-mcp-config`).
  No `mcp_server_errors` key was present when all is well.
- The prompt tool is called only when a tool call would otherwise prompt. `echo probe-ok`
  was auto-allowed by the user's settings (`permission_decision.source: "config"`) and the
  tool was **not** called. `mkdir probe-dir-allowed` did prompt and produced:

```
{"method":"tools/call","params":{"name":"approve",
  "arguments":{"tool_name":"Bash",
               "input":{"command":"mkdir probe-dir-allowed","description":"Create probe-dir-allowed directory"},
               "tool_use_id":"toolu_01Fihbi38pqnRE6HpLenNe2X"},
  "_meta":{"claudecode/toolUseId":"toolu_01Fihbi38pqnRE6HpLenNe2X","progressToken":2}},
 "jsonrpc":"2.0","id":2}
```

  So `arguments` = `{tool_name, input, tool_use_id}`; no `suggestions` (permission updates)
  were sent for this Bash call (the app still forwards them if a later CLI sends any).
- Allow answer (tool result text, a JSON string):
  `{"content":[{"type":"text","text":"{\"behavior\":\"allow\",\"updatedInput\":{…the input…}}"}]}`
  → the command ran; the tool result carried
  `tool_result_meta[].permission_decision = {"decision":"accept","source":"user_temporary"}`.
  `--allowedTools` did **not** need to list `mcp__claude_graph__approve`.
- Deny answer `{"behavior":"deny","message":"probe says no"}` → Claude saw a tool_result with
  `"content":"probe says no","is_error":true`, `permission_decision {"decision":"reject","source":"user_reject"}`;
  the run still ended `result/success` and listed the call in `permission_denials`.
- Modes: probed with `--permission-mode default` only. `bypassPermissions` never prompts and
  `dontAsk` denies instead of prompting, so the tool is only useful with `default` (the app
  uses `default` for `ask_app`). Not measured: how long the CLI waits for a `tools/call`
  answer (the app's server waits without a timeout; a user who leaves a dialog open for very
  long may hit the CLI's MCP tool timeout, `MCP_TOOL_TIMEOUT`).
- Cost: about one cent per probe run with haiku.
- App end to end (`cargo test --test real_ask -- --ignored --nocapture`, 2026-11-07): an
  `ask_app` node asked to `mkdir ask-app-probe-dir` produced one prompt
  `{tool_name:"Bash", input:{command:"mkdir ask-app-probe-dir", description:"…"}, suggestions:[]}`,
  the test answered allow/once, the folder was created, node succeeded ($0.009, about 17 s).

## Turn outcomes: hook payloads, transcript, dialogs and their keys — verified 2026-11-08, Claude Code 2.1.319, Windows 11

Probed with the real CLI in a ConPTY (120x40, `--permission-mode default --model haiku`, a
`--settings` file whose hooks ran a script that appended `<event> <stdin JSON>` to a log). Each
item: **V** = verified on that date, **U** = not verified.

**Hook stdin payloads (V).** Every hook gets one JSON object on stdin.
- Common fields: `session_id`, `transcript_path` (absolute path of the session's JSONL under
  `~/.claude/projects/<cwd-slug>/<session_id>.jsonl`), `cwd`, `hook_event_name`; most also
  carry `scratchpad_dir`, `prompt_id`, `permission_mode`, `effort {level}`.
- `SessionStart`: `source` ("startup"), `model` (e.g. `claude-haiku-5-5`).
- `UserPromptSubmit`: `prompt` = the submitted text, exactly.
- `Stop`: `stop_hook_active` (false), **`last_assistant_message`** = the turn's final assistant
  text, exactly (markdown kept, e.g. `You chose **B**.`), `background_tasks`, `session_crons`.
  So the app takes the reply text straight from the Stop line; the JSONL is the fallback.
- `Notification`: `message` ("Claude needs your permission"), `notification_type`
  ("permission_prompt", also sent for an open AskUserQuestion dialog). It comes **late**: 10 s
  (permission) to 18 s (AskUserQuestion) after the dialog was drawn, so it cannot be the
  dialog signal; it only corroborates.
- `PreToolUse` / `PostToolUse` (probed, not used by the app): `tool_name`, `tool_input`,
  `tool_use_id`; PostToolUse adds `tool_response` (AskUserQuestion: `answers {question: label}`).

**Transcript JSONL timing (V).** When the AskUserQuestion dialog is on screen, the JSONL already
ends with the assistant entry whose `message.content` holds `{type: "tool_use", id, name:
"AskUserQuestion", input: {questions: [{question, header, multiSelect, options: [{label,
description}]}]}}` (`stop_reason: "tool_use"`), with no `tool_result` for that id yet. Same for
a Bash call waiting on its permission prompt. After the answer a `user` entry with
`{type: "tool_result", tool_use_id}` follows. Other line types seen at the tail: `ai-title`,
`tools-used`, `session-owner`, `atis-latch`, `system` (`stop_hook_summary`, `turn_duration`),
attachments (`hook_success`).

**AskUserQuestion dialog (V).** Drawn in place of the input box:
```
 ☐ A or B                                   ← header (☒ once answered; tabs "← ☐ h  ✔ Submit →" for multi / several questions)
Which option do you want: A or B?            ← question
❯ 1. A                                       ← cursor ❯ on the highlighted option
     Choose option A.                        ← description (indented)
  2. B
     Choose option B.
  3. Type something.                         ← free-text option
──────────
  4. Chat about this
Enter to select · ↑/↓ to navigate · Esc to cancel   (also "· ctrl+g to edit in Notepad ·")
```
- Down/Up arrows (`ESC[B`/`ESC[A`) move the cursor (V).
- A digit selects that option **and submits at once** for a single-select question (V: Up back
  to 1, then `2` → "User answered … → B", the turn went on).
- A digit on the "Type something." row only moves the cursor there; typed text replaces the
  row's label in place (`❯ 3. neither, use C`) and Enter submits it (V).
- multiSelect: labels are `[ ] X`; Space toggles the highlighted box (`[✔] X`) (V); Enter on
  an option row **also toggles** (V). After the options come `[ ] Type something` and a
  `Submit` row; Enter on `Submit` opens "Review your answers … Ready to submit your answers?
  ❯ 1. Submit answers 2. Cancel", and Enter there submits (V).
- Several questions in one call (tabs): not probed (U); each question shows as its own dialog.

**Permission prompt (V).**
```
 Bash command
 …
 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and always allow access to <dir> from this project      (wraps onto a 2nd row)
   3. Yes, and switch to auto mode · auto mode handles these prompts for you
   4. No
 Esc to cancel · Tab to amend
```
- The digit `1` selected and ran the command at once (V). Enter confirms the highlighted
  option (U for this prompt; V for the trust dialog).
- **Esc** interrupts the turn: the screen shows "⎿ Interrupted · What should Claude do
  instead?" and **no Stop hook fires** (V). "4. No" behaves the same way (U). The app treats
  that screen as a `question` outcome so the node does not wait forever.

**Workspace / MCP dialogs (V).** A fresh folder can show "New MCP server found in this
project …" (options without numbers, `❯` on the highlighted row, "Enter to confirm · Esc to
cancel") or the trust dialog ("❯ No, exit / Yes, I trust this folder"). These stay start-up
dialogs (readiness gate markers), not node prompts.

**Open reply questions (V).** "Ask me one short open question …" ended with Stop and
`last_assistant_message: "What's your favourite colour?"`; the screen shows the idle box.

**End to end (V, 2026-11-08).** `cargo test --test real_terminal real_claude_dialog -- --ignored`
(two haiku turns in one session): the AskUserQuestion call became a `dialog` prompt from the
transcript (`question: "Do you want option A or option B?"`, options A, B, "Type something.",
"Chat about this", `selected: 0`) about 1 s after it was drawn; `answer_prompt {choice: 1}` sent
the digit `2`, the dialog closed, the reply was "You chose **B**." and the node succeeded. The
next node's reply "What's your favourite colour?" became a `question` prompt; `continue_node`
ended it (`succeeded`, no deliverables) and the run succeeded. hooks.log words: `start submit
stop submit stop`.
