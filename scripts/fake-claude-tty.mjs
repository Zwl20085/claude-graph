// Interactive mode of scripts/fake-claude.mjs (terminal mode): plays an
// interactive Claude Code in a pty. See the header of fake-claude.mjs for
// the delivery switches. Hooks get Claude Code's stdin payloads (verified
// field names, docs/CLAUDE_CLI_NOTES.md "Turn outcomes"): every payload has
// `session_id`, `transcript_path` (a fake JSONL next to the settings file),
// `cwd`, `hook_event_name`; UserPromptSubmit adds `prompt`; Stop adds
// `stop_hook_active` and `last_assistant_message`.
//
// Turn-outcome markers (in the prompt or the brief):
// - FAKE_QUESTION: the reply ends with "Which framework should I use?" and
//   "1. vitest / 2. jest"; the next prompt is answered "Thanks, using <it>."
// - FAKE_DIALOG: before replying, an AskUserQuestion-style dialog (question,
//   three numbered options with ❯ on the first, "Enter to select · ↑/↓ to
//   navigate · Esc to cancel"); Up/Down move, a digit selects and submits
//   (the "Type something." digit only moves there; typed text then Enter),
//   Enter selects the highlighted option; then "You chose <label>." and Stop.
//   The assistant `tool_use` entry is written to the fake transcript before
//   the dialog is drawn, its `tool_result` after the answer.
// - FAKE_PERMISSION: a "Do you want to proceed?" prompt with "1. Yes /
//   2. Yes, and don't ask again / 3. No" for a Bash call; a digit or Enter
//   answers, then the turn goes on as usual.
// - FAKE_OFFER: deliverables are written and the reply ends with "Want me to
//   also write tests?".
import { execSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

const BOX = '\n>\u00a0';
const PASTE_START = '\u001b[200~';
const PASTE_END = '\u001b[201~';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** argv plus the switches in `fake-claude-args.txt` in the cwd. */
function interactiveArgs() {
  let extra = [];
  try {
    extra = readFileSync(join(process.cwd(), 'fake-claude-args.txt'), 'utf8').split(/\s+/).filter(Boolean);
  } catch {
    // no file: argv only
  }
  return [...process.argv, ...extra];
}

const argValue = (name) => {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
};

const out = (text) => process.stdout.write(text.replace(/\r?\n/g, '\r\n'));

/** Keys of a raw-mode chunk: escape sequences, Enter, other characters. */
function keysOf(chunk) {
  const keys = [];
  let i = 0;
  while (i < chunk.length) {
    if (chunk[i] === '\u001b' && (chunk[i + 1] === '[' || chunk[i + 1] === 'O') && i + 2 < chunk.length) {
      keys.push(chunk.slice(i, i + 3));
      i += 3;
    } else {
      keys.push(chunk[i]);
      i += 1;
    }
  }
  return keys;
}

/** The session: hooks, the fake transcript, the input state. */
function makeSession(helpers) {
  const args = interactiveArgs();
  const settingsPath = argValue('--settings');
  const settings = settingsPath ? JSON.parse(readFileSync(settingsPath, 'utf8')) : {};
  const base = settingsPath ? dirname(settingsPath) : process.cwd();
  const sessionId = randomUUID();
  const transcript = join(base, 'fake-transcript.jsonl');
  const runHooks = (event, extra = {}) => {
    const payload = JSON.stringify({
      session_id: sessionId,
      transcript_path: transcript,
      cwd: process.cwd(),
      hook_event_name: event,
      ...extra,
    });
    for (const matcher of settings?.hooks?.[event] ?? []) {
      for (const h of matcher.hooks ?? []) {
        if (h.type === 'command') {
          execSync(h.command, { input: payload, stdio: ['pipe', 'ignore', 'ignore'], windowsHide: true });
        }
      }
    }
  };
  const log = (entry) => appendFileSync(transcript, `${JSON.stringify({ sessionId, ...entry })}\n`, 'utf8');
  return {
    helpers,
    runHooks,
    log,
    received: join(base, 'fake-received.log'),
    dialogMs: Number(args.find((a) => a.startsWith('--fake-dialog-ms='))?.split('=')[1] ?? 0) || 0,
    swallow: args.includes('--fake-swallow-first'),
    dropEnter: args.includes('--fake-drop-enter'),
    held: null,
    dialogUntil: 0,
    /** FAKE_QUESTION asked: the next prompt is its answer. */
    asked: false,
    /** A dialog waiting for keys: { draw, onKey } (raw mode while set). */
    dialog: null,
    paste: null,
    pending: '',
  };
}

function assistantText(s, text) {
  s.log({ type: 'assistant', message: { role: 'assistant', content: [{ type: 'text', text }] } });
}

/** The prompt an input line submits, or null (swallowed, held, empty). */
function accept(text, s) {
  if (s.swallow) {
    s.swallow = false;
    out(BOX);
    return null;
  }
  if (s.dropEnter && text) {
    s.held = text;
    out(`${BOX}${text.split('\n')[0]}`);
    return null;
  }
  const held = s.held;
  s.held = null;
  if (!text && held === null) {
    out(BOX);
    return null;
  }
  return text || held;
}

/** Show a choice dialog and resolve with the chosen label (raw keys). */
function choiceDialog(s, { header, question, options, footer, textOption }) {
  return new Promise((resolveChoice) => {
    let cursor = 0;
    let typed = '';
    const draw = () => {
      const rows = ['\u001b[2J\u001b[H'];
      if (header) rows.push(` \u2610 ${header}`);
      rows.push(question);
      options.forEach((o, i) => {
        const label = i === textOption && typed ? typed : o.label;
        rows.push(`${i === cursor ? '\u276f' : ' '} ${i + 1}. ${label}`);
        if (o.detail) rows.push(`     ${o.detail}`);
      });
      rows.push('', footer, '');
      out(rows.join('\n'));
    };
    const finish = (label) => {
      s.dialog = null;
      if (process.stdin.isTTY) process.stdin.setRawMode(false);
      out('\u001b[2J\u001b[H');
      resolveChoice(label);
    };
    const onKey = (k) => {
      if (k === '\u001b[A' || k === '\u001bOA') cursor = Math.max(0, cursor - 1);
      else if (k === '\u001b[B' || k === '\u001bOB') cursor = Math.min(options.length - 1, cursor + 1);
      else if (k === '\r' || k === '\n') return finish(cursor === textOption && typed ? typed : options[cursor].label);
      else if (/^[1-9]$/.test(k) && !(cursor === textOption && typed)) {
        const i = Number(k) - 1;
        if (i >= options.length) return undefined;
        cursor = i;
        if (i !== textOption) return finish(options[i].label);
      } else if (k === '\u007f' || k === '\b') typed = typed.slice(0, -1);
      else if (cursor === textOption && k >= ' ') typed += k;
      draw();
      return undefined;
    };
    s.dialog = { onKey };
    if (process.stdin.isTTY) process.stdin.setRawMode(true);
    draw();
  });
}

async function askDialog(s) {
  const options = [
    { label: 'vitest', detail: 'Fast, Vite-native.' },
    { label: 'jest', detail: 'The classic.' },
    { label: 'Type something.' },
  ];
  const id = `toolu_${randomUUID().slice(0, 8)}`;
  s.log({
    type: 'assistant',
    message: {
      role: 'assistant',
      content: [{
        type: 'tool_use', id, name: 'AskUserQuestion',
        input: { questions: [{ question: 'Which framework should I use?', header: 'Framework', multiSelect: false,
          options: options.slice(0, 2).map((o) => ({ label: o.label, description: o.detail })) }] },
      }],
    },
  });
  const label = await choiceDialog(s, {
    header: 'Framework',
    question: 'Which framework should I use?',
    options,
    footer: 'Enter to select \u00b7 \u2191/\u2193 to navigate \u00b7 Esc to cancel',
    textOption: 2,
  });
  s.log({ type: 'user', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content: label }] } });
  return label;
}

