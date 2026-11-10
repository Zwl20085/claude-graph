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
3. **Start**: the tip is re-read and checked again. Every round runs in a
   fresh graph folder of its own (see *Where things live*). Nothing is
   written before the claim, so a tip that cannot start leaves no folder.
   Before the claim:
   - no graph folder of the tip may be busy: a run of this app (its own
     in-memory list, never a record on disk, which a round could forge) in
     an earlier round's folder or the 0.7.0 folder; a busy folder gets a
     re-scan after 60 s, at most 5 in a row;
   - the folders the Inbox owns above the round (`inbox`, `inbox/rounds`,
     `inbox/rounds/<tip>`), where they exist, must be plain folders: a
     symbolic link, a junction or any other reparse point is refused (a
     session in a round could otherwise swap one for a junction to a place
     whose parents hold `CLAUDE.md`);
   - the new round's folder must not exist yet, and neither it nor any
     folder above it up to the app's data folder may contain `CLAUDE.md`,
     `CLAUDE.local.md`, `.mcp.json`, `.claude` or `.git` (a round could have
     planted them there, and a new session would load them: Claude Code
     reads `CLAUDE.md` from every parent folder and project settings from a
     git root; the tip is not started, the reason names each folder, nothing
     is deleted; a folder outside the app's data folder is refused outright).
     What a round left in its own folder (for example the
     `.claude/settings.local.json` Claude Code writes when you answer its MCP
     dialog or pick "don't ask again") is in neither, so it never blocks the
     next round and is never loaded by it;
   - the generated graph must pass the trust gate as it is.

   Then the tip is **claimed** (`agent.state: running`, `host`, `run`, `at`;
   the body read now is the *seen body*), the round's folder is created
   (each missing owned level as a plain folder, the round folder itself only
   if it is new), the graph is written, and, as the last step before the
   run starts, every owned level and the round folder are checked again: plain
   folders, the round folder resolving to exactly its own path, no planted
   file above it. A failure there is reported on the tip as `failed`. Then
   the graph is started as a normal terminal run, so the normal session
   window opens.
4. **Outcome**: the node's own turn outcome decides (protocol section 7
   table). The reply, the round's prompt and the node's deliverables are
   copied to `<tips folder>/agent/<tip id>/` (by name, never deleted), then
   one block is appended to the body and `agent` is updated. The copy is skipped when a
   look at the tip's frontmatter right before it shows the tip gone, no
   longer an agent tip, or claimed by another machine or run.

| The node | Tip state | Block |
|---|---|---|
| succeeded | `done` | `## ⭐ Result` with the reply (cut at 4000 chars with a pointer to `result.md`) and `Files: agent/<id>/` |
| waits on a reply question | `question` | `## ❓ Questions` with the end of the reply |
| waits on a permission or choice dialog | `question` | `## ❓ Questions`: "Waiting at `<host>`: `<dialog>`. Answer in the Claude Graph session window." |
| waits at a Claude Code start-up dialog (workspace trust, new MCP server) before its message was typed | `question` | the same, with a fixed text: `Claude Code start-up: do you trust the files in this folder? (workspace trust)`, `Claude Code start-up: new MCP server found in this project`, or `Claude Code start-up dialog` (also the MCP one: a legitimate `.mcp.json` above the app's data folder shows it, so it is a question, not a failure) |
| failed, cancelled, did not start, timed out | `failed` | `## ⚠ Failed` with the reason |

When something of a done or failed round did not reach the results folder,
the block ends with the runner's own line, in fixed words, kept whatever
the reply's length: `Not copied to agent/<id>/: `a.md`, `sub/b.md`.` (the
files not copied, names relative to the deliverables folder, each in a code
span; `result.md` for the reply; `(the results folder)` when that folder
could not be used at all; `(more files)` when the deliverables went past
the copy cap plus 50; at most 10 names, then "and N more"; never an error
text or a local path). The reasons are in the Inbox panel's log. A round
that failed before the agent replied does not list `result.md` (there was
none); a done round whose reply file is missing does. A reply over 32 MiB
is copied as written, and the block says so on one more fixed line:
`` `result.md` is the reply as written, not rewritten (over 32 MiB). ``

A reply cut at 4000 characters points at `agent/<id>/result.md`; when the
reply did not reach the results folder it says "the full reply could not be
copied to agent/<id>/" instead. Two deliverables that would get the same
name in the results folder (the agent's own `deliverable-result.md` and a
renamed `result.md`) are not mixed up: the second is not copied and is
listed.

