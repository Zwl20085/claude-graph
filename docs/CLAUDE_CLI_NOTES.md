# Driving `claude -p` from the app (Claude Code 2.1.x) — verified facts, 2026-11-07

Sources: https://code.claude.com/docs/en/headless.md , https://code.claude.com/docs/en/cli-reference.md ,
https://code.claude.com/docs/en/agent-sdk/typescript.md (event types).

## Invocation we use

```
claude -p "<short instruction>" --output-format stream-json --verbose \
       --model <sonnet|opus|haiku> --permission-mode <acceptEdits|default|plan|dontAsk|bypassPermissions> \
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
- `--add-dir` paths must exist. `--model` accepts aliases sonnet/opus/haiku.
- Resume: `--resume <session_id>` works from any cwd. `--session-id <uuid>` sets one up front.
- Exit code 0 on success, non-zero on failure. SIGTERM → 143, no result. Auth failures are printed
  as the result on stdout (exact JSON unverified), so always parse the result object.

## stream-json lines (one JSON object per line on stdout)

- `{"type":"system","subtype":"init","session_id":"…","cwd":"…","model":"…","tools":[…],"permissionMode":"…","claude_code_version":"2.1.x", …}` — first line normally. Optional `plugin_errors`, `mcp_server_errors`.
- `{"type":"assistant","session_id":"…","parent_tool_use_id":null,"message":{"role":"assistant","content":[{"type":"text","text":"…"},{"type":"tool_use","id":"toolu_…","name":"Read","input":{…}}],"stop_reason":"tool_use"}}` — `parent_tool_use_id` non-null for subagent messages.
- `{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"…","content":"…","is_error":false}]}}`
- `{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"retry_delay_ms":…,"error_status":429,"error":"rate_limit"}` — error ∈ authentication_failed, oauth_org_not_allowed, account_on_hold, billing_error, rate_limit, overloaded, invalid_request, model_not_found, server_error, max_output_tokens, cloud_credential_error, unknown.
- Final success: `{"type":"result","subtype":"success","session_id":"…","is_error":false,"result":"<final text>","stop_reason":"end_turn","num_turns":3,"duration_ms":12345,"duration_api_ms":11000,"total_cost_usd":0.04,"usage":{},"modelUsage":{},"permission_denials":[]}`
- Final error: `{"type":"result","subtype":"error_max_turns"|"error_during_execution"|"error_max_budget_usd"|"error_max_structured_output_retries","is_error":true,"errors":["…"],"session_id":"…","num_turns":…,"total_cost_usd":…}` — no `result` field.
- Other types exist (`stream_event` only with `--include-partial-messages`, `rate_limit_event`, `compact_boundary`, hook events). **Ignore unknown type/subtype, never fail on them.** Keep every raw line in events.jsonl.
- "Waiting for background work" and spinner text go to stderr, stdout stays clean JSON. If the consumer reads slowly the CLI waits up to 30 s to drain. Read stdout continuously.

## Windows

- Native installer puts `claude.exe` in `%USERPROFILE%\.local\bin\claude.exe`. npm global install gives
  `claude.cmd` shim in `%APPDATA%\npm` (common knowledge, unverified in docs).
- Rust `Command::new("claude")` finds `.exe` on PATH but not `.cmd`. Resolution order in the app:
  1. `settings.claude_bin` if set; 2. env `CLAUDE_GRAPH_CLAUDE_BIN` (may be `node path/to/fake-claude.mjs`, i.e. a command line, split on whitespace);
  3. `%USERPROFILE%\.local\bin\claude.exe`; 4. `where claude` / `which claude`; if the hit ends with `.cmd`/`.bat`, spawn `cmd /C <path> args…`.
- Kill: `taskkill /T /F /PID <pid>` on Windows (process tree); on Unix spawn in its own process group and kill the group.
- No sandbox on native Windows.
