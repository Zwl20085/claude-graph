# Agent tips protocol, v1

A contract between two apps that never talk to each other directly:

- **Vindictive** (the board) is where you write a task. It only reads and shows agent state.
- **Claude Graph** (the runner) scans the tips folder on a timer, runs one agent session
  per task, and writes the outcome back into the tip.

The only channel is the tips folder (synced by OneDrive). This file is kept identical in
both repositories (`docs/AGENT-PROTOCOL.md`). Change it in both or in neither.

## 1. What marks a tip as an agent task

A tip is an agent task if and only if its frontmatter has `kind: agent`.
Every other tip is invisible to the runner: it is never read past its frontmatter, never
written, never run. Existing tips therefore need no migration and are never handled.

The task text is the tip itself: the title plus the Markdown body.

## 2. Frontmatter: the `agent` map

```yaml
---
title: Survey hairpin AC loss papers
kind: agent
status: open
agent:
  v: 1
  state: question            # running | done | question | failed; absent = never handled
  hash: sha256:262828d1...   # body hash as the runner left it (section 4)
  host: DESKTOP-ABC          # machine that handled it last
  run: 20261109-073100-ab12  # runner's run id
  at: 2026-11-09T07:31:00    # naive local time of the last state change
  results: agent/survey-hairpin-ac-loss-papers/   # relative to the tips folder
---
```

- The **runner** is the only writer of `agent`. The board never writes or removes it, and
  must preserve it (including unknown sub-keys) through every edit it makes.
- The **user** never has to touch `agent`. A new task is just `kind: agent` and some text.
- Unknown sub-keys of `agent` are preserved by both sides. `v` above 1 means: the board
  shows the tip as a plain agent tip without state; the runner skips it.

## 3. Body layout

The body is a conversation that only grows. The user's text is free Markdown. Each time the
runner finishes a round it appends exactly one block at the end of the body:

```markdown
<your prompt, any Markdown>

<!-- agent:begin 2026-11-09T07:31:00 question -->
## ❓ Questions · 2026-11-09 07:31
1. Which year range should the survey cover?
2. IEEE only, or arXiv too?
<!-- agent:end -->

2020 onwards, and include arXiv.

<!-- agent:begin 2026-11-09T19:31:00 done -->
## ⭐ Result · 2026-11-09 19:31
Found 14 papers; the summary table is in `survey.md`.

Files: agent/survey-hairpin-ac-loss-papers/
<!-- agent:end -->
```

Block rules:

- First line `<!-- agent:begin <naive local time> <state> -->`, last line `<!-- agent:end -->`,
  each on its own line, with one blank line before the block.
- `<state>` is `done`, `question` or `failed`. Headings are `## ⭐ Result`, `## ❓ Questions`,
  `## ⚠ Failed`, followed by ` · YYYY-MM-DD HH:MM`.
- A block is at most 4000 characters; a longer reply is cut with a pointer to the results folder.
- The runner never edits or removes earlier text, its own earlier blocks included.
- The user answers a question, or asks for more, by typing anywhere in the body (normally
  below the last block). There is no other "submit" action.

## 4. Body hash

`hash = "sha256:" + lowercase hex SHA-256 of normalise(body)`, where `normalise`:

1. drops a leading U+FEFF;
2. turns `\r\n` and lone `\r` into `\n`;
3. removes trailing spaces and tabs from every line;
4. removes leading and trailing empty lines;
5. encodes as UTF-8 with no final newline.

The body is everything after the closing `---` of the frontmatter. The title and the
frontmatter are not hashed, so renaming, recolouring or reordering a tip does not re-run it.

Test vectors (both repos carry these as unit tests):

| Raw body (escapes as in Rust string literals) | Hash (hex) |
|---|---|
| `"Find three recent papers on hairpin AC loss.\r\n\r\n"` | `a7b5da43b6fbfa12a04b1d93b1c7eb5b224f672554632f78c5bf68c6f5a2386f` |
| `"\u{feff}\n  line one  \r\n\tline two\t\n\n\n"` | `7ceac30c0680c7b43ef2cc5971ddff12cd7584a56cf6fa615f865ffb6f97adca` |
| `""` and `"\n \n"` | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `"调研 ⭐\n"` | `2115d91d771e0eafc845500cc950c60545e93cd145b2f4b80434fd36a6bf9f95` |
| `"Prompt\n\n<!-- agent:begin 2026-11-09T07:31:00 question -->\n## ❓ Questions\n1. Which year range?\n<!-- agent:end -->\n\n2020 onwards\n"` | `262828d169635feb78f04a5a63dde71f681a21c534598a9b19556af76446b9ac` |

## 5. Display state (what the board shows)

