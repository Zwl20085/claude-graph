/** "Ask me in the app" mock backend: FAKE_ASK prompts, answers, scopes, cancel. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NodeEvent, PermissionPrompt, RunState } from '../types/contract';
import { updateNode } from './graphOps';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { createMockPermissions, matchKey, promptSummary, SUMMARY_MAX, type AskDecision } from './mockPermissions';
import { askedTool } from './mockRunner';

const STEP = 100;
const DIR = MOCK_EXAMPLE_DIR;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-07T10:15:00Z'));
});
afterEach(() => vi.useRealTimers());

async function setup(prompt = 'Do it. FAKE_ASK=Bash') {
  const api = createMockIpc({ stepMs: STEP, now: () => new Date(Date.now()), trustAll: true });
  const prompts: PermissionPrompt[] = [];
  const answered: string[] = [];
  const events: NodeEvent[] = [];
  const states: RunState[] = [];
  await api.onPermissionPrompt((p) => prompts.push(p));
  await api.onPermissionAnswered((a) => answered.push(a.id));
  await api.onNodeEvent((e) => events.push(e));
  await api.onRunState((s) => states.push(s));
  const g = await api.openGraph(DIR);
  await api.saveGraph(DIR, updateNode(g, 'survey', { prompt }));
  const last = (): RunState | undefined => states[states.length - 1];
  const surveyKinds = (): string[] => events.filter((e) => e.node_id === 'survey').map((e) => e.kind);
  return { api, prompts, answered, events, last, surveyKinds };
}

describe('mock FAKE_ASK', () => {
  it('raises one prompt with the summary and the node waits until it is answered', async () => {
    const { api, prompts, events, last } = await setup();
    const runId = await api.startRun(DIR, { from: 'survey' });
    await vi.advanceTimersByTimeAsync(STEP * 20);

    expect(prompts).toHaveLength(1);
    expect(prompts[0]).toMatchObject({ run_id: runId, node_id: 'survey', tool_name: 'Bash', summary: 'echo hi', suggestions: [] });
    expect(prompts[0]?.input).toEqual({ command: 'echo hi' });
    expect(await api.listPermissionPrompts()).toEqual(prompts);
    expect(events.find((e) => e.kind === 'permission_prompt')?.text).toBe('Bash: echo hi');
    // Still waiting, however long it takes.
    await vi.advanceTimersByTimeAsync(STEP * 100);
    expect(last()?.nodes.survey?.status).toBe('running');
    expect(events.some((e) => e.node_id === 'survey' && e.kind === 'result')).toBe(false);
  });

  it('allow once: the node continues, writes the allowed file and succeeds', async () => {
    const { api, prompts, answered, events, last, surveyKinds } = await setup();
    await api.startRun(DIR, { from: 'survey' });
    await vi.advanceTimersByTimeAsync(STEP * 20);
    const id = prompts[0]?.id ?? '';
    await api.answerPermission({ id, allow: true, scope: 'once' });
    expect(answered).toEqual([id]);
    expect(await api.listPermissionPrompts()).toEqual([]);
    await vi.runAllTimersAsync();

    expect(events.find((e) => e.kind === 'permission_answered')?.text).toBe('allowed (once)');
    expect(events.some((e) => e.text === 'wrote asked-allowed.txt')).toBe(true);
    expect(surveyKinds()).toEqual([
      'system', 'text', 'tool_use', 'permission_prompt', 'permission_answered', 'tool_result', 'tool_use', 'tool_result', 'text', 'result',
    ]);
    expect(last()?.status).toBe('succeeded');
  });

  it('deny with a message: the node is told why and writes the denied file', async () => {
    const { api, prompts, events } = await setup();
    await api.startRun(DIR, { from: 'survey' });
    await vi.advanceTimersByTimeAsync(STEP * 20);
    await api.answerPermission({ id: prompts[0]?.id ?? '', allow: false, scope: 'once', message: 'not today' });
    await vi.runAllTimersAsync();
    expect(events.find((e) => e.kind === 'permission_answered')?.text).toBe('denied (once)');
    expect(events.some((e) => e.text === 'wrote asked-denied.txt: not today')).toBe(true);
  });

  it('answer of an unknown or answered prompt rejects with not_found', async () => {
    const { api, prompts } = await setup();
    await api.startRun(DIR, { from: 'survey' });
    await vi.advanceTimersByTimeAsync(STEP * 20);
    const id = prompts[0]?.id ?? '';
    await api.answerPermission({ id, allow: true, scope: 'once' });
    await expect(api.answerPermission({ id, allow: true, scope: 'once' })).rejects.toMatchObject({ code: 'not_found' });
  });

  it('graph scope is remembered for the folder: a new run auto-answers without a prompt', async () => {
    const { api, prompts, events, last } = await setup();
    await api.startRun(DIR, { from: 'survey' });
    await vi.advanceTimersByTimeAsync(STEP * 20);
    await api.answerPermission({ id: prompts[0]?.id ?? '', allow: true, scope: 'graph' });
    await vi.runAllTimersAsync();
    expect(last()?.status).toBe('succeeded');

    vi.setSystemTime(new Date('2026-11-07T10:20:00Z'));
    const second = await api.startRun(DIR, { from: 'survey' });
    await vi.runAllTimersAsync();
    expect(prompts).toHaveLength(1);
    const mine = events.filter((e) => e.run_id === second && e.node_id === 'survey');
    expect(mine.find((e) => e.kind === 'permission_prompt')?.text).toBe('Bash: echo hi');
    expect(mine.find((e) => e.kind === 'permission_answered')?.text).toBe('allowed (graph, remembered)');
    expect(last()).toMatchObject({ run_id: second, status: 'succeeded' });
  });

  it('cancelling the run denies the pending prompt with "cancelled"', async () => {
    const { api, prompts, answered, events, last } = await setup();
    const runId = await api.startRun(DIR, { from: 'survey' });
    await vi.advanceTimersByTimeAsync(STEP * 20);
    await api.cancelRun(runId);
    await vi.runAllTimersAsync();
    expect(answered).toEqual([prompts[0]?.id]);
    expect(await api.listPermissionPrompts()).toEqual([]);
    expect(events.find((e) => e.kind === 'permission_answered')?.text).toBe('denied (cancelled)');
    expect(events.some((e) => e.text.startsWith('wrote asked-'))).toBe(false);
    expect(last()?.status).toBe('cancelled');
  });

  it('cancelling the node denies its prompt', async () => {
    const { api, prompts, answered } = await setup();
    const runId = await api.startRun(DIR, { from: 'survey' });
    await vi.advanceTimersByTimeAsync(STEP * 20);
    await api.cancelNode(runId, 'survey');
    expect(answered).toEqual([prompts[0]?.id]);
  });

  it('is ignored in terminal mode (the backend relays headless nodes only)', async () => {
    const api = createMockIpc({ stepMs: STEP, now: () => new Date(Date.now()), trustAll: true, exampleRunMode: 'terminal' });
    const prompts: PermissionPrompt[] = [];
    await api.onPermissionPrompt((p) => prompts.push(p));
    const g = await api.openGraph(DIR);
    await api.saveGraph(DIR, updateNode(g, 'survey', { prompt: 'FAKE_ASK=Bash' }));
    await api.startRun(DIR, { from: 'survey' });
    await vi.runAllTimersAsync();
    expect(prompts).toEqual([]);
  });
});

describe('mock permission scopes', () => {
  const deps = () => ({ now: () => new Date(), emitPrompt: vi.fn(), emitAnswered: vi.fn(), emitEvent: vi.fn() });
  const req = { graph_dir: '/g', run_id: 'r1', node_id: 'n', tool_name: 'Bash', input: { command: 'ls' } };

  it('node scope auto-answers a second identical ask of the same node until the run ends', () => {
    const d = deps();
    const perms = createMockPermissions(d);
    const decisions: AskDecision[] = [];
    perms.ask(req, (x) => decisions.push(x));
    perms.answer({ id: perms.list()[0]?.id ?? '', allow: true, scope: 'node' });
    perms.ask(req, (x) => decisions.push(x));
    expect(d.emitPrompt).toHaveBeenCalledTimes(1);
    expect(decisions.map((x) => [x.allow, x.scope, x.auto])).toEqual([[true, 'node', false], [true, 'node', true]]);

    // Another node, another command, or another run asks again.
    perms.ask({ ...req, node_id: 'other' }, () => {});
    perms.ask({ ...req, input: { command: 'rm -rf x' } }, () => {});
    perms.endRun('r1');
    perms.ask(req, () => {});
    expect(d.emitPrompt).toHaveBeenCalledTimes(4);
  });

  it('a deny is not remembered', () => {
    const d = deps();
    const perms = createMockPermissions(d);
    perms.ask(req, () => {});
    perms.answer({ id: perms.list()[0]?.id ?? '', allow: false, scope: 'node' });
    perms.ask(req, () => {});
    expect(d.emitPrompt).toHaveBeenCalledTimes(2);
  });

  it('summaries and match keys follow the contract', () => {
    expect(promptSummary('Bash', { command: 'echo hi' })).toBe('echo hi');
    expect(promptSummary('Write', { file_path: 'a.txt', content: 'x' })).toBe('a.txt');
    expect(promptSummary('Edit', { file_path: 'b.txt' })).toBe('b.txt');
    expect(promptSummary('WebSearch', { query: 'q' })).toBe('q');
    expect(promptSummary('WebFetch', { url: 'https://x' })).toBe('https://x');
    expect(promptSummary('mcp__x__y', { a: 'z'.repeat(300) })).toHaveLength(SUMMARY_MAX);
    expect(matchKey('Bash', { command: 'ls', description: 'd' }, 'ls')).toBe('ls');
    expect(matchKey('Other', { a: 1 }, '{"a":1}')).toBe('{"a":1}');
  });

  it('askedTool reads FAKE_ASK=<tool> from the prompt or the extra context', () => {
    expect(askedTool({ prompt: 'x FAKE_ASK=WebSearch y', extra_context: '' })).toBe('WebSearch');
    expect(askedTool({ prompt: '', extra_context: 'FAKE_ASK=mcp__a__b' })).toBe('mcp__a__b');
    expect(askedTool({ prompt: 'FAKE_ASK', extra_context: '' })).toBeNull();
  });
});
