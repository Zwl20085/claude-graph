#!/usr/bin/env node
// Fake `claude -p --output-format stream-json` for tests and demos (docs/ARCHITECTURE.md §8).
//
//   CLAUDE_GRAPH_CLAUDE_BIN="node scripts/fake-claude.mjs"
//
// Reads the node prompt from stdin, prints a plausible stream-json sequence
// (system init, assistant text, assistant tool_use Write, user tool_result,
// assistant final text, result), and writes a stub for every deliverable listed
// under "## Deliverables (required)" into the deliverables folder named in the
// prompt. If the prompt contains FAKE_FAIL the final text ends with
// "## Status: BLOCKED" (exit code still 0).
//
// `--version` prints a version line and exits. Optional env FAKE_CLAUDE_DELAY_MS
// (or arg `--fake-delay-ms N`) adds a pause between lines (useful to watch or
// cancel a run). `--resume <id>` reuses that session id.
import { randomUUID } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, normalize, resolve, sep } from 'node:path';

const VERSION = '2.1.0 (fake-claude)';
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
    return;
  }
  const started = Date.now();
  const prompt = await readStdin();
  // `--resume <id>` continues that session, as the real CLI does.
  const sessionId = argValue('--resume') ?? randomUUID();
  const model = argValue('--model') ?? 'sonnet';
  const cwd = process.cwd();
  const fail = prompt.includes('FAKE_FAIL');
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
  const finalText = fail
    ? `Could not finish.\n\n## Deliverables\n${list}\n## Status: BLOCKED\nFAKE_FAIL was present in the prompt, so this fake run reports BLOCKED.`
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
    permission_denials: [],
  });
}

main().then(
  () => {
    process.exitCode = 0;
  },
  (err) => {
    process.stderr.write(`fake-claude: ${err?.stack ?? err}\n`);
    process.exitCode = 1;
  },
);