**Paths in the texts.** The agent works in a folder of this machine, so the
paths it writes mean nothing on your other devices and show your Windows
user name. Every text the runner writes into the tip (the reply, a
question, a dialog line, a failure reason) and `result.md` are rewritten;
nothing else in the tip is. Only absolute paths are touched. A path starts
with a drive (`C:\`, `c:/`, `C%3A%5C`, Git Bash `/c/`, WSL `/mnt/c/`,
`/C:/`), `~/`, or a variable (`%NAME%`, `$env:NAME`, `$HOME`,
`$USERPROFILE`), after an optional `file://` (also `file://localhost/`),
`\\?\` or `\\.\` prefix. A drive letter never follows a letter, digit or
`:` (except a `:` right after a word, as in `Path:C:\…`), and `~/` or `/c/`
never follow a letter, digit or `/`, so URLs are not paths. The most
specific known folder wins:

| The path is under | It becomes |
|---|---|
| this round's deliverables folder: a copied file | `agent/<tip id>/<name of the copy>` (a renamed copy by its new name) |
| … a folder of copied files (the whole path), or the folder itself | `agent/<tip id>/<folder>`, `agent/<tip id>/` |
| … a file that was not copied | `<file> (not copied)` |
| … any other name (also a file under a folder of copies that was not copied) | `<name>` (its name with `/`, no claim) |
| … a `..` way out of it (also `%2e%2e`) | `(a folder on the runner's machine)` |
| this round's folder, the app data folder, or any `claude-graph` data folder (another round, another tip, another machine) | `(a folder on the runner's machine)` |
| the tips folder | the path relative to it (the folder itself: `(the tips folder)`) |
| your home folder (anything else under it: Documents, Temp, `.claude`) | `~/<rest>`; written with a variable (`$HOME/proj`, `%USERPROFILE%\x`) it stays as written (no name shows, and a copied command keeps working); a variable-rooted path into one of the folders above is rewritten like the others |
| anything else (another drive, outside your home folder) | unchanged |

Folders are compared component by component, case-insensitively, each
component in its long or 8.3 form (so mixed spellings such as
`C:\Users\ZHANGW~1\AppData\Roaming\claude-graph\…` work; an 8.3 name only
stands for the long name it abbreviates, never for another folder next to
it, and `~` spellings are made only from an exact spelling of your home
folder), with any
separators between (`/`, `\`, doubled, `%2F`, `%5C`, `./`); a component
must end at a name boundary, so `C:\Users\<you>2` or `<you>-old` are not
your home folder. After the deliverables folder, file names are matched
against the round's file list, so names with spaces, `%20`, back or doubled
slashes and `./` work, and what follows a name (`#L10`, `:12`, punctuation,
Chinese punctuation, emphasis marks) stays outside it. Prose about the
folder layout without an absolute root is not touched.

A last pass (the backstop) hides what odd spellings leave, by context only:
`Users`, then a run of separator-like characters (`/ \ ／ ＼ ∕ ⁄ ∖ ⧵`,
`&#92;`, `&#47;`, `&#x5c;`, `&#x2f;`, `&bsol;`, `&sol;`, `%5C`, `%2F`, mixed
with zero-width characters, `*`, `_`, backticks, spaces or line breaks,
with at least one real separator), then your home folder's name (long or
8.3, zero-width characters, `*`, backticks or line breaks inside it
skipped; also a domain profile folder `<name>.<DOMAIN>` followed by a
separator, but not `<name>.txt` at the end of a path): the name becomes
`(a folder on the runner's machine)`. The same
for `AppData` … `Roaming` … `claude-graph` as a whole. This covers UNC
paths (`\\localhost\C$\Users\<you>`), `C:Users\<you>`, HTML entities,
look-alike separators and markup-split names. Your name in ordinary prose
(not after `Users` and a separator) is left as it is.

The rewrite is linear in the text (each scan from a path start is bounded
to 4 KiB); `result.md` is rewritten up to 32 MiB (above that it is copied as
written, see the line above). A reply that is not UTF-8 is rewritten in one
pass with each invalid byte sequence held by a placeholder the rewrite
skips inside paths (so an invalid byte cannot hide a path or your name); the
invalid bytes come out byte for byte, except one inside a path that was
rewritten, which goes with it. Texts for the tip are rewritten up to their first 64 KiB (the block
keeps 4000 characters of them anyway). The brief also asks the agent to
name its files by file name only.


