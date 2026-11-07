import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NodeEvent, NodeMessage, RunState } from '../types/contract';
import { composerState, displayMessages, pruneLocal } from './messages';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { MOCK_REPLY_MS } from './mockRunner';

const msg = (role: NodeMessage['role'], text: string, ts: string): NodeMessage => ({ role, text, ts });

describe('displayMessages', () => {
  it('orders by time and marks user messages without a reply as queued', () => {
    const out = displayMessages(
      [msg('user', 'task', 't1'), msg('assistant', 'done', 't2'), msg('user', 'more', 't3')],
      [{ text: 'again', ts: 't4' }],
    );
    expect(out.map((m) => [m.role, m.text, m.queued])).toEqual([
      ['user', 'task', false],
      ['assistant', 'done', false],
      ['user', 'more', true],
      ['user', 'again', true],
    ]);
  });

  it('does not duplicate a local send already in history', () => {
    const out = displayMessages([msg('user', 'hi', 't2')], [{ text: 'hi', ts: 't1' }]);
    expect(out).toHaveLength(1);
  });

  it('pruneLocal drops local sends that history now holds', () => {
    expect(pruneLocal([msg('user', 'hi', 't2')], [{ text: 'hi', ts: 't1' }, { text: 'x', ts: 't1' }])).toEqual([
      { text: 'x', ts: 't1' },
    ]);
  });
});

describe('composerState', () => {
  it.each([
    [null, false, /Run the node first/],
    ['skipped', false, /Run the node first/],
    ['pending', false, /Waiting/],
    ['running', true, /after the current turn/],
    ['succeeded', true, /resumes/],
    ['failed', true, /resumes/],
  ] as const)('%s', (status, enabled, hint) => {
    const s = composerState(status);
    expect(s.enabled).toBe(enabled);
    expect(s.hint).toMatch(hint);
  });
});

describe('mock send_message', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('resumes a finished node and echoes a reply after 1.5 s', async () => {
    const api = createMockIpc({ stepMs: 10, trustAll: true });
    const states: RunState[] = [];
    const events: NodeEvent[] = [];
    void api.onRunState((s) => states.push(s));
    void api.onNodeEvent((e) => events.push(e));
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await vi.runAllTimersAsync();
    expect(await api.listMessages(MOCK_EXAMPLE_DIR, runId, 'survey')).toHaveLength(2);

    await api.sendMessage(MOCK_EXAMPLE_DIR, runId, 'survey', 'and one more');
    expect(states[states.length - 1]?.nodes.survey?.status).toBe('running');
    expect(events[events.length - 1]).toMatchObject({ kind: 'user_message', text: 'and one more' });
    await vi.advanceTimersByTimeAsync(MOCK_REPLY_MS);
    const history = await api.listMessages(MOCK_EXAMPLE_DIR, runId, 'survey');
    expect(history.map((m) => m.role)).toEqual(['user', 'assistant', 'user', 'assistant']);
    expect(history[3]?.text).toContain('and one more');
    expect(states[states.length - 1]).toMatchObject({ status: 'succeeded', nodes: { survey: { status: 'succeeded' } } });
  });

  it('refuses nodes that never ran and empty text', async () => {
    const api = createMockIpc({ stepMs: 10, trustAll: true });
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, { from: 'report' });
    await expect(api.sendMessage(MOCK_EXAMPLE_DIR, runId, 'report', 'hi')).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.sendMessage(MOCK_EXAMPLE_DIR, runId, 'survey', 'hi')).rejects.toMatchObject({ code: 'not_found' });
    await expect(api.sendMessage(MOCK_EXAMPLE_DIR, runId, 'report', ' ')).rejects.toMatchObject({ code: 'invalid_input' });
    await vi.runAllTimersAsync();
  });
});
