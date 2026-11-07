#!/usr/bin/env node
// Fake `claude -p --output-format stream-json` for tests and demos (docs/ARCHITECTURE.md §8).
//
//   CLAUDE_GRAPH_CLAUDE_BIN="node scripts/fake-claude.mjs"
//
// Reads the node prompt from stdin, prints a plausible stream-json sequence
// (system init, assistant text, assistant tool_use Write, user tool_result,
// assistant final text, result), and writes a stub for every deliverable listed
// under "## Deliverables (required)" into the deliverables folder named in the
// prompt. Prompt markers (node mode): FAKE_BLOCKED ends the final text with
// "## Status: BLOCKED" and a question line (exit code 0); FAKE_FAIL ends with an
// error result and exit code 1 (a real failure); FAKE_DENY adds a WebSearch
// tool_use, the CLI's `system`/`permission_denied` event for it, and a
// `permission_denials` entry in the result.
//
// `--version` prints a version line and exits. Optional env FAKE_CLAUDE_DELAY_MS
// (or arg `--fake-delay-ms N`) adds a pause between lines (useful to watch or
// cancel a run). `--resume <id>` reuses that session id unless `--fork-session`
// is also given (then a new id, as the real CLI does). A prompt containing
// FAKE_NO_STATUS ends without any "## Status:" line.
// `--hold-stdout-ms N` starts a child process that inherits stdout and sleeps
// N ms, so the pipe stays open after this process exits (tests the runner's
// drain grace and leftover kill).
//
// Scheduler mode (assistant mode): when the `-p` instruction starts with
// "You are the work scheduler", the fake plays the work scheduler instead. It
// writes `assistant/PLAN.md` in its cwd and replies in the charter's sections:
// by default a note, `## Stage: <first node id>`, a `## Decision:` (with a
// `### detail` sub-heading in its body) and a `## Question`; when the prompt
// contains FAKE_FINAL a stage and a `## Final`. Prompt markers:
// FAKE_SLEEP_MS=<n> sleeps n ms before replying and then writes
// `assistant/slept.txt` (a cancelled turn never writes it); FAKE_ERROR ends
// with an error result; FAKE_DENY reports `permission_denials` for Bash.
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, normalize, resolve, sep } from 'node:path';

const VERSION = '2.1.0 (fake-claude)';
const SCHEDULER_PREFIX = 'You are the work scheduler';
const argValue = (name) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
};

// `--fake-delay-ms N` (tests) overrides env FAKE_CLAUDE_DELAY_MS.
const DELAY_MS = Number(argValue('--fake-delay-ms') ?? process.env.FAKE_CLAUDE_DELAY_MS ?? 0) || 0;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Write one JSON line and wait until it has been handed to the OS. */
const emit = (obj) =>
  new Promise((res, rej) => {
    process.stdout.write(`${JSON.stringify(obj)}\n`, (err) => (err ? rej(err) : res()));
  }).then(() => (DELAY_MS ? sleep(DELAY_MS) : undefined));

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString('utf8');
}

function findDeliverablesDir(prompt) {
  const m = prompt.match(/^Your deliverables folder:\s*(.+?)\s*(?:\(write ALL outputs here\))?\s*$/m);
  return m ? m[1].trim() : null;
}

/** Lines "- <path> — <description>" under "## Deliverables (required)". */
function findDeliverables(prompt) {
  const lines = prompt.split(/\r?\n/);
  const start = lines.findIndex((l) => l.trim() === '## Deliverables (required)');
  if (start < 0) return [];
  const out = [];
  for (const line of lines.slice(start + 1)) {
    if (line.startsWith('## ')) break;
    const m = line.match(/^\s*-\s+(.+?)(?:\s+—\s+(.*))?\s*$/);
    if (m) out.push({ path: m[1].trim(), description: (m[2] ?? '').trim() });
  }
  return out;
}

/** Resolve a deliverable path inside `root`, refusing absolute or escaping paths. */
function safeJoin(root, rel) {
  if (isAbsolute(rel)) return null;
  const base = resolve(root);
  const full = resolve(base, normalize(rel));
  return full === base || !full.startsWith(base + sep) ? null : full;
}