A `done` round's session is ended and its window closed right after its
outcome and files are written (Inbox runs only; ordinary graph runs keep
their windows). The run itself is not cancelled (it stays recorded as
succeeded), and the closing happens in the background, so a waiting tip
can take the freed slot at once. If the done outcome could not be written
yet (see below), the session stays open until it is, and is closed at the
scan after that. If closing fails it is logged and the tip stays `done`; a
session that does not end within 20 s is killed again at the next scan. A
`failed` session stays open after its write-back, so you can look at it,
and is ended, its window closed, at the start of the next scan (timed, at
start-up, a re-scan, or **Scan now**). Once a session is ended this way, the
round's run record (`runs/<run>/run.json`) is updated to say so (no session
alive, no process id); the run's own status is left as it was. The write
holds the round's folder against a run starting there meanwhile; if the
run or a session was still alive at that moment, it is done at the next
scan instead (at most 20 scans; then it is logged once and dropped, and
`run.json` may keep saying the session is alive).

A start-up dialog of Claude Code (workspace trust, a new MCP server) holds
the session before its message is typed. The app never answers it and types
nothing into it; the session window shows it, the session's own
notification event fires, and the tip gets one ❓ block per dialog kind and
round (not again while the same dialog stays open, nor when a dialog of a
kind already reported comes back; the tip shows ❓ again meanwhile). The
kind is read from the terminal screen as it is now (the session's live
grid), not from the order of the output. Answer it in the window: the
round then goes on, and its outcome is written as one more block, as for a
question answered in the window. Time at such a dialog does not count
toward the timeout (a dialog that keeps redrawing holds it off for at most
30 s at a time). A session that ends while a dialog was on its screen ends
the round `failed`.

Claude Code's workspace trust is per folder path, and every round has a new
folder, so you either answer the trust dialog in each round's window, or
trust the folder `<app data>\claude-graph\inbox` once (Claude Code then
trusts every folder below it). The once-only choice is acceptable because
of the checks above: nothing the Inbox runs in can be a link or junction to
somewhere else, and no folder between a round and the app's data folder may
carry `CLAUDE.md`, `.claude`, `.mcp.json` or `.git`. Either way Claude Code
adds one project entry per round folder to its own `~/.claude.json`; the
app does not touch that file.

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

- The generated graph of a round:
  `<app data>/claude-graph/inbox/rounds/<slug>-<hash>/<run id>/`
  (`%APPDATA%\claude-graph\inbox\rounds\…` on Windows), never inside the
  tips folder or OneDrive; `<run id>` is the round's `agent.run`. One node
  `task`, permission mode `claude_settings`, model inherit, default harness,
  terminal mode. Earlier rounds' folders (with their runs, deliverables and
  anything Claude Code wrote there) stay as they are; a new round starts
  with an empty deliverables folder and sees earlier rounds only through
  the tip body (their blocks). A round folder is deleted 15 days after the
  round finished (see *Old round folders*). Version 0.7.0 used one folder
  for all rounds of a tip, `<app data>/claude-graph/inbox/<slug>-<hash>/`;
  it is left as it is (never deleted, delete it by hand) and only checked
  for a run still active in it. Inbox graphs are not added to the recent
  list; the Inbox panel opens them.
