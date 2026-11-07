import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Api } from '../lib/api';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { ASSISTANT_RELOAD_MS, createAssistantStore, RUN_WORKFLOW_TEXT } from './assistantStore';
import { createGraphStore } from './graphStore';

const REPLY = 50;
const DIR = MOCK_EXAMPLE_DIR;

const mockApi = (): Api => createMockIpc({ assistantReplyMs: REPLY, trustAll: true });

async function setup(overrides: Partial<Api> = {}, base: Api = mockApi()) {
  const api: Api = { ...base, ...overrides };
  const graph = createGraphStore(api);
  expect(await graph.getState().openGraph(DIR)).toBe(true);
  const store = createAssistantStore(api, () => graph);
  await store.getState().load(DIR);
  const errors = (): string[] => graph.getState().toasts.filter((t) => t.kind === 'error').map((t) => t.text);
  return { api, graph, store, errors };
}

/** Let the reply land and the debounced reload run. */
const settle = (): Promise<void> => vi.advanceTimersByTimeAsync(REPLY + ASSISTANT_RELOAD_MS + 10).then(() => undefined);

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('assistantStore', () => {
  it('load fetches the empty idle state of the folder', async () => {
    const { store } = await setup();
    expect(store.getState()).toMatchObject({ dir: DIR, busy: false, error: null });
    expect(store.getState().state?.session.status).toBe('idle');
  });

  it('send: instruction first, then answer while waiting; updates reload the state', async () => {
    const { store } = await setup();
    expect(await store.getState().send(RUN_WORKFLOW_TEXT)).toBe(true);
    expect(store.getState().state?.session.status).toBe('working');
    await settle();
    expect(store.getState().state?.session.status).toBe('waiting');
    expect(store.getState().state?.messages.map((m) => m.kind)).toEqual(['instruction', 'stage', 'question']);

    expect(await store.getState().send('yes')).toBe(true);
    await settle();
    const s = store.getState().state;
    expect(s?.messages.map((m) => m.kind).slice(3)).toEqual(['answer', 'stage', 'final']);
    expect(s?.session.status).toBe('done');
  });

  it('saves the graph before sending', async () => {
    const order: string[] = [];
    const base = mockApi();
    const { store, graph } = await setup({
      saveGraph: async (d, g) => {
        order.push('save');
        return base.saveGraph(d, g);
      },
      assistantSend: async (d, t) => {
        order.push('send');
        return base.assistantSend(d, t);
      },
    }, base);
    graph.getState().setName('renamed');
    expect(await store.getState().send('go')).toBe(true);
    expect(order).toEqual(['save', 'send']);
    expect(graph.getState().dirty).toBe(false);
  });

  it('a failed save blocks the send with a toast', async () => {
    const send = vi.fn();
    const { store, graph, errors } = await setup({
      saveGraph: async () => Promise.reject({ code: 'io', message: 'disk full' }),
      assistantSend: send,
    });
    graph.getState().setName('renamed');
    expect(await store.getState().send('go')).toBe(false);
    expect(send).not.toHaveBeenCalled();
    expect(errors().some((t) => t.startsWith('Not sent: the graph could not be saved'))).toBe(true);
  });

  it('does not send while the scheduler works; backend errors become toasts', async () => {
    const { store, errors } = await setup();
    await store.getState().send('go');
    expect(await store.getState().send('again')).toBe(false);
    expect(errors().some((t) => t.includes('The scheduler is working'))).toBe(true);

    const failing = await setup({ assistantSend: async () => Promise.reject({ code: 'assistant_busy', message: 'busy elsewhere' }) });
    expect(await failing.store.getState().send('go')).toBe(false);
    expect(failing.errors()).toContain('Not sent: busy elsewhere');
    expect(failing.store.getState().busy).toBe(false);
  });

  it('ignores blank text and a concurrent call', async () => {
    const { store } = await setup();
    expect(await store.getState().send('   ')).toBe(false);
    const first = store.getState().send('one');
    expect(store.getState().busy).toBe(true);
    expect(await store.getState().send('two')).toBe(false);
    expect(await first).toBe(true);
  });

  it('cancel and reset', async () => {
    const { store, errors } = await setup();
    await store.getState().send('go');
    await store.getState().cancel();
    await vi.advanceTimersByTimeAsync(ASSISTANT_RELOAD_MS + 10);
    expect(store.getState().state?.session.status).toBe('cancelled');

    expect(await store.getState().reset()).toBe(true);
    const s = store.getState().state;
    expect(s?.session).toMatchObject({ status: 'idle', session_id: null });
    expect(s?.messages[s.messages.length - 1]?.kind).toBe('system');

    await store.getState().cancel();
    expect(errors().some((t) => t.startsWith('Cancel failed'))).toBe(true);
  });

  it('reset while working reports assistant_busy as a toast', async () => {
    const { store, errors } = await setup();
    await store.getState().send('go');
    expect(await store.getState().reset()).toBe(false);
    expect(errors().some((t) => t.startsWith('Reset failed'))).toBe(true);
  });

  it('collects scheduler log lines only', async () => {
    const { store } = await setup();
    await store.getState().send('go');
    await settle();
    const log = store.getState().log;
    expect(log.length).toBeGreaterThan(3);
    expect(log[log.length - 1]?.kind).toBe('result');
    store.getState().clearLog();
    expect(store.getState().log).toEqual([]);
  });

  it('close unsubscribes: later updates do not reload', async () => {
    const stateSpy = vi.fn();
    const base = mockApi();
    const { store } = await setup({
      assistantState: async (d) => {
        stateSpy();
        return base.assistantState(d);
      },
    }, base);
    stateSpy.mockClear();
    store.getState().close();
    expect(store.getState().dir).toBeNull();
    await base.assistantSend(DIR, 'go');
    await settle();
    expect(stateSpy).not.toHaveBeenCalled();
    expect(store.getState().state).toBeNull();
  });

  it('a load failure is a toast and an error, not a crash', async () => {
    const { store, errors } = await setup({ assistantState: async () => Promise.reject({ code: 'io', message: 'unreadable' }) });
    expect(store.getState().error).toBe('unreadable');
    expect(errors()).toContain('Could not load the assistant: unreadable');
  });

  it('drops malformed payloads', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { store } = await setup({ assistantState: async () => ({ nope: true }) as never });
    expect(store.getState().state).toBeNull();
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});
