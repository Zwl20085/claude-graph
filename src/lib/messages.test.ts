import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NodeEvent, NodeMessage, RunState } from '../types/contract';
import { closeInTime, composerState, displayMessages, mergeMessages, pruneLocal, SAME_MESSAGE_WINDOW_MS } from './messages';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { MOCK_RECORD_MS, MOCK_REPLY_MS } from './mockRunner';

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

describe('same message, different timestamps (send to a finished node)', () => {
  // Backend: line saved at S (chrono, nanoseconds), event emitted later at E; local send stamped by the window at L.
  const L = '2026-11-07T10:00:00.100Z';
  const S = '2026-11-07T10:00:00.250123456Z';
  const E = '2026-11-07T10:00:00.251987654Z';

  it('closeInTime parses sub-millisecond RFC 3339 and honours the window', () => {
    expect(closeInTime(S, E)).toBe(true);
    expect(closeInTime('2026-11-07T10:00:00+00:00', '2026-11-07T10:00:04Z')).toBe(true);
    const far = new Date(Date.parse(L) + SAME_MESSAGE_WINDOW_MS + 1).toISOString();
    expect(closeInTime(L, far)).toBe(false);
    expect(closeInTime('t1', 't1')).toBe(true);
    expect(closeInTime('t1', 't2')).toBe(false);
  });

  it('snapshot + live event + local send show one user bubble', () => {
    const snapshot = [msg('user', 'task', 't0'), msg('assistant', 'done', '2026-11-07T09:00:00Z'), msg('user', 'more', S)];
    const history = mergeMessages(snapshot, [msg('user', 'more', E)]);
    expect(history.filter((m) => m.text === 'more')).toHaveLength(1);
    expect(displayMessages(history, [{ text: 'more', ts: L }]).filter((m) => m.text === 'more')).toHaveLength(1);
    expect(pruneLocal(history, [{ text: 'more', ts: L }])).toEqual([]);
  });

  it('a live event before the snapshot holds the line is kept, then absorbed', () => {
    const before = [msg('assistant', 'done', '2026-11-07T09:00:00Z')];
    expect(mergeMessages(before, [msg('user', 'more', E)]).map((m) => m.text)).toEqual(['done', 'more']);
    const after = [...before, msg('user', 'more', S), msg('assistant', 'reply', '2026-11-07T10:00:09Z')];
    expect(mergeMessages(after, [msg('user', 'more', E)]).map((m) => m.text)).toEqual(['done', 'more', 'reply']);
  });

  it('a message really sent twice stays twice', () => {
    const snapshot = [msg('user', 'yes', S), msg('user', 'yes', E)];
    expect(mergeMessages(snapshot, [msg('user', 'yes', E), msg('user', 'yes', E)])).toHaveLength(2);
    expect(displayMessages(snapshot, [{ text: 'yes', ts: L }, { text: 'yes', ts: L }]).length).toBe(2);
    const later = new Date(Date.parse(E) + SAME_MESSAGE_WINDOW_MS + 1).toISOString();
    expect(displayMessages(snapshot, [{ text: 'yes', ts: later }]).length).toBe(3);
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
    expect(events[events.length - 1]).toMatchObject({ kind: 'result' });
    // The reply is recorded after the result event, as in the backend.
    expect(await api.listMessages(MOCK_EXAMPLE_DIR, runId, 'survey')).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(MOCK_RECORD_MS);
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
