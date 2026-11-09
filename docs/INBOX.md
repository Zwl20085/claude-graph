# Inbox: agent tips from Vindictive

Vindictive is a tile to-do board whose tips are Markdown files with YAML
frontmatter in a OneDrive-synced folder. A tip with `kind: agent` is a task
for an agent. The Inbox is Claude Graph's side of that deal: it scans the tips
folder, runs every eligible agent tip in its own session window, and writes
the outcome back into the tip. You only edit tips on the board.

The contract between the two apps is [AGENT-PROTOCOL.md](AGENT-PROTOCOL.md)
(kept identical in both repositories). Claude Graph is the *runner* of that
protocol (sections 6 to 9). This page says how the runner works in this app.

## The loop

1. **Scan** about 30 s after the app starts (when the Inbox is enabled), then
   every *N* hours (default 12) while the app is open, and on **Scan now**.
   One scan at a time; a request during a scan adds one more pass instead of
   running in parallel. A scan only reads: it lists `*.md` directly in the
   folder (not starting with `.` or `~`), reads each tip only up to its
   closing `---` fence (at most 64 KiB), and reads the whole file only for
   `kind: agent` tips. Fences are exactly `---` on their own line, as on the
   board. Unreadable files (not UTF-8, no frontmatter or a fence with
   trailing spaces, no closing fence within 64 KiB, bad YAML, an `agent`
   value that is not a map), agent tips whose frontmatter cannot be patched
   provably safely ("cannot patch the frontmatter; edit it by hand", for
   example a comment line inside the `agent:` block) and agent tips whose
   file name ends in a dot or a space are listed under *Skipped* with the
   reason and are never written.
2. **Eligible** tips (protocol section 6: new, changed since the last round,
   or left `running` by this machine after a crash or exit) join an in-memory
   queue. At most *max parallel* sessions (default 2) work at once; a queued
   tip is claimed only when it actually starts. A `running` claim of this
   machine is re-run only when its run id is one this machine recorded when
   claiming (the last 500, in `<app data>/claude-graph/inbox/state.json`);
   any other one ("claimed by an unknown run of this machine", for example a
   OneDrive conflict copy `name-HOST.md`) is left alone.
3. **Start**: the tip is re-read and checked again, its graph folder must be
   free (the "one run per graph folder" rule, and no agent-called run in it;
   a busy folder gets a re-scan after 60 s, at most 5 in a row), neither it
   nor any folder above it up to the app's data folder may contain
   `CLAUDE.md`, `CLAUDE.local.md`, `.mcp.json`, `.claude` or `.git` (an
   earlier round could have left them, and a new session would load them:
   Claude Code reads `CLAUDE.md` from every parent folder and project
   settings from a git root; the tip is not started, the reason names each
   folder, nothing is deleted), and the generated graph must pass the trust
   gate as it is. Then the tip is
   **claimed** (`agent.state: running`, `host`, `run`, `at`; the body read
   now is the *seen body*) and its graph is started as a normal terminal run,
   so the normal session window opens.
4. **Outcome**: the node's own turn outcome decides (protocol section 7
   table). The reply and the node's deliverables are copied to
   `<tips folder>/agent/<tip id>/` (by name, never deleted), then one block is
   appended to the body and `agent` is updated. The copy is skipped when a
   look at the tip's frontmatter right before it shows the tip gone, no
   longer an agent tip, or claimed by another machine or run.

| The node | Tip state | Block |
|---|---|---|
| succeeded | `done` | `## ⭐ Result` with the reply (cut at 4000 chars with a pointer to `result.md`) and `Files: agent/<id>/` |
| waits on a reply question | `question` | `## ❓ Questions` with the end of the reply |
| waits on a permission or choice dialog | `question` | `## ❓ Questions`: "Waiting at `<host>`: `<dialog>`. Answer in the Claude Graph session window." |
| failed, cancelled, did not start, timed out | `failed` | `## ⚠ Failed` with the reason |

A `done` or `failed` session stays open after its write-back, so you can
look at it, and is closed at the start of the next scan (timed, at
start-up, a re-scan, or **Scan now**): every Inbox session whose round was
written back as done or failed before that scan began is killed. A round
whose outcome is still waiting to be written stays open until it is.

A tip in `question` keeps its session open. You can answer in the session
window: when the node then finishes, that outcome is written as one more
block. Or you type the answer into the tip on the board: the next scan sees the
changed body, closes the earlier session and starts a fresh round with the
whole tip (earlier blocks included).

A round that works longer than the timeout (default 120 minutes) is cancelled
and reported `failed`. Time waiting at a question does not count. On app exit
the sessions are killed and nothing is written; tips this machine left
`running` are picked up again by the stale-claim rule at the next scan.

If a finished outcome cannot be written (the tip file is read-only, locked,
or kept changing), it is kept in memory and written at the next scan; the
panel shows "outcome not written yet: …" and the tip is not run again
meanwhile. (Kept outcomes do not survive an app restart; the stale-claim
rule then runs the tip again.)

When the earlier session of an edited question tip does not close within
20 s, the tip is not started; a re-scan follows after 60 s (at most 5 in a
row, then the next timed scan).

## Where things live

- The generated graph of a tip: `<app data>/claude-graph/inbox/<slug>-<hash>/`
  (`%APPDATA%\claude-graph\inbox\…` on Windows), never inside the tips folder
  or OneDrive. One node `task`, permission mode `claude_settings`, model
  inherit, default harness, terminal mode. Earlier rounds' runs and
  deliverables stay there. Inbox graphs are not added to the recent list; the
  Inbox panel opens them.
