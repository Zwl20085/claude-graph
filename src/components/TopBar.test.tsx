/** Top bar (v0.6): File menu, Run split-button, run chip, needs-you badge, bell, one line. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ReactFlowProvider } from '@xyflow/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StoreApi } from 'zustand/vanilla';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import type { Api } from '../lib/api';
import type { SessionPrompt } from '../types/contract';
import { createGraphStore, installStore, type GraphStore } from '../store/graphStore';
import { TopBar } from './TopBar';
import { harnessChipText, needYouText, runChipText } from './TopBarParts';
import { BUILTIN_HARNESSES } from '../lib/harness';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let store: StoreApi<GraphStore>;
let api: Api;

const q = <T extends Element = HTMLElement>(sel: string): T | null => document.querySelector<T>(sel);
const button = (text: string): HTMLButtonElement | undefined =>
  [...document.querySelectorAll('button')].find((b) => b.textContent?.includes(text));
const click = async (el: Element | null | undefined): Promise<void> => {
  if (!el) throw new Error('element not found');
  await act(async () => (el as HTMLElement).click());
};

beforeEach(async () => {
  api = createMockIpc({ trustAll: true, exampleRunMode: 'as_file' });
  store = createGraphStore(api, { autosaveMs: 60_000 });
  installStore(store);
  expect(await store.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () =>
    root?.render(
      <ReactFlowProvider>
        <TopBar logVisible={false} onToggleLog={() => {}} onOpenAssistant={() => {}} />
      </ReactFlowProvider>,
    ),
  );
});

afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});

describe('TopBar', () => {
  it('stays one line; the File menu holds New, Open graph, Show in file manager and Save', async () => {
    const header = q('header');
    expect(header?.className).toContain('flex-nowrap');
    expect(q('[role="menu"]')).toBeNull();
    await click(q('button[aria-label="File"]'));
    const items = [...document.querySelectorAll('[role="menu"] [role="menuitem"]')].map((b) => b.textContent);
    expect(items).toEqual(['New graph…', 'Open graph…', 'Show in file manager', 'SaveCtrl+S']);
    // A chosen item closes the menu.
    await click(button('Save'));
    expect(q('[role="menu"]')).toBeNull();
  });

  it('Run all is a split button: Run from selected and Rerun failed sit in its dropdown', async () => {
    expect(q<HTMLButtonElement>('button[aria-label="Run all"]')?.title).toBe('Run all (Ctrl+Enter)');
    await click(q('button[aria-label="more run options"]'));
    const fromSelected = button('Run from selected');
    expect(fromSelected?.disabled).toBe(true);
    await click(q('button[aria-label="more run options"]'));
    await act(async () => store.getState().select('design'));
    await click(q('button[aria-label="more run options"]'));
    expect(button('Run from selected')?.disabled).toBe(false);
    expect(button('Rerun failed')?.disabled).toBe(true);
  });

  it('the run chip names the mode and harness and opens Settings', async () => {
    const chip = q<HTMLButtonElement>('button[aria-label^="run mode"]');
    expect(chip?.textContent).toBe('Windows · Claude Code');
    await click(chip);
    expect(q('[role="dialog"][aria-label="Settings"]')).not.toBeNull();
    await act(async () => store.getState().updateSettings({ run_mode: 'headless' }));
    expect(q('button[aria-label^="run mode"]')?.textContent).toBe('Headless · claude');
  });

  it('a waiting prompt shows "N need you"; a click reveals the oldest node on Chat', async () => {
    expect(q('button[aria-label$="you"]')).toBeNull();
    const sp = (id: string, node: string, sid: string): SessionPrompt => ({
      run_id: 'R1', sid, node_id: node,
      prompt: { type: 'reply', id, question: 'Which?', choices: [], reply_excerpt: 'Which?' },
    });
    await act(async () => store.getState().addSessionPrompt(sp('p1', 'design', 's1')));
    await act(async () => store.getState().addSessionPrompt(sp('p2', 'report', 's2')));
    const badge = q<HTMLButtonElement>(`button[aria-label="${needYouText(2)}"]`);
    expect(badge?.textContent).toBe('2 need you');
    const seq = store.getState().revealSeq;
    await click(badge);
    expect(store.getState().selectedNodeId).toBe('design');
    expect(store.getState().inspectorTab).toBe('chat');
    expect(store.getState().revealSeq).toBe(seq + 1);
  });

  it('the bell lists recent notifications', async () => {
    await act(async () => store.getState().pushToast('error', 'Something failed', 'design'));
    await click(q('button[aria-label="notifications"]'));
    expect(q('[role="dialog"][aria-label="Notifications"]')?.textContent).toContain('Something failed');
    await click(button('Show'));
    expect(store.getState().selectedNodeId).toBe('design');
  });
});

describe('top bar texts', () => {
  it('run chip and harness chip follow the default harness', () => {
    const dsh = BUILTIN_HARNESSES.find((h) => h.kind === 'deepseek-harness');
    if (dsh) expect(runChipText({ harness: dsh.id } as never, BUILTIN_HARNESSES)).toBe('Windows · dsh (headless)');
    const codex = BUILTIN_HARNESSES.find((h) => h.id === 'codex') ?? null;
    expect(harnessChipText(codex, 'codex', { kind: 'checking' }, { kind: 'done', check: { id: 'codex', ok: false, version: null, message: 'x' } }))
      .toEqual({ ok: false, text: 'Codex CLI not found' });
    expect(harnessChipText(codex, 'codex', { kind: 'checking' }, { kind: 'done', check: { id: 'codex', ok: true, version: '0.167.0', message: null } }).text)
      .toBe('Codex CLI 0.167.0');
    expect(harnessChipText(null, 'claude', { kind: 'error', message: 'no' }, null)).toEqual({
      ok: false,
      text: 'claude not found — install Claude Code and log in',
    });
    expect(needYouText(1)).toBe('1 needs you');
  });
});