Derived, never stored. With `h = hash(current body)`:

| Condition, first match wins | Display state | Badge |
|---|---|---|
| `kind` is not `agent` | none | |
| `agent.v` present and above 1 | none | |
| `agent.state` is `running` | `running` | ⚙ |
| `agent.state` absent, or `agent.hash` absent, or `agent.hash != h` | `queued` | 🕓 |
| `agent.state` is `done` | `done` | ⭐ |
| `agent.state` is `question` | `question` | ❓ |
| `agent.state` is `failed` | `failed` | ⚠ |
| anything else | `queued` | 🕓 |

So a ⭐ or ❓ tip turns back to 🕓 the moment the user types more, without any app writing state.

## 6. When the runner handles a tip

At each scan, a tip is **eligible** when all of these hold:

1. `kind: agent`, `status` is not `done`, and `agent.v` is absent or 1;
2. the title or the normalised body is non-empty;
3. one of:
   - `agent.state` is absent (new task);
   - `agent.state` is `done`, `question` or `failed`, and `agent.hash != hash(body)`
     (the user added or changed something);
   - `agent.state` is `running`, `agent.host` is this machine, and the runner has no live
     run with id `agent.run` (left over from a crash or shutdown).

Everything else is skipped: unchanged ⭐/❓/⚠ tips, tips another host is running, done tips.
Marking the tip Done on the board is how the user retires a task.

## 7. Runner write-back

All writes are whole-file: read the file and its stamp (`<mtime ns>-<byte length>`), change
only the `agent` map and/or append one block, write a temp file in the same folder
(`.<name>.<pid>-<n>.tmp`), check that the stamp is unchanged, rename over the target. On a
changed stamp, re-read and retry (up to 5 times). All other frontmatter keys keep their
values and order.

1. **Claim** before starting: `state: running`, `host`, `run`, `at`. `hash` is left as it was.
   The body read in this step is the *seen body*: it is what the agent is given.
2. **Finish**: append the block, set `state` to `done` / `question` / `failed`, `at`,
   `results`, and
   - `hash = hash(new body)` if the body before the append equals the seen body;
   - otherwise (the user edited while the agent ran) `hash = hash(seen body)`, which no
     longer matches, so the tip shows 🕓 and the next scan runs it again with the new text.
3. **Files**: the final reply as `result.md`, plus any deliverables, are copied to
   `<tips folder>/agent/<tip id>/` (tip id = file name without `.md`). The board lists only
   `*.md` directly inside the tips folder, so this subfolder is ignored by it and synced by
   OneDrive. Earlier rounds' files are overwritten by name, never deleted.

Outcome mapping:

| Agent turn ends with | `state` | Block |
|---|---|---|
| a finished reply | `done` | `## ⭐ Result` + the reply (or its head) + `Files:` line |
| a question in the reply | `question` | `## ❓ Questions` + the question and its options |
| a permission or choice dialog in the session window | `question` | `## ❓ Questions` + "Waiting at `<host>`: `<dialog text>`. Answer in the Claude Graph session window." |
| error, cancelled, CLI missing, timeout | `failed` | `## ⚠ Failed` + the reason |

## 8. What the agent is told

Each eligible tip runs in its **own** agent session (its own graph folder, its own window).
The session gets the title and the whole seen body, with a fixed preamble saying: the text
is a task from the user's board; blocks between `agent:begin` and `agent:end` are earlier
rounds (your questions, your results); text outside them is the user's and the newest text
is what to act on now; if you need a decision, ask a short numbered list of questions and
stop; write files into the deliverables folder; end with a short summary.

The tip text is data for that one session. It is never typed into a terminal as a command
line, never used as a path, and never changes permissions, working folder, model or tools:
those come from the runner's own settings on the machine.

## 9. Operating rules

- The runner is **off by default** and enabled per machine. Enable it on one machine only;
  `host` plus the `running` claim keeps a second one from repeating work, but OneDrive sync
  lag makes that a courtesy, not a lock.
- The tips folder must not be shared for writing with anyone else: whoever can write a
  `kind: agent` tip there gets an agent session on the runner's machine.
- Frontmatter fences are exactly `---` on their own line (no trailing spaces) on both sides.
  The runner reads no more than the frontmatter of a tip that is not an agent tip.
- A stale `running` claim is only re-run by the machine that made it, and only for a run id
  that machine recorded. To reset a tip left `running` by a machine that is gone, delete
  the `agent:` lines in a text editor; it is then a new task.
- Scans happen at app start and every N hours (default 12), plus on demand.
- A board build older than this protocol reports `kind: agent` tips as unreadable. Update
  the board on every device before creating the first agent tip.