async function main() {
  if (process.argv.includes('--version')) {
    process.stdout.write(`${VERSION}\n`);
    return 0;
  }
  const started = Date.now();
  const prompt = await readStdin();
  // `--resume <id>` continues that session, as the real CLI does; `--fork-session` branches it.
  const forked = process.argv.includes('--fork-session');
  const sessionId = (!forked && argValue('--resume')) || randomUUID();
  const holdMs = Number(argValue('--hold-stdout-ms') ?? 0) || 0;
  if (holdMs > 0) {
    spawn(process.execPath, ['-e', `setTimeout(() => {}, ${holdMs})`], {
      stdio: ['ignore', 'inherit', 'ignore'],
      windowsHide: true,
    }).unref();
  }
  if ((argValue('-p') ?? '').startsWith(SCHEDULER_PREFIX)) {
    await scheduler(prompt, sessionId, started);
    return 0;
  }
  const model = argValue('--model') ?? 'sonnet';
  const cwd = process.cwd();
  const fail = prompt.includes('FAKE_FAIL');
  const blocked = prompt.includes('FAKE_BLOCKED');
  const deny = prompt.includes('FAKE_DENY');
  const dir = findDeliverablesDir(prompt);
  const deliverables = findDeliverables(prompt);
  const base = { session_id: sessionId, parent_tool_use_id: null };

  await emit({
    type: 'system',
    subtype: 'init',
    session_id: sessionId,
    cwd,
    model,
    tools: ['Read', 'Write', 'Edit', 'Bash'],
    permissionMode: argValue('--permission-mode') ?? 'default',
    claude_code_version: VERSION,
  });
  await emit({
    type: 'assistant',
    ...base,
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: `Reading the brief (${prompt.length} chars) and writing ${deliverables.length} deliverable(s).` }],
      stop_reason: 'tool_use',
    },
  });

  const denials = deny ? await deniedSearch(base, sessionId) : [];
  const written = [];
  for (const d of deliverables) {
    const target = dir ? safeJoin(dir, d.path) : null;
    if (!target) continue;
    const toolId = `toolu_${randomUUID().replace(/-/g, '').slice(0, 24)}`;
    const content = `# ${d.path}\n\nStub written by fake-claude.\n\n${d.description}\n`;
    await emit({
      type: 'assistant',
      ...base,
      message: {
        role: 'assistant',
        content: [{ type: 'tool_use', id: toolId, name: 'Write', input: { file_path: target, content } }],
        stop_reason: 'tool_use',
      },
    });
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, content, 'utf8');
    written.push(d);
    await emit({
      type: 'user',
      session_id: sessionId,
      parent_tool_use_id: null,
      message: {
        role: 'user',
        content: [{ type: 'tool_result', tool_use_id: toolId, content: `File created successfully at: ${target}`, is_error: false }],
      },
    });
  }

  const list = written.map((d) => `- ${d.path} — ${d.description || 'stub'}`).join('\n');
  const noStatus = prompt.includes('FAKE_NO_STATUS');
  if (fail) {
    const duration = Date.now() - started;
    await emit({
      type: 'result',
      subtype: 'error_during_execution',
      session_id: sessionId,
      is_error: true,
      errors: ['FAKE_FAIL was present in the prompt'],
      num_turns: 1 + written.length,
      duration_ms: duration,
      total_cost_usd: 0.0123,
      permission_denials: denials,
    });
    return 1;
  }
  const finalText = noStatus
    ? 'Answered the message without a status line.'
    : blocked
    ? `Could not finish.\n\n## Deliverables\n${list}\n## Status: BLOCKED\nFAKE_BLOCKED was present in the prompt.\nMay I continue without web access? (yes / no)`
    : `All deliverables written.\n\n## Deliverables\n${list}\n## Status: DONE`;
  await emit({
    type: 'assistant',
    ...base,
    message: { role: 'assistant', content: [{ type: 'text', text: finalText }], stop_reason: 'end_turn' },
  });

  const duration = Date.now() - started;
  await emit({
    type: 'result',
    subtype: 'success',
    session_id: sessionId,
    is_error: false,
    result: finalText,
    stop_reason: 'end_turn',
    num_turns: 2 + written.length,
    duration_ms: duration,
    duration_api_ms: duration,
    total_cost_usd: 0.0123,
    usage: {},
    modelUsage: {},
    permission_denials: denials,
  });
  return 0;
}