async function permissionDialog(s) {
  const id = `toolu_${randomUUID().slice(0, 8)}`;
  s.log({
    type: 'assistant',
    message: { role: 'assistant', content: [{ type: 'tool_use', id, name: 'Bash', input: { command: 'mkdir fake_dir' } }] },
  });
  const label = await choiceDialog(s, {
    header: null,
    question: ' Bash command\n mkdir fake_dir\n\n Do you want to proceed?',
    options: [{ label: 'Yes' }, { label: "Yes, and don't ask again" }, { label: 'No' }],
    footer: ' Esc to cancel \u00b7 Tab to amend',
    textOption: -1,
  });
  s.log({ type: 'user', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content: label }] } });
  return label;
}

async function answer(text, s) {
  const model = text.match(/^\/model\s+(\S+)/);
  if (model) {
    out(`\nSet model to ${model[1]}${BOX}`);
    return;
  }
  s.runHooks('UserPromptSubmit', { prompt: text });
  s.log({ type: 'user', message: { role: 'user', content: text } });
  appendFileSync(s.received, `${text.replace(/\n/g, '\\n')}\n`, 'utf8');
  const { findDeliverablesDir, findDeliverables, safeJoin } = s.helpers;
  const briefPath = text.match(/read the brief at (.+?brief\.md)/)?.[1];
  const brief = briefPath ? readFileSync(briefPath, 'utf8') : '';
  const all = `${text}\n${brief}`;
  if (all.includes('FAKE_NOTIFY')) s.runHooks('Notification', { message: 'Claude needs your permission', notification_type: 'permission_prompt' });
  const sleepMs = Number(all.match(/FAKE_SLEEP_MS=(\d+)/)?.[1] ?? 0);
  if (sleepMs > 0) await sleep(sleepMs);
  const exitCode = all.match(/FAKE_EXIT=(\d+)/)?.[1];
  if (exitCode !== undefined) process.exit(Number(exitCode));
  let reply;
  if (s.asked) {
    s.asked = false;
    reply = `Thanks, using ${text.split('\n')[0]}.`;
  } else if (all.includes('FAKE_DIALOG')) {
    reply = `You chose ${await askDialog(s)}.`;
  }
  if (all.includes('FAKE_PERMISSION')) {
    const choice = await permissionDialog(s);
    out(`\n\u25cf Bash(mkdir fake_dir)\n  \u23bf  ${choice === 'No' ? 'Denied' : 'Done'}\n`);
  }
  let written = 0;
  if (brief && !all.includes('FAKE_FAIL')) {
    const dir = findDeliverablesDir(brief);
    for (const d of findDeliverables(brief)) {
      const target = dir ? safeJoin(dir, d.path) : null;
      if (!target) continue;
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, `# ${d.path}\n\nStub written by fake-claude (interactive).\n`, 'utf8');
      written += 1;
    }
  }
  if (reply === undefined) {
    reply = `you said: ${text.split('\n')[0]}\nok (${written} file(s) written)`;
    if (all.includes('FAKE_QUESTION')) {
      s.asked = true;
      reply = 'I can set this up.\n\nWhich framework should I use?\n1. vitest\n2. jest';
    } else if (all.includes('FAKE_OFFER')) {
      reply = `All ${written} file(s) written.\n\nWant me to also write tests?`;
    }
  }
  out(`\n${reply}\n`);
  assistantText(s, reply);
  s.runHooks('Stop', { stop_hook_active: false, last_assistant_message: reply });
  out(BOX);
}