- The last scan time and the run ids this machine claimed with (last 500):
  `<app data>/claude-graph/inbox/state.json`.
- Results: `<tips folder>/agent/<tip id>/result.md` (the reply, copied last)
  plus the deliverables; a deliverable itself named `result.md` is copied as
  `deliverable-result.md`.

## Settings (per machine)

In the Inbox panel (top bar **Inbox**, or the start screen), stored in
`<app data>/claude-graph/settings.json` next to `claude_bin`:

| Key | Default | Rule |
|---|---|---|
| `inbox.enabled` | `false` | needs a folder; off means nothing is ever scanned |
| `inbox.folder` | empty | absolute local path; enabling needs an existing folder |
| `inbox.interval_hours` | 12 | 1 to 168 |
| `inbox.max_parallel` | 2 | 1 to 4 |
| `inbox.timeout_minutes` | 120 | 5 to 1440 |
| `autostart` | `false` | start Claude Graph with Windows (tauri-plugin-autostart, applied by the backend) |

With no folder set, the panel suggests one: the board's own `folder` setting
(`%APPDATA%\dev.zhangwentao.vindictive\settings.json`) if readable, else
`<OneDrive>\Vindictive` (env `OneDriveConsumer`, `OneDrive`,
`OneDriveCommercial`). A suggestion is only shown; **Use it** or **Choose…**
stores it, and the Inbox stays off until you enable it.

## Safety notes

- **Enable it on one machine only.** `host` and the `running` claim keep a
  second machine from repeating work, but OneDrive sync lag makes that a
  courtesy, not a lock.
- **Do not share the tips folder for writing.** Whoever can write a
  `kind: agent` tip into it gets an agent session on your machine with your
  Claude Code permissions. Keep it in your own OneDrive, not a shared one.
- **A tip stuck `running` from a machine that is gone** is never taken over
  (only the machine that claimed it re-runs it). To reset it, open the tip
  in a text editor and delete the `agent:` lines; it is then a new task.
- **Tip text is data.** It goes into the node brief only, fenced and labelled
  as the user's task. It is never typed into the terminal: the session always
  gets the one-line pointer `Node "Board task" (task): read the brief at
  <graph>/nodes/task/brief.md and do it; write all outputs to
  <graph>/nodes/task/deliverables/.` It never becomes a path, flag, working
  folder, model, harness or tool list.
- **Permissions are yours.** Sessions use `claude_settings` (no permission
  flag): your own Claude Code settings decide what the agent may do without
  asking. A permission dialog stops the agent in the session window and the
  tip shows ❓ with the dialog text; answer it in the window (the Inbox
  panel's session button opens it). Nothing is approved for you.
- **Planted files are refused, not removed.** A round could write
  `CLAUDE.md` next to or above its graph folder, or make a parent a git
  repository and put `.claude/` or `.mcp.json` there, and every later
  session would load it. The start check looks in the graph folder and in
  each folder above it up to and including the app's data folder (never
  higher: not your home folder), for `CLAUDE.md`, `CLAUDE.local.md`,
  `.mcp.json`, `.claude` and `.git`. The app itself never creates any of
  them there.
- **The trust gate is not bypassed.** The generated graph has no elevated
  settings. If its folder gains something elevated (for example an agent
  wrote `.claude/settings.json` into it), the tip is not started and the
  panel says why; no trust record is written for it.
- **Only agent tips are written**, and only their `agent` map and the end of
  the body. Every other frontmatter key keeps its bytes, value and position;
  a change that cannot be verified after re-parsing is not written. Writes
  go through a temp file in the same folder (flushed to disk), a stamp check
  (`<mtime ns>-<length>`) right before the rename, and the rename; a tip
  edited meanwhile is re-read and the change re-applied (5 tries); a tip
  deleted meanwhile is not recreated; a read-only tip is reported, not
  retried forever.

## Known limitations

- **The write window.** Between the last stamp check and the rename (a few
  microseconds) an edit saved by the board would be overwritten, and a
  delete would be undone; Windows offers no compare-and-swap rename.
  OneDrive keeps conflict copies, and the body hash makes the next scan run
  an overwritten edit again only if it reached the file before the check.
- **The copy window.** The results are copied after a read-only check that
  the claim is still this run's; another machine claiming the tip between
  that check and the write-back could still see its `result.md` replaced.
- **Question sessions stay open** until the tip is edited (then superseded),
  answered in the window, or the app exits; they have no timeout. (Done and
  failed sessions are closed at the next scan, see above.)
- **Generated graph folders under `<app data>/claude-graph/inbox/` are not
  pruned**: one per tip, with every round's runs and deliverables. Delete
  folders of retired tips by hand.
- The blank line before an appended block is ensured, not normalised: a
  body ending in several blank lines keeps them.
- Block markers in agent replies are defused case-sensitively
  (`agent:begin` / `agent:end` only).

## Code

`src-tauri/src/inbox/` (see the module docs in `mod.rs`): the pure protocol
core (`split`, `tip`, `hash`, `eligible`, `patch`, `block`, `ops`), the
folder I/O (`folder`, `writeback`, `results`, `scan`), orchestration
(`graphgen`, `runner`, `start`, `watch`, `timer`) and the Tauri side (`gui`).
IPC types: `src-tauri/src/contract_inbox.rs` / `src/types/contract.ts`.
Frontend: `src/store/inboxStore.ts`, `src/components/InboxPanel.tsx`,
`src/lib/inboxApi.ts`. End-to-end test with the fake CLI in a real pty:
`src-tauri/tests/e2e_inbox.rs`.
