/** Renders the whole UI in jsdom against the mock backend to catch runtime errors. */
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { api } from './lib/api';
import { MOCK_EXAMPLE_DIR } from './lib/ipc.mock';
import { createGraphStore, installStore } from './store/graphStore';
import App from './App';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

beforeAll(() => {
  class RO {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  (globalThis as unknown as { ResizeObserver: typeof RO }).ResizeObserver = RO;
  class DM {
    m22 = 1;
    constructor() {}
  }
  (globalThis as unknown as { DOMMatrixReadOnly: typeof DM }).DOMMatrixReadOnly = DM;
});

afterEach(() => {
  document.body.innerHTML = '';
});

const click = async (el: Element | null | undefined): Promise<void> => {
  if (!el) throw new Error('element not found');
  await act(async () => {
    (el as HTMLElement).click();
  });
};
const button = (text: string): HTMLButtonElement | undefined =>
  [...document.querySelectorAll('button')].find((b) => b.textContent?.includes(text));

describe('App smoke (mock mode)', () => {
  it('start screen -> open example -> select node -> each inspector tab -> run', async () => {
    const errors = vi.spyOn(console, 'error');
    const store = createGraphStore(api);
    installStore(store);
    void api.onRunState((s) => store.getState().applyRunState(s));
    void api.onNodeEvent((e) => store.getState().appendEvent(e));
    await store.getState().init();

    const host = document.createElement('div');
    host.style.width = '1200px';
    host.style.height = '800px';
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => root.render(<App />));
    expect(document.body.textContent).toContain('Recent graphs');

    await click(button(MOCK_EXAMPLE_DIR));
    expect(document.title).toBe('Claude Graph — hello-pipeline');
    expect(document.querySelectorAll('.react-flow__node').length).toBe(3);
    expect(document.body.textContent).toContain('Literature survey');

    await act(async () => store.getState().select('design'));
    for (const tab of ['Brief', 'Chat', 'Results', 'Prompt']) await click(button(tab));
    await click(button('Preview brief'));
    await act(async () => new Promise((r) => setTimeout(r, 500)));
    expect(document.body.textContent).toContain('# Node: Layout rule set');

    await act(async () => store.getState().updateNode('design', { title: 'Rules' }));
    expect(document.title).toBe('Claude Graph — hello-pipeline*');

    // Escape closes the preview dialog.
    await act(async () => {
      document.querySelector('[role="dialog"]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    expect(document.querySelector('[role="dialog"]')).toBeNull();

    // The example's design node allows Bash(python:*): the trust dialog asks first.
    await click(button('Run all'));
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain('Node design allows Bash tools');
    await click(button('Trust this graph and run'));
    await act(async () => new Promise((r) => setTimeout(r, 50)));
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(store.getState().runState?.status).toBe('running');

    await click(document.querySelector('[aria-label="File"]'));
    await click(button('Show in file manager'));
    await click(document.querySelector('[aria-label="settings"]'));
    expect(document.body.textContent).toContain('This machine');
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });
    expect(document.body.textContent).not.toContain('This machine');
    await act(async () => root.unmount());
    expect(errors).not.toHaveBeenCalled();
  });
});
