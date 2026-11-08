#!/usr/bin/env node
// Fake DeepSeek Harness CLI (`dsh`) for tests: the headless profile only, with
// the run-event schema verified with dsh 0.2.1-rc.2 on 2026-11-08
// (docs/CLAUDE_CLI_NOTES.md "dsh"). Use it through
//   CLAUDE_GRAPH_DSH_BIN="node scripts/fake-dsh.mjs"
//
// `dsh --version` prints a version. `dsh --profile <p> [--json]
// [--session-id <id>] [task…|-]` answers one task (`-` or no task: stdin)
// and exits 0; with --json, stdout is one JSON event per line: session,
// status (turn_start, step_start, step_end, turn_end), thinking, text,
// tool_call, tool_result, final. Sessions persist in `<cwd>/.fake-dsh/<id>.json`;
// an unknown --session-id is an `error` event, "dsh: …" on stderr and exit 1.
// Every task is appended to `<cwd>/fake-dsh-received.log` (newlines as \n).
//
// Markers in the task (or the brief it points at with "read the brief at"):
// - FAKE_QUESTION: the answer ends with "Which framework should I use?" and
//   "1. vitest / 2. jest"; the next task of that session is answered
//   "Thanks, using <task>.".
// - FAKE_FAIL: an error event and exit 1 (no deliverables).
// - FAKE_APPROVAL: a write outside the workspace that headless dsh denies
//   ("requires approval, but no approval channel is available").
// - REMEMBER=<word> stores a word in the session; RECALL answers with it.
// - FAKE_SLOW: the answer comes after 1.5 s.
// - FAKE_BACKGROUND: starts a background process that inherits stdout and
//   stderr and lives 60 s (a tool's `npm run dev &`), then exits normally.
// A brief's deliverables are written (as tool_call / tool_result pairs).
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, normalize, resolve, sep } from 'node:path';

const VERSION = '0.2.1-rc.2 (fake-dsh)';
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const value = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString('utf8');
}

function taskWords() {
  const skip = new Set(['--profile', '--session-id']);
  const words = [];
  for (let i = 0; i < args.length; i += 1) {
    if (skip.has(args[i])) i += 1;
    else if (!args[i].startsWith('--')) words.push(args[i]);
  }
  return words;
}

function deliverablesOf(brief) {
  const dir = brief.match(/^Your deliverables folder:\s*(.+?)\s*(?:\(write ALL outputs here\))?\s*$/m)?.[1]?.trim();
  const lines = brief.split(/\r?\n/);
  const start = lines.findIndex((l) => l.trim() === '## Deliverables (required)');
  const out = [];
  if (start >= 0) {
    for (const line of lines.slice(start + 1)) {
      if (line.startsWith('## ')) break;
      const m = line.match(/^\s*-\s+(.+?)(?:\s+—\s+(.*))?\s*$/);
      if (m) out.push(m[1].trim());
    }
  }
  return { dir, paths: out };
}

function safeJoin(root, rel) {
  if (isAbsolute(rel)) return null;
  const base = resolve(root);
  const full = resolve(base, normalize(rel));
  return full === base || !full.startsWith(base + sep) ? null : full;
}