- Path length: a round folder is
  `<app data>\claude-graph\inbox\rounds\<up to 57 chars>\<20 chars>\`,
  about 140 characters with a usual user name before anything the agent
  writes into `nodes\task\deliverables\`; deep deliverable trees can hit
  Windows' 260-character limit in tools that do not support long paths.
- The last scan time and the run ids this machine claimed with (last 500):
  `<app data>/claude-graph/inbox/state.json`.
- Results: `<tips folder>/agent/<tip id>/result.md` (the reply, copied last),
  `prompt.md` (the prompt the round ran with: the node prompt built from the
  tip, that is the fixed preamble plus the tip title and the seen body as
  data, exactly as written into the round's `graph.json`; the brief adds
  the node's goal, rules and local paths around it), plus the deliverables.
  `prompt.md` is this app's addition next to the files protocol 7.3 lists
  (the reply and the deliverables); the board ignores the folder anyway.
  Like every file there, `result.md` and `prompt.md` are the latest round's
  (overwritten by name, never deleted; earlier rounds' prompts stay in their
  round folders until those are deleted). A round whose run never started
  writes no `prompt.md`. Deliverables themselves named `result.md` or
  `prompt.md` are copied as `deliverable-result.md` /
  `deliverable-prompt.md`. Every file is written to a new temp file in the
  results folder and renamed over its name, so a hard link placed at a name
  is replaced, never written through; a source with other names (a hard
  link) or a link or junction is not copied and is reported.

## Old round folders

At each scan, before the tips are read, round folders under
`<app data>/claude-graph/inbox/rounds/<tip>/` are deleted 15 days after
their round finished (a fixed time; there is no setting). Only folders the
app made (named by a run id, `YYYYMMDD-HHMMSS-xxxx`) are touched, and never
one of a tip with a round starting, working, asking, or finished with its
session still open, nor one a run of the app or a live session is in. A
folder that is, or contains anywhere, a link or junction is not deleted at
all (nothing behind it is touched) and is listed in the panel's log. A tip
folder left empty is removed too. Errors are logged and never stop the
scan.

The finish time is the newest modification time of the round's run records
(`runs/*/run.json`). An agent could touch those only while its session
runs; a time before the round's start (from its folder name) or later than
now is not believed and the start is used instead, so a round folder cannot
be kept forever by moving that time ahead.

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

With no folder set, the panel suggests one (only at that moment: the
results are written relative to the tips folder, so nothing needs to know
where OneDrive is when a round is written back): the board's own `folder` setting
(`%APPDATA%\dev.zhangwentao.vindictive\settings.json`) if readable, else
`<OneDrive>\Vindictive` under the OneDrive roots the OneDrive client
sets in the environment (`OneDriveConsumer`, `OneDrive`,
`OneDriveCommercial`, in that order): the first root where that folder
already exists, else the first root. A suggestion is only shown; **Use it** or **Choose…**
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
  `.mcp.json`, `.claude` and `.git`, on the path the session really runs in
  (built from the resolved data folder, every level a plain folder, no link
  or junction). The app itself never creates any of them there.
- **The trust gate is not bypassed.** The generated graph has no elevated
  settings; it is checked before the claim as it will be written, and the
  run checks the written folder again when it starts. If it is not trusted,
  the tip is not started (before the claim) or reported failed (at the
  start) and the panel says why; no trust record is written for it.
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
  answered in the window, or the app exits; they have no timeout. A tip
  waiting at a start-up dialog is one of them. (Done sessions are closed
  after their write-back, failed ones at the next scan, see above.)
- **Start-up dialogs are recognised by their screen text** (the markers in
  `terminal/screen.rs`), and only before the node's message was typed. A
  dialog Claude Code draws differently in a future version shows as the
  generic `Claude Code start-up dialog`, or, if no marker matches at all,
  the tip stays plain running until it is answered. A second dialog of the
  same kind in one round (a second new MCP server) is not written again.
- **The 0.7.0 graph folders** (`<app data>/claude-graph/inbox/<slug>-<hash>/`)
  are never deleted; delete those of retired tips by hand. Round folders
  are deleted 15 days after their round (see above).
- **The last check is not the spawn.** The round folder is checked right
  before the run is started, but the session process is spawned a moment
  later by the run itself; a swap in that moment is not seen. Every level
  is a plain folder only the app and its own sessions write.
- The blank line before an appended block is ensured, not normalised: a
  body ending in several blank lines keeps them.
- Block markers in agent replies are defused case-sensitively
  (`agent:begin` / `agent:end` only).

## Code

`src-tauri/src/inbox/` (see the module docs in `mod.rs`): the pure protocol
core (`split`, `tip`, `hash`, `eligible`, `patch`, `block`, `ops`), the
folder I/O (`folder`, `writeback`, `results`, `scan`), orchestration
(`graphgen`, `runner`, `start`, `round`, `watch`, `startup`, `close`, `prune`,
`timer`), the file-system guards (`fsguard`) and the Tauri side (`gui`).
IPC types: `src-tauri/src/contract_inbox.rs` / `src/types/contract.ts`.
Frontend: `src/store/inboxStore.ts`, `src/components/InboxPanel.tsx`,
`src/lib/inboxApi.ts`. End-to-end tests with the fake CLI in a real pty:
`src-tauri/tests/e2e_inbox.rs` and `src-tauri/tests/e2e_inbox_startup.rs`
(a start-up dialog).
