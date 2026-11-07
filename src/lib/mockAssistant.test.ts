import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppError, AssistantState, NodeEvent } from '../types/contract';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { MOCK_SESSION_RESET_TEXT } from './mockAssistant';

const REPLY = 50;
const DIR = MOCK_EXAMPLE_DIR;

function setup() {
  const api = createMockIpc({ assistantReplyMs: REPLY, trustAll: true });
  const updates: AssistantState[] = [];
  const events: NodeEvent[] = [];
  void api.onAssistantUpdate((s) => updates.push(s));
  void api.onNodeEvent((e) => events.push(e));
  return { api, updates, events };
}

const kinds = (s: AssistantState): string[] => s.messages.map((m) => m.kind);
const code = async (p: Promise<unknown>): Promise<string> => {
  try {
    await p;
    return 'ok';
  } catch (err) {
    return (err as AppError).code;
  }
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('mock assistant scheduler', () => {
  it('starts with an empty idle state', async () => {
    const { api } = setup();
    const s = await api.assistantState(DIR);
    expect(s.session).toMatchObject({ status: 'idle', turns: 0, session_id: null });
    expect(s.messages).toEqual([]);
  });

  it('instruction -> stage + question (waiting); answer -> stage + final (done); later instruction -> note + final', async () => {
    const { api, updates } = setup();
    const sent = await api.assistantSend(DIR, '  Execute the workflow from the start.  ');
    expect(sent.session).toMatchObject({ status: 'working', turns: 1 });
    expect(sent.messages[0]).toMatchObject({ role: 'user', kind: 'instruction', turn: 1, text: 'Execute the workflow from the start.' });
    await vi.advanceTimersByTimeAsync(REPLY);
    let s = await api.assistantState(DIR);
    expect(s.session.status).toBe('waiting');
    expect(kinds(s)).toEqual(['instruction', 'stage', 'question']);
    expect(s.messages[1]).toMatchObject({ role: 'scheduler', title: 'survey' });

    const answered = await api.assistantSend(DIR, 'yes');
    expect(answered.messages[3]).toMatchObject({ kind: 'answer', turn: 2 });
    await vi.advanceTimersByTimeAsync(REPLY);
    s = await api.assistantState(DIR);
    expect(s.session.status).toBe('done');
    expect(kinds(s).slice(3)).toEqual(['answer', 'stage', 'final']);

    await api.assistantSend(DIR, 'redo the report');
    await vi.advanceTimersByTimeAsync(REPLY);
    s = await api.assistantState(DIR);
    expect(kinds(s).slice(6)).toEqual(['instruction', 'note', 'final']);
    expect(s.session).toMatchObject({ status: 'done', turns: 3 });
    expect(updates.length).toBe(6);
  });

  it('streams scheduler lines on node:event', async () => {
    const { api, events } = setup();
    await api.assistantSend(DIR, 'go');
    await vi.advanceTimersByTimeAsync(REPLY);
    expect(events.every((e) => e.run_id === 'assistant' && e.node_id === 'scheduler')).toBe(true);
    expect(events.map((e) => e.kind)).toEqual(['user_message', 'system', 'text', 'tool_use', 'tool_result', 'result']);
  });

  it('refuses blank text and a second send while working', async () => {
    const { api } = setup();
    expect(await code(api.assistantSend(DIR, '   '))).toBe('invalid_input');
    await api.assistantSend(DIR, 'go');
    expect(await code(api.assistantSend(DIR, 'again'))).toBe('assistant_busy');
    expect(await code(api.assistantReset(DIR))).toBe('assistant_busy');
    expect(await code(api.assistantSend('/mock/none', 'go'))).toBe('not_found');
  });

  it('cancel stops the turn; cancel with nothing running is not_found', async () => {
    const { api } = setup();
    expect(await code(api.assistantCancel(DIR))).toBe('not_found');
    await api.assistantSend(DIR, 'go');
    await api.assistantCancel(DIR);
    await vi.advanceTimersByTimeAsync(REPLY * 2);
    const s = await api.assistantState(DIR);
    expect(s.session.status).toBe('cancelled');
    expect(s.messages[s.messages.length - 1]).toMatchObject({ role: 'app', kind: 'system', text: 'Turn 1 cancelled' });
  });

  it('reset clears the session and the next instruction starts fresh', async () => {
    const { api } = setup();
    await api.assistantSend(DIR, 'go');
    await vi.advanceTimersByTimeAsync(REPLY);
    const r = await api.assistantReset(DIR);
    expect(r.session).toMatchObject({ status: 'idle', session_id: null, workflow_hash: null });
    expect(r.messages[r.messages.length - 1]).toMatchObject({ kind: 'system', text: MOCK_SESSION_RESET_TEXT, turn: 0 });
    await api.assistantSend(DIR, 'go again');
    await vi.advanceTimersByTimeAsync(REPLY);
    const s = await api.assistantState(DIR);
    expect(kinds(s).slice(-3)).toEqual(['instruction', 'stage', 'question']);
  });

  it('FAKE_FAIL fails the turn with an app error message', async () => {
    const { api } = setup();
    await api.assistantSend(DIR, 'please FAKE_FAIL');
    await vi.advanceTimersByTimeAsync(REPLY);
    const s = await api.assistantState(DIR);
    expect(s.session.status).toBe('failed');
    expect(s.session.last_error).toContain('FAKE_FAIL');
    expect(s.messages[s.messages.length - 1]).toMatchObject({ role: 'app', kind: 'error' });
  });
});
