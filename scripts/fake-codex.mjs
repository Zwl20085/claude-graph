#!/usr/bin/env node
// Fake OpenAI Codex CLI (interactive TUI) for the terminal-mode tests
// (docs/ARCHITECTURE.md "Harness profiles"). It imitates what the app reads
// from the real Codex 0.16x with `--no-alt-screen`:
//
// - `--version` prints a version line and exits.
// - `-C <dir>` changes the working directory; `-m <model>` is shown in the
//   banner; `-c notify=[...]` (a TOML array of basic strings, which parses as
//   JSON) names the program run after every turn with ONE JSON argument
//   appended, like Codex does.
// - A banner, then the composer: a row `› Ask Codex to do anything` (the
//   placeholder). Input is read in raw mode; typed characters are drawn in the
//   composer row, Ctrl+U (0x15) clears it, a bracketed paste (ESC[200~ …
//   ESC[201~) is one message, Enter submits.
// - On submit: the message stays in the history as `› <text>`, a
//   `• Working (0s • esc to interrupt)` row appears with the composer
//   placeholder below it; the message is appended to `fake-received.log`
//   next to the notify target (else in the cwd), a brief named by
//   `read the brief at <path>` gets its deliverables written (unless the
//   message or brief contains FAKE_FAIL; FAKE_SLEEP_MS=<n> waits first); then
//   the reply `• done (n file(s) written)` and the empty composer. A turn
//   takes at least 1.5 s.
// - Notify: on the thread's first message, right after "Working" shows, for
//   a side turn (a thread-title request, as the real Codex sends; other
//   `input-messages`), so the app must not take it for the turn's end; after
//   the reply, for the turn itself. Payloads hold quotes, `&`, `|` and
//   newlines.
//
// Switches (argv, or one per line / space-separated in `fake-codex-args.txt`
// in the working directory): `--fake-dialog-ms=<n>` shows the folder-trust
// dialog ("Folder access / Trust this folder? / Continue only if you trust
// these files / Trust and continue / Quit") for n ms, discarding input;
// `--fake-no-notify` never runs the notify program.
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, normalize, resolve, sep } from 'node:path';

const VERSION = 'codex-cli 0.163.0 (fake-codex)';
const PLACEHOLDER = 'Ask Codex to do anything';
const GLYPH = '\u203a';
const PASTE_START = '\u001b[200~';
const PASTE_END = '\u001b[201~';

const argValue = (name) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = (text) => process.stdout.write(text);

function notifyArgv() {
  for (let i = 0; i < process.argv.length - 1; i += 1) {
    if (process.argv[i] !== '-c') continue;
    const value = process.argv[i + 1];
    if (!value.startsWith('notify=')) continue;
    const list = JSON.parse(value.slice('notify='.length));
    if (Array.isArray(list) && list.length > 0 && list.every((s) => typeof s === 'string')) return list;
  }
  return null;
}

function extraArgs() {
  try {
    return readFileSync(join(process.cwd(), 'fake-codex-args.txt'), 'utf8').split(/\s+/).filter(Boolean);
  } catch {
    return [];
  }
}

function findDeliverablesDir(brief) {
  const m = brief.match(/^Your deliverables folder:\s*(.+?)\s*(?:\(write ALL outputs here\))?\s*$/m);
  return m ? m[1].trim() : null;
}

function findDeliverables(brief) {
  const lines = brief.split(/\r?\n/);
  const start = lines.findIndex((l) => l.trim() === '## Deliverables (required)');
  if (start < 0) return [];
  const found = [];
  for (const line of lines.slice(start + 1)) {
    if (line.startsWith('## ')) break;
    const m = line.match(/^\s*-\s+(.+?)(?:\s+—\s+(.*))?\s*$/);
    if (m) found.push(m[1].trim());
  }
  return found;
}

function safeJoin(root, rel) {
  if (isAbsolute(rel)) return null;
  const base = resolve(root);
  const full = resolve(base, normalize(rel));
  return full === base || !full.startsWith(base + sep) ? null : full;
}

function runNotify(argv, payload) {
  const [program, ...args] = argv;
  const r = spawnSync(program, [...args, JSON.stringify(payload)], { stdio: 'ignore', windowsHide: true });
  if (r.error) process.stderr.write(`fake-codex: notify failed: ${r.error.message}\n`);
}

const composer = (text) => `\r\u001b[2K${GLYPH} ${text || PLACEHOLDER}`;

