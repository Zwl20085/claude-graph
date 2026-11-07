/** Side tab switching in the whole App (mock mode): Assistant default, node selection, TopBar pill, scheduler log. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import App from '../App';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { ASSISTANT_RELOAD_MS, createAssistantStore, installAssistantStore } from '../store/assistantStore';
import { createGraphStore, installStore } from '../store/graphStore';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const REPLY = 50;

beforeAll(() => {
  class RO {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  (globalThis as unknown as { ResizeObserver: typeof RO }).ResizeObserver = RO;
  class DM {
    m22 = 1;
  }
  (globalThis as unknown as { DOMMatrixReadOnly: typeof DM }).DOMMatrixReadOnly = DM;
});

let root: Root | null = null;
beforeEach(() => localStorage.clear());
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
  vi.useRealTimers();
});

const tab = (name: string): HTMLButtonElement | undefined =>
  [...document.querySelectorAll<HTMLButtonElement>('[role="tab"]')].find((b) => b.textContent === name);
const byLabel = (label: string): HTMLButtonElement | null => document.querySelector(`button[aria-label="${label}"]`);
async function click(el: Element | null | undefined): Promise<void> {
  if (!el) throw new Error('element not found');
  await act(async () => (el as HTMLElement).click());
}

describe('side panel tabs', () => {
  it('Assistant by default; node selection switches to Node; the TopBar pill opens Assistant', async () => {
    const api = createMockIpc({ assistantReplyMs: REPLY, trustAll: true });
    const graph = createGraphStore(api);
    installStore(graph);
    installAssistantStore(createAssistantStore(api, () => graph));
    void api.onNodeEvent((e) => graph.getState().appendEvent(e));
    expect(await graph.getState().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);

    const host = document.createElement('div');
    document.body.appendChild(host);
    root = createRoot(host);
    await act(async () => root?.render(<App />));
    await act(async () => new Promise((r) => setTimeout(r, 20)));

    expect(document.querySelector('aside')?.className).toContain('w-[400px]');
    expect(tab('Assistant')?.getAttribute('aria-selected')).toBe('true');
    expect(byLabel('run the workflow')).not.toBeNull();
    expect(byLabel('open the assistant tab')).toBeNull();

    await act(async () => graph.getState().select('design'));
    expect(tab('Node')?.getAttribute('aria-selected')).toBe('true');
    expect(document.querySelector('input[aria-label="Node title"]')).not.toBeNull();
    expect(localStorage.getItem('cg.sideTab.assistant')).toBe('0');

    await click(tab('Assistant'));
    expect(byLabel('run the workflow')).not.toBeNull();

    // Sending keeps the Assistant tab; the pill shows while working; the log shows the scheduler.
    await click(byLabel('run the workflow'));
    expect(tab('Assistant')?.getAttribute('aria-selected')).toBe('true');
    expect(byLabel('open the assistant tab')?.textContent).toBe('Working…');
    await act(async () => new Promise((r) => setTimeout(r, REPLY + ASSISTANT_RELOAD_MS + 30)));
    expect(byLabel('open the assistant tab')?.textContent).toBe('Waiting for your answer');
    expect(document.body.textContent).toContain('Write assistant/PLAN.md');

    await click(tab('Node'));
    expect(document.body.textContent).not.toContain('Write assistant/PLAN.md');
    await click(byLabel('open the assistant tab'));
    expect(tab('Assistant')?.getAttribute('aria-selected')).toBe('true');
  });
});