async function main() {
  if (flag('--version') || flag('-V')) {
    process.stdout.write(`${VERSION}\n`);
    return 0;
  }
  const profile = value('--profile');
  if (profile !== 'headless') {
    process.stderr.write(`dsh: profile ${profile} is not installed (fake-dsh knows headless only)\n`);
    return 1;
  }
  const json = flag('--json');
  const words = taskWords();
  const task = (words.length === 0 || words[0] === '-' ? await readStdin() : words.join(' ')).trim();
  const store = join(process.cwd(), '.fake-dsh');
  mkdirSync(store, { recursive: true });
  appendFileSync(join(process.cwd(), 'fake-dsh-received.log'), `${task.replace(/\r?\n/g, '\\n')}\n`, 'utf8');
  const emit = (obj) => {
    if (json) process.stdout.write(`${JSON.stringify(obj)}\n`);
  };
  const adopt = value('--session-id');
  const sessionId = adopt ?? `session-${randomUUID()}`;
  const file = join(store, `${sessionId}.json`);
  if (adopt !== undefined && !existsSync(file)) {
    const message = `session "${adopt}" does not exist; omit --session-id to start a new Session`;
    emit({ type: 'error', message });
    process.stderr.write(`dsh: ${message}\n`);
    return 1;
  }
  const session = existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : { turns: 0, asked: false, memory: null };
  const turn = session.turns + 1;
  emit({ type: 'session', sessionId, cwd: process.cwd() });
  emit({ type: 'status', phase: 'turn_start', turn });
  emit({ type: 'status', phase: 'step_start', turn, step: 1 });
  emit({ type: 'thinking', text: '' });

  const briefPath = task.match(/read the brief at (.+?brief\.md)/)?.[1];
  const brief = briefPath && existsSync(briefPath) ? readFileSync(briefPath, 'utf8') : '';
  const all = `${task}\n${brief}`;
  let final;
  let code = 0;
  if (all.includes('FAKE_FAIL')) {
    const message = 'model request failed (FAKE_FAIL)';
    emit({ type: 'error', message });
    process.stderr.write(`dsh: provider_error: ${message}\n`);
    code = 1;
  } else {
    let call = 0;
    const tool = (name, input, status, result) => {
      call += 1;
      const callId = `call_${String(call).padStart(2, '0')}_fake`;
      emit({ type: 'tool_call', callId, tool: name, input });
      emit({ type: 'tool_result', callId, status, result });
    };
    const { dir, paths } = deliverablesOf(brief);
    for (const p of paths) {
      const target = dir ? safeJoin(dir, p) : null;
      if (!target) continue;
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, `# ${p}\n\nStub written by fake-dsh.\n`, 'utf8');
      tool('write', { file_path: target, content: '…' }, 'completed', `<path>${target}</path>\nCreated file`);
    }
    if (all.includes('FAKE_APPROVAL')) {
      tool(
        'write',
        { file_path: '/outside/x.txt', content: 'hi', sandbox_permissions: 'danger-full-access' },
        'error',
        'Error: sandbox escalation to "danger-full-access" requires approval, but no approval channel is available',
      );
    }
    if (all.includes('FAKE_SLOW')) await new Promise((done) => setTimeout(done, 1500));
    if (all.includes('FAKE_BACKGROUND')) {
      const bg = spawn(process.execPath, ['-e', 'setTimeout(() => {}, 60000)'], {
        stdio: ['ignore', 'inherit', 'inherit'],
        windowsHide: true,
      });
      bg.unref();
    }
    const remember = all.match(/REMEMBER=(\w+)/)?.[1];
    if (remember) session.memory = remember;
    if (session.asked) {
      session.asked = false;
      final = `Thanks, using ${task.split('\n')[0]}.`;
    } else if (all.includes('RECALL')) {
      final = session.memory ?? '(nothing remembered)';
    } else if (all.includes('FAKE_QUESTION')) {
      session.asked = true;
      final = 'I can set this up.\n\nWhich framework should I use?\n1. vitest\n2. jest';
    } else {
      final = `done (${paths.length} file(s) written)`;
    }
    emit({ type: 'text', text: final });
    emit({ type: 'status', phase: 'step_end', turn, step: 1, usage: { inputTokens: 10, outputTokens: 2, totalTokens: 12 } });
    emit({ type: 'status', phase: 'turn_end', turn, reason: { kind: 'completed' } });
    emit({ type: 'final', text: final });
    if (!json) process.stdout.write(`${final}\n`);
  }
  session.turns = turn;
  writeFileSync(file, JSON.stringify(session), 'utf8');
  return code;
}

main().then(
  (code) => {
    process.exitCode = code ?? 0;
  },
  (err) => {
    process.stderr.write(`dsh: internal: ${err?.stack ?? err}\n`);
    process.exitCode = 1;
  },
);