async function handle(text, ctx) {
  appendFileSync(ctx.received, `${text.replace(/\n/g, '\\n')}\n`, 'utf8');
  out(`${composer(text.split('\n')[0])}\r\n\r\n\u2022 Working (0s \u2022 esc to interrupt)\r\n\r\n${composer('')}`);
  if (ctx.notify && !ctx.titled) {
    // The side turn finishes first, while the real turn still works.
    ctx.titled = true;
    runNotify(ctx.notify, {
      type: 'agent-turn-complete',
      'thread-id': randomUUID(),
      'input-messages': [`Generate a concise title. Do not answer.\n\nUser prompt:\n${text}`],
      'last-assistant-message': '{"title":"said \\"ok\\" & then | more\\nsecond line"}',
    });
  }
  const briefPath = text.match(/read the brief at (.+?brief\.md)/)?.[1];
  const brief = briefPath ? readFileSync(briefPath, 'utf8') : '';
  const all = `${text}\n${brief}`;
  const sleepMs = Number(all.match(/FAKE_SLEEP_MS=(\d+)/)?.[1] ?? 0);
  await sleep(Math.max(sleepMs, 1500));
  let written = 0;
  if (brief && !all.includes('FAKE_FAIL')) {
    const dir = findDeliverablesDir(brief);
    for (const d of findDeliverables(brief)) {
      const target = dir ? safeJoin(dir, d) : null;
      if (!target) continue;
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, `# ${d}\n\nStub written by fake-codex.\n`, 'utf8');
      written += 1;
    }
  }
  // FAKE_QUESTION: the reply asks the user (screen and notify payload).
  const asks = all.includes('FAKE_QUESTION');
  const shown = asks ? 'Which framework should I use? 1. vitest 2. jest' : `done (${written} file(s) written)`;
  out(`\r\u001b[2K\u2022 ${shown}\r\n\r\n  Worked for 0s\r\n\r\n${composer('')}`);
  if (ctx.notify) {
    runNotify(ctx.notify, {
      type: 'agent-turn-complete',
      'thread-id': ctx.thread,
      'input-messages': [text],
      'last-assistant-message': asks
        ? 'Which framework should I use?\n1. vitest\n2. jest'
        : `done (${written} file(s) written); said "ok" & then | more\nsecond line`,
    });
  }
}

async function interactive() {
  const dir = argValue('-C');
  if (dir) process.chdir(dir);
  const args = [...process.argv, ...extraArgs()];
  const dialogMs = Number(args.find((a) => a.startsWith('--fake-dialog-ms='))?.split('=')[1] ?? 0) || 0;
  const notify = args.includes('--fake-no-notify') ? null : notifyArgv();
  const ctx = {
    notify,
    thread: randomUUID(),
    titled: false,
    received: join(notify ? dirname(notify[notify.length - 1]) : process.cwd(), 'fake-received.log'),
  };
  if (process.stdin.isTTY) process.stdin.setRawMode(true);
  out(`  >_ Fake Codex (${VERSION})\r\n     ${process.cwd()}\r\n  model: ${argValue('-m') ?? 'default'}\r\n\r\n`);
  let dialogUntil = 0;
  if (dialogMs > 0) {
    dialogUntil = Date.now() + dialogMs;
    out('  Folder access\r\n\r\n  Trust this folder?\r\n  Continue only if you trust these files\r\n\r\n');
    out(`${GLYPH} 1. Trust and continue\r\n  2. Quit\r\n`);
    setTimeout(() => out(`\r\n${composer('')}`), dialogMs);
  } else {
    out(composer(''));
  }
  let buffer = '';
  let pasting = false;
  let busy = Promise.resolve();
  process.stdin.on('data', (chunk) => {
    if (Date.now() < dialogUntil) return;
    let data = chunk.toString('utf8');
    for (;;) {
      if (!pasting && data.includes(PASTE_START)) {
        const i = data.indexOf(PASTE_START);
        busy = busy.then(() => feed(data.slice(0, i)));
        data = data.slice(i + PASTE_START.length);
        pasting = true;
        continue;
      }
      if (pasting && data.includes(PASTE_END)) {
        const i = data.indexOf(PASTE_END);
        buffer += data.slice(0, i).replace(/\r\n?/g, '\n');
        data = data.slice(i + PASTE_END.length);
        pasting = false;
        out(composer(buffer.split('\n')[0]));
        continue;
      }
      break;
    }
    if (pasting) {
      buffer += data.replace(/\r\n?/g, '\n');
      return;
    }
    busy = busy.then(() => feed(data));
  });
  async function feed(data) {
    for (const ch of data.replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, '')) {
      if (ch === '\u0003') process.exit(0);
      if (ch === '\u0015') {
        buffer = '';
        out(composer(''));
      } else if (ch === '\r' || ch === '\n') {
        const text = buffer.trim();
        buffer = '';
        if (text) await handle(text, ctx);
      } else if (ch >= ' ') {
        buffer += ch;
        out(composer(buffer.split('\n')[0]));
      }
    }
  }
  await new Promise(() => {});
}

if (process.argv.includes('--version')) {
  out(`${VERSION}\n`);
} else {
  interactive().catch((err) => {
    process.stderr.write(`fake-codex: ${err?.stack ?? err}\n`);
    process.exitCode = 1;
  });
}
