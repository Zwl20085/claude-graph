/** Terminal mode in the main window: session chips, run-mode radio, Prompt note, Messages and Results paths. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RunState, TerminalSession } from '../types/contract';
import { api } from '../lib/api';
import { MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { createGraphStore, installStore, type GraphStore } from '../store/graphStore';
import type { StoreApi } from 'zustand/vanilla';
import { HEADLESS_ONLY_NOTE, PromptTab } from './PromptTab';
import { MessagesTab } from './MessagesTab';
import { ResultsTab } from './ResultsTab';
import { SessionChips } from './SessionChips';
import { SettingsPopover } from './SettingsPopover';
import { SESSION_CLOSED_TEXT, terminalLine } from './TerminalMessages';

vi.mock('../lib/api', async () => {
  const mock = await import('../lib/ipc.mock');
  return { api: mock.createMockIpc({ stepMs: 1, trustAll: true, exampleRunMode: 'as_file' }), MOCK_MODE: true, isTauri: () => false };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let store: StoreApi<GraphStore>;

async function render(el: JSX.Element): Promise<void> {
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root?.render(el));
}
const click = async (el: Element | null | undefined): Promise<void> => {
  if (!el) throw new Error('element not found');
  await act(async () => (el as HTMLElement).click());
};
const button = (text: string): HTMLButtonElement | undefined =>
  [...document.querySelectorAll('button')].find((b) => b.textContent?.includes(text));
function typeInto(el: HTMLTextAreaElement, value: string): void {
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set?.call(el, value);
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

const session = (patch: Partial<TerminalSession>): TerminalSession => ({
  sid: 's1', title: 'Literature survey', node_ids: ['survey', 'design', 'report'], status: 'running', pid: 1, current_node: 'survey', ...patch,
});
function terminalRun(sessions: TerminalSession[]): RunState {
  const node = { status: 'running' as const, started_at: 't', finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null };
  return {
    run_id: 'R1', graph_dir: MOCK_EXAMPLE_DIR, started_at: '2026-11-07T10:00:00Z', finished_at: null, status: 'running',
    nodes: { survey: node, design: { ...node, status: 'pending', started_at: null }, report: { ...node, status: 'pending', started_at: null } },
    mode: 'terminal', sessions,
  };
}

beforeEach(async () => {
  store = createGraphStore(api, { autosaveMs: 60_000 });
  installStore(store);
  expect(await store.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
});
afterEach(async () => {
  vi.restoreAllMocks();
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
});

describe('SessionChips', () => {
  it('one chip per session with its status colour; a click opens that session window', async () => {
    const open = vi.spyOn(api, 'openSessionWindow').mockResolvedValue(undefined);
    await render(<SessionChips />);
    expect(document.querySelector('[aria-label="sessions"]')).toBeNull();
    await act(async () => store.getState().applyRunState(terminalRun([session({}), session({ sid: 's2', title: 'Solo', status: 'done', node_ids: [] })])));
    const chips = [...document.querySelectorAll('[aria-label="sessions"] button')] as HTMLButtonElement[];
    expect(chips.map((c) => c.textContent)).toEqual(['Literature survey', 'Solo']);
    expect(chips[0]?.style.borderColor).toBe('rgb(59, 130, 246)');
    expect(chips[1]?.style.borderColor).toBe('rgb(34, 197, 94)');
    await click(chips[1]);
    expect(open).toHaveBeenCalledWith('R1', 's2');
    expect(open).toHaveBeenCalledTimes(1);
  });

  it('never opens windows by itself (the backend does); a failed click becomes a toast', async () => {
    const open = vi.spyOn(api, 'openSessionWindow').mockRejectedValue({ code: 'internal', message: 'no window' });
    await render(<SessionChips />);
    await act(async () => store.getState().applyRunState(terminalRun([session({})])));
    await act(async () => store.getState().applyRunState(terminalRun([session({ current_node: 'design' })])));
    expect(open).not.toHaveBeenCalled();
    await click(document.querySelector('[aria-label="sessions"] button'));
    await act(async () => new Promise((r) => setTimeout(r, 0)));
    expect(open).toHaveBeenCalledTimes(1);
    expect(store.getState().toasts.map((t) => t.text)).toContain('Could not open the session window: no window');
  });

  it('headless runs show no chips', async () => {
    await render(<SessionChips />);
    await act(async () => store.getState().applyRunState({ ...terminalRun([session({})]), mode: 'headless' }));
    expect(document.querySelector('[aria-label="sessions"]')).toBeNull();
  });
});

describe('run-mode radio and Prompt note', () => {
  it('a graph.json without run_mode shows terminal; switching to headless updates settings and hides the note', async () => {
    expect(store.getState().graph?.settings.run_mode).toBeUndefined();
    await render(
      <>
        <SettingsPopover />
        <PromptTab node={store.getState().graph?.nodes[0] as never} />
      </>,
    );
    const radio = (v: string) => document.querySelector<HTMLInputElement>(`input[name="run_mode"][value="${v}"]`);
    expect(radio('terminal')?.checked).toBe(true);
    expect(radio('headless')?.checked).toBe(false);
    expect(document.body.textContent).toContain('Interactive windows (default)');
    expect(document.querySelector('[aria-label="terminal mode note"]')?.textContent).toBe(HEADLESS_ONLY_NOTE);
    await click(radio('headless'));
    expect(store.getState().graph?.settings.run_mode).toBe('headless');
    expect(radio('headless')?.checked).toBe(true);
    expect(document.querySelector('[aria-label="terminal mode note"]')).toBeNull();
  });

  it('new graphs default to terminal', async () => {
    expect(await store.getState().createGraph('/mock/fresh', 'fresh')).toBe(true);
    await render(<SettingsPopover />);
    expect(document.querySelector<HTMLInputElement>('input[value="terminal"]')?.checked).toBe(true);
  });
});

describe('Messages tab, terminal path', () => {
  it('types the text plus \\r into the live session', async () => {
    const write = vi.spyOn(api, 'ptyWrite').mockResolvedValue(undefined);
    await act(async () => store.getState().applyRunState(terminalRun([session({})])));
    await render(<MessagesTab nodeId="design" />);
    expect(document.body.textContent).toContain('Typed into the session window “Literature survey”.');
    const box = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="message to this node"]');
    expect(box?.disabled).toBe(false);
    await act(async () => typeInto(box as HTMLTextAreaElement, 'use the\nsecond dataset'));
    await click(button('Send'));
    expect(write).toHaveBeenCalledWith('R1', 's1', 'use the second dataset\r');
    expect(document.body.textContent).toContain('use the second dataset');
  });

  it('a closed session says so and offers Run from here instead of the composer', async () => {
    await act(async () => store.getState().applyRunState(terminalRun([session({ status: 'cancelled' })])));
    await render(<MessagesTab nodeId="survey" />);
    expect(document.body.textContent).toContain(SESSION_CLOSED_TEXT);
    expect(document.querySelector<HTMLTextAreaElement>('textarea')).toBeNull();
    const start = vi.spyOn(store.getState(), 'startRun');
    expect(button('Run from here')).toBeDefined();
    void start;
  });

  it('a failed write keeps the draft and shows a toast', async () => {
    vi.spyOn(api, 'ptyWrite').mockRejectedValue({ code: 'not_found', message: 'session gone' });
    await act(async () => store.getState().applyRunState(terminalRun([session({})])));
    await render(<MessagesTab nodeId="survey" />);
    const box = document.querySelector<HTMLTextAreaElement>('textarea') as HTMLTextAreaElement;
    await act(async () => typeInto(box, 'hello'));
    await click(button('Send'));
    expect(box.value).toBe('hello');
    const toast = store.getState().toasts.find((t) => t.text === 'Message to survey not sent: session gone');
    expect(toast?.action?.label).toBe('Retry');
  });

  it('terminalLine flattens line breaks', () => {
    expect(terminalLine('  a\r\n  b\nc ')).toBe('a b c');
  });
});

describe('Results tab', () => {
  it('shows "Open session window" while the node session is alive', async () => {
    const open = vi.spyOn(api, 'openSessionWindow').mockResolvedValue(undefined);
    await act(async () => store.getState().applyRunState(terminalRun([session({ status: 'done' })])));
    await render(<ResultsTab nodeId="report" />);
    await act(async () => new Promise((r) => setTimeout(r, 0)));
    await click(button('Open session window'));
    expect(open).toHaveBeenCalledWith('R1', 's1');
    await act(async () => store.getState().applyRunState({ ...terminalRun([session({ status: 'failed' })]), status: 'failed' }));
    expect(button('Open session window')).toBeUndefined();
  });
});