/** FAKE_DENY: a WebSearch call the CLI refuses, streamed as the real CLI does. */
async function deniedSearch(base, sessionId) {
  const id = 'toolu_fake_denied_search';
  const input = { query: 'fake denied search', mode: 'standard' };
  const message = "Claude requested permissions to use WebSearch, but you haven't granted it yet.";
  await emit({
    type: 'assistant',
    ...base,
    message: { role: 'assistant', content: [{ type: 'tool_use', id, name: 'WebSearch', input }], stop_reason: 'tool_use' },
  });
  await emit({ type: 'system', subtype: 'permission_denied', tool_name: 'WebSearch', tool_use_id: id, message, session_id: sessionId });
  await emit({
    type: 'user',
    session_id: sessionId,
    parent_tool_use_id: null,
    message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content: message, is_error: true }] },
  });
  return [{ tool_name: 'WebSearch', tool_use_id: id, tool_input: input }];
}

/** Scheduler mode: see the header comment. */
async function scheduler(prompt, sessionId, started) {
  const cwd = process.cwd();
  const base = { session_id: sessionId, parent_tool_use_id: null };
  await emit({
    type: 'system',
    subtype: 'init',
    session_id: sessionId,
    cwd,
    model: 'claude-fake-scheduler',
    tools: ['Read', 'Write', 'Edit', 'Bash', 'Task'],
    permissionMode: 'default',
    claude_code_version: VERSION,
  });
  const plan = join(cwd, 'assistant', 'PLAN.md');
  await emit({
    type: 'assistant',
    ...base,
    message: {
      role: 'assistant',
      content: [
        { type: 'text', text: `Planning (${prompt.length} chars of prompt).` },
        { type: 'tool_use', id: 'toolu_plan', name: 'Write', input: { file_path: plan } },
      ],
      stop_reason: 'tool_use',
    },
  });
  mkdirSync(dirname(plan), { recursive: true });
  writeFileSync(plan, '# Plan\n\nStub written by fake-claude (scheduler mode).\n', 'utf8');

  const sleepMs = Number(prompt.match(/FAKE_SLEEP_MS=(\d+)/)?.[1] ?? 0);
  if (sleepMs > 0) {
    await sleep(sleepMs);
    writeFileSync(join(cwd, 'assistant', 'slept.txt'), 'not cancelled\n', 'utf8');
  }
  const duration = () => Date.now() - started;
  if (prompt.includes('FAKE_ERROR')) {
    await emit({
      type: 'result',
      subtype: 'error_during_execution',
      session_id: sessionId,
      is_error: true,
      errors: ['fake scheduler error'],
      num_turns: 1,
      duration_ms: duration(),
      total_cost_usd: 0.001,
    });
    return;
  }
  const node = prompt.match(/^### Node `([^`]+)`/m)?.[1] ?? 'plan';
  const finalText = prompt.includes('FAKE_FINAL')
    ? `## Stage: ${node}\nFinished after the answer.\n\n## Final\nAll nodes done; see nodes/*/deliverables/.`
    : `Starting the workflow.\n\n## Stage: ${node}\nWrote the plan to assistant/PLAN.md.\n\n` +
      '## Decision: Order\nRun nodes in topological order.\n### detail\nkept in the decision body\n\n' +
      '## Question\nProceed with the remaining nodes? (yes / no)';
  await emit({
    type: 'assistant',
    ...base,
    message: { role: 'assistant', content: [{ type: 'text', text: finalText }], stop_reason: 'end_turn' },
  });
  const denials = prompt.includes('FAKE_DENY')
    ? [{ tool_name: 'Bash', tool_use_id: 'toolu_denied', tool_input: { command: 'make' } }]
    : [];
  await emit({
    type: 'result',
    subtype: 'success',
    session_id: sessionId,
    is_error: false,
    result: finalText,
    stop_reason: 'end_turn',
    num_turns: 3,
    duration_ms: duration(),
    duration_api_ms: duration(),
    total_cost_usd: 0.0042,
    usage: {},
    modelUsage: {},
    permission_denials: denials,
  });
}

main().then(
  (code) => {
    process.exitCode = code ?? 0;
  },
  (err) => {
    process.stderr.write(`fake-claude: ${err?.stack ?? err}\n`);
    process.exitCode = 1;
  },
);
