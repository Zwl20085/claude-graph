/**
 * Regression: after a send to a finished node the user bubble showed twice and
 * the reply only appeared after a tab switch. The backend emits `user_message`
 * with a later ts than the saved line, and emits `result` before it records
 * the reply; the mock below reproduces both (ticking clock, record delay).
 */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from '../lib/api';
import { MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { MOCK_RECORD_MS, MOCK_REPLY_MS } from '../lib/mockRunner';
import { createGraphStore, installStore } from '../store/graphStore';
import { MessagesTab } from './MessagesTab';

vi.mock('../lib/api', async () => {
  const { createMockIpc } = await import('../lib/ipc.mock');
  let tick = 0;
  // Every reading of the clock is 1 ms later: saved line and event differ in ts.
  const now = (): Date => new Date(Date.now() + (tick += 1));
  return { api: createMockIpc({ stepMs: 10, trustAll: true, now }), MOCK_MODE: true };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const bubbles = (): string[] => [...document.querySelectorAll('div.whitespace-pre-wrap')].map((b) => b.textContent ?? '');
const advance = (ms: number): Promise<void> => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));

function typeInto(el: HTMLTextAreaElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

describe('MessagesTab follow-up to a finished node', () => {
  let root: Root | null = null;
  beforeEach(() => vi.useFakeTimers());
  afterEach(async () => {
    if (root) await act(async () => root?.unmount());
    root = null;
    document.body.innerHTML = '';
    vi.useRealTimers();
  });

  it('shows the user message once and the reply without a tab switch', async () => {
    const store = createGraphStore(api);
    installStore(store);
    void api.onRunState((s) => store.getState().applyRunState(s));
    void api.onNodeEvent((e) => store.getState().appendEvent(e));
    expect(await store.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
    await act(async () => void (await store.getState().startRun('all')));
    await advance(5000);
    expect(store.getState().runState?.nodes.survey?.status).toBe('succeeded');

    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    await act(async () => root?.render(<MessagesTab nodeId="survey" />));
    await advance(10);
    expect(bubbles()).toHaveLength(2);

    const box = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="message to this node"]');
    if (!box) throw new Error('composer missing');
    await act(async () => typeInto(box, 'and one more'));
    const send = [...document.querySelectorAll('button')].find((b) => b.textContent?.includes('Send'));
    await act(async () => send?.click());
    await advance(10);
    expect(bubbles().filter((t) => t.startsWith('and one more'))).toHaveLength(1);

    // `result` arrives before the reply is recorded: still one user bubble, no reply yet.
    await advance(MOCK_REPLY_MS);
    expect(bubbles().filter((t) => t.startsWith('and one more'))).toHaveLength(1);

    // The run:state that follows the recording triggers the refetch.
    await advance(MOCK_RECORD_MS + 10);
    const shown = bubbles();
    expect(shown.filter((t) => t.startsWith('and one more'))).toHaveLength(1);
    expect(shown[shown.length - 1]).toBe('(mock) Received: and one more');
    expect(shown).toHaveLength(4);
  });
});
