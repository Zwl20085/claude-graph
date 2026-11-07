/** Assistant tab against the mock scheduler: empty state, cards per kind, composer states, Ctrl+Enter, reset. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Api } from '../lib/api';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { COMPOSER_PLACEHOLDER } from '../lib/assistantView';
import { ASSISTANT_RELOAD_MS, createAssistantStore, installAssistantStore } from '../store/assistantStore';
import { createGraphStore, installStore } from '../store/graphStore';
import { AssistantCard } from './AssistantCard';
import { AssistantPanel } from './AssistantPanel';
import type { AssistantKind, AssistantMessage } from '../types/contract';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const REPLY = 50;
const advance = (ms: number): Promise<void> => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));
const settle = (): Promise<void> => advance(REPLY + ASSISTANT_RELOAD_MS + 10);
const button = (label: string): HTMLButtonElement | null => document.querySelector(`button[aria-label="${label}"]`);
const composer = (): HTMLTextAreaElement => {
  const el = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="message to the scheduler"]');
  if (!el) throw new Error('composer missing');
  return el;
};
const cardKinds = (): string[] => [...document.querySelectorAll('article[data-kind]')].map((a) => a.getAttribute('data-kind') ?? '');

function typeInto(el: HTMLTextAreaElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

async function click(el: Element | null): Promise<void> {
  if (!el) throw new Error('element not found');
  await act(async () => (el as HTMLElement).click());
}

let root: Root | null = null;

async function mount(api: Api = createMockIpc({ assistantReplyMs: REPLY, trustAll: true })) {
  const graph = createGraphStore(api);
  installStore(graph);
  expect(await graph.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
  const store = createAssistantStore(api, () => graph);
  installAssistantStore(store);
  await store.getState().load(MOCK_EXAMPLE_DIR);
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root?.render(<AssistantPanel />));
  return { api, graph, store };
}

beforeEach(() => vi.useFakeTimers());
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
  vi.useRealTimers();
});

describe('AssistantPanel', () => {
  it('shows the empty state and runs the workflow from it', async () => {
    await mount();
    expect(document.body.textContent).toContain('Model and permissions come from your Claude Code settings.');
    expect(document.body.textContent).toContain('permissions.defaultMode');
    expect(document.querySelector('[data-status="idle"]')?.textContent).toBe('Idle');
    await click(button('run the workflow'));
    expect(document.querySelector('[data-status="working"]')?.textContent).toBe('Working…');
    expect(composer().disabled).toBe(true);
    expect(button('cancel the scheduler turn')).not.toBeNull();
    expect(button('reset the scheduler session')).toBeNull();
    expect(cardKinds()).toEqual(['instruction']);
    await settle();
    expect(cardKinds()).toEqual(['instruction', 'stage', 'question']);
    expect(document.body.textContent).toContain('Stage: survey');
    expect(document.querySelector('[data-status="waiting"]')?.textContent).toBe('Waiting for your answer');
  });

  it('answers with Ctrl+Enter; the Answer button focuses the composer', async () => {
    await mount();
    await click(button('run the workflow'));
    await settle();
    expect(composer().placeholder).toBe(COMPOSER_PLACEHOLDER.answer);
    expect(composer().disabled).toBe(false);
    composer().blur();
    await click(button('answer this question'));
    expect(document.activeElement).toBe(composer());

    const send = button('send to the scheduler');
    expect(send?.disabled).toBe(true);
    await act(async () => typeInto(composer(), 'yes'));
    expect(send?.disabled).toBe(false);
    await act(async () => {
      composer().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }));
    });
    await advance(0);
    expect(composer().value).toBe('');
    await settle();
    expect(cardKinds()).toEqual(['instruction', 'stage', 'question', 'answer', 'stage', 'final']);
    expect(document.querySelector('[data-status="done"]')).not.toBeNull();
    expect(composer().placeholder).toBe(COMPOSER_PLACEHOLDER.instruction);
  });

  it('plain Enter does not send', async () => {
    const { store } = await mount();
    await act(async () => typeInto(composer(), 'hello'));
    await act(async () => {
      composer().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    });
    expect(store.getState().state?.messages).toEqual([]);
  });

  it('keeps the draft when the send fails', async () => {
    const base = createMockIpc({ assistantReplyMs: REPLY, trustAll: true });
    const { graph } = await mount({ ...base, assistantSend: async () => Promise.reject({ code: 'spawn_failed', message: 'no claude' }) });
    await act(async () => typeInto(composer(), 'go'));
    await click(button('send to the scheduler'));
    expect(composer().value).toBe('go');
    expect(graph.getState().toasts.map((t) => t.text)).toContain('Not sent: no claude');
  });

  it('reset asks inline first', async () => {
    const { store } = await mount();
    await click(button('run the workflow'));
    await settle();
    await click(button('reset the scheduler session'));
    expect(document.body.textContent).toContain('Start a fresh session?');
    await click(button('keep the session'));
    expect(store.getState().state?.session.session_id).not.toBeNull();
    await click(button('reset the scheduler session'));
    await click(button('confirm reset'));
    expect(store.getState().state?.session).toMatchObject({ status: 'idle', session_id: null });
    expect(cardKinds()[cardKinds().length - 1]).toBe('system');
  });

  it('cancel stops a working turn', async () => {
    await mount();
    await click(button('run the workflow'));
    await click(button('cancel the scheduler turn'));
    await settle();
    expect(document.querySelector('[data-status="cancelled"]')).not.toBeNull();
    expect(document.body.textContent).toContain('Turn 1 cancelled');
  });
});

describe('AssistantCard', () => {
  const msg = (kind: AssistantKind, title: string | null = null): AssistantMessage => ({
    id: kind,
    ts: '2026-11-07T10:00:00Z',
    turn: 1,
    role: kind === 'instruction' || kind === 'answer' ? 'user' : kind === 'system' || kind === 'error' ? 'app' : 'scheduler',
    kind,
    title,
    text: `body of ${kind}\n### sub-heading stays text`,
  });

  it('renders every kind with its heading and plain-text body', async () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    const all: AssistantMessage[] = [
      msg('instruction'), msg('answer'), msg('stage', 'survey'), msg('question'), msg('decision', 'Skip design'),
      msg('note'), msg('final'), msg('system'), msg('error'),
    ];
    await act(async () => root?.render(<>{all.map((m) => <AssistantCard key={m.id} message={m} onAnswer={() => undefined} />)}</>));
    const labels = [...document.querySelectorAll('article')].map((a) => a.getAttribute('aria-label'));
    expect(labels).toEqual(['Instruction', 'Answer', 'Stage: survey', 'Question', 'Decision: Skip design', 'Note', 'Final', 'System', 'Error']);
    expect(document.querySelectorAll('button[aria-label="answer this question"]')).toHaveLength(1);
    expect(document.body.textContent).toContain('### sub-heading stays text');
    const user = document.querySelector('article[data-kind="instruction"]')?.parentElement;
    expect(user?.className).toContain('justify-end');
  });
});