/** One complete input line (cooked mode). */
async function onLine(raw, s) {
  let line = raw;
  if (Date.now() < s.dialogUntil) return; // the dialog eats input
  if (line.includes('\u0015')) line = line.slice(line.lastIndexOf('\u0015') + 1);
  if (s.paste === null && line.includes(PASTE_START)) {
    s.paste = [];
    line = line.replace(PASTE_START, '');
  }
  if (s.paste !== null) {
    const end = line.includes(PASTE_END);
    s.paste.push(line.replace(PASTE_END, ''));
    if (!end) return;
    line = s.paste.join('\n');
    s.paste = null;
  }
  const text = accept(line.trim(), s);
  if (text !== null) await answer(text, s);
}

/** Interactive mode: see the header comment of fake-claude.mjs. */
export async function interactive(helpers) {
  const s = makeSession(helpers);
  out(`fake-claude interactive (cwd ${process.cwd()})\n  ? for shortcuts`);
  if (!process.argv.includes('--fake-no-start')) s.runHooks('SessionStart', { source: 'startup' });
  if (s.dialogMs > 0) {
    s.dialogUntil = Date.now() + s.dialogMs;
    out('\n\nDo you trust the files in this folder?\n\n \u276f Yes, proceed\n   No, exit\n\nEnter to confirm \u00b7 Esc to exit\n');
    setTimeout(() => out(BOX), s.dialogMs);
  } else {
    out(BOX);
  }
  // Input lines are handled one at a time, in order; while a dialog is open
  // (the line handler awaits it) its keys go to it at once.
  let chain = Promise.resolve();
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk) => {
    if (s.dialog) {
      for (const k of keysOf(chunk)) if (s.dialog) s.dialog.onKey(k);
      return;
    }
    chain = chain.then(async () => {
      s.pending += chunk;
      let m;
      while ((m = s.pending.match(/\r\n|\r|\n/))) {
        const line = s.pending.slice(0, m.index);
        s.pending = s.pending.slice(m.index + m[0].length);
        await onLine(line, s);
        if (s.dialog) return;
      }
    }).catch((err) => process.stderr.write(`fake-claude: ${err?.stack ?? err}\n`));
  });
  await new Promise((r) => process.stdin.on('end', r));
  await chain;
  return 0;
}
