/** Fix pass 3 (v0.5 audit): dialog-safe composer, per-prompt cards, Rust option patterns, liveness, trust, harness guards. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RunState, SessionPrompt, TerminalSession } from '../types/contract';
import { api } from '../lib/api';
import { MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { isSessionAlive } from '../lib/sessions';
import { optionPatternError } from '../lib/validate';
import { trustItems, trustItemText } from '../lib/trust';
import { isHarnessProfile, validProfiles } from '../store/harnessSlice';
import { createGraphStore, installStore, type GraphStore } from '../store/graphStore';
import type { StoreApi } from 'zustand/vanilla';
import { MessagesTab } from './MessagesTab';
import { OPTIONS_ONLY_TEXT } from './TerminalMessages';
import { BUILTIN_HARNESSES } from '../lib/harness';

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
  sid: 's1', title: 'Literature survey', node_ids: ['survey'], status: 'running', pid: 1, current_node: 'survey', ...patch,
});
function terminalRun(sessions: TerminalSession[]): RunState {
  const node = { status: 'waiting' as const, started_at: 't', finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null };
  return {
    run_id: 'R1', graph_dir: MOCK_EXAMPLE_DIR, started_at: '2026-11-07T10:00:00Z', finished_at: null, status: 'running',
    nodes: { survey: node }, mode: 'terminal', sessions,
  };
}
const dialog = (id: string, allowsText: boolean, labels: string[] = ['Yes', 'No']): SessionPrompt => ({
  run_id: 'R1',
  sid: 's1',
  node_id: 'survey',
  prompt: {
    type: 'dialog', id, kind: allowsText ? 'choice' : 'permission', question: 'Proceed?',
    options: labels.map((label, index) => ({ index, label })), selected: 0, multi: true, allows_text: allowsText, source: 'screen',
  },
});

beforeEach(async () => {
  store = createGraphStore(api, { autosaveMs: 60_000 });
  installStore(store);
  expect(await store.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});

describe('composer while a dialog is open', () => {
  it('a dialog without a text option disables the composer: keys would pick an option', async () => {
    const write = vi.spyOn(api, 'ptyWrite').mockResolvedValue(undefined);
    await act(async () => store.getState().applyRunState(terminalRun([session({ pending_prompt: 'p1' })])));
    await act(async () => store.getState().addSessionPrompt(dialog('p1', false)));
    await render(<MessagesTab nodeId="survey" />);
    const box = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="message to this node"]');
    expect(box?.disabled).toBe(true);
    expect(box?.placeholder).toBe(OPTIONS_ONLY_TEXT);
    expect(document.body.textContent).toContain(OPTIONS_ONLY_TEXT);
    expect(write).not.toHaveBeenCalled();
  });

  it('a dialog with a text option takes the composer text as its answer, never raw keys', async () => {
    const write = vi.spyOn(api, 'ptyWrite').mockResolvedValue(undefined);
    const answer = vi.spyOn(api, 'answerPrompt').mockResolvedValue(undefined);
    await act(async () => store.getState().applyRunState(terminalRun([session({ pending_prompt: 'p1' })])));
    await act(async () => store.getState().addSessionPrompt(dialog('p1', true)));
    await render(<MessagesTab nodeId="survey" />);
    const box = document.querySelector<HTMLTextAreaElement>('textarea[aria-label="message to this node"]') as HTMLTextAreaElement;
    expect(box.disabled).toBe(false);
    await act(async () => typeInto(box, 'my own'));
    await click(button('Send'));
    expect(answer).toHaveBeenCalledWith('R1', 's1', 'p1', { text: 'my own' });
    expect(write).not.toHaveBeenCalled();
  });

  it('a new prompt on the same node starts with a fresh card (no carried ticks)', async () => {
    await act(async () => store.getState().applyRunState(terminalRun([session({ pending_prompt: 'p1' })])));
    await act(async () => store.getState().addSessionPrompt(dialog('p1', false, ['A', 'B', 'C'])));
    await render(<MessagesTab nodeId="survey" />);
    const boxes = (): HTMLInputElement[] => [...document.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
    await click(boxes()[0]);
    expect(boxes()[0]?.checked).toBe(true);
    await act(async () => store.getState().addSessionPrompt(dialog('p2', false, ['A', 'B', 'C'])));
    expect(boxes()[0]?.checked).toBe(false);
  });

  it('the options are labelled by the question', async () => {
    await act(async () => store.getState().applyRunState(terminalRun([session({ pending_prompt: 'p1' })])));
    await act(async () => store.getState().addSessionPrompt(dialog('p1', false)));
    await render(<MessagesTab nodeId="survey" />);
    const group = document.querySelector('[role="group"]');
    const label = group?.getAttribute('aria-labelledby') ?? '';
    expect(document.getElementById(label)?.textContent).toBe('Proceed?');
  });
});

describe('small fixes', () => {
  it('Rust-only inline flags are left to the backend; other bad patterns are caught', () => {
    expect(optionPatternError('(?i)^\\s*(?P<num>\\d+)\\.\\s+(?P<label>.+)')).toBeNull();
    expect(optionPatternError('^(?P<label>[a-z]+')).toBe('Not a valid regular expression.');
    expect(optionPatternError('^(?P<label>.+)$')).toBeNull();
  });

  it('a chained session with no pid is alive while it takes messages', () => {
    expect(isSessionAlive({ status: 'done', pid: null, alive: true })).toBe(true);
    expect(isSessionAlive({ status: 'done', pid: 5, alive: false })).toBe(false);
    expect(isSessionAlive({ status: 'done', pid: 5 })).toBe(true);
    expect(isSessionAlive({ status: 'failed', pid: null, alive: true })).toBe(false);
  });

  it('a prompt starting with ! is a shell-command trust item; / is listed', () => {
    const graph = store.getState().graph;
    if (!graph) throw new Error('no graph');
    const [first, second] = graph.nodes;
    if (!first || !second) throw new Error('example has nodes');
    const g = { ...graph, nodes: [{ ...first, prompt: '  !rm -rf build' }, { ...second, prompt: '/compact' }] };
    const items = trustItems(g).filter((i) => i.kind === 'shell_prompt' || i.kind === 'slash_prompt');
    expect(items.map((i) => i.kind)).toEqual(['shell_prompt', 'slash_prompt']);
    expect(trustItemText(items[0] as (typeof items)[number])).toContain('types shell commands into the agent');
  });

  it('harness lists from the backend are shape-checked', () => {
    expect(BUILTIN_HARNESSES.every(isHarnessProfile)).toBe(true);
    vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    expect(validProfiles([{ id: 'x' }, ...BUILTIN_HARNESSES])).toHaveLength(BUILTIN_HARNESSES.length);
    expect(validProfiles('nope')).toEqual([]);
  });
});
