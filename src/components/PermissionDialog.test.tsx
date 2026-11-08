/** "Ask me in the app" UI: dialog buttons, keys, queue, top-bar badge, card footer, log colours. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ReactFlowProvider } from '@xyflow/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PermissionAnswer, PermissionPrompt } from '../types/contract';
import { graphToFlowNodes } from '../lib/flow';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { createGraphStore, installStore } from '../store/graphStore';
import { kindLabel, lineTextColor } from './LogPanel';
import { ARM_DELAY_MS, PermissionDialog, PermissionDialogView, denyAnswer, prettyInput } from './PermissionDialog';
import { ApprovalsBadge } from './TopBar';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
});

async function render(el: JSX.Element): Promise<void> {
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root?.render(el));
}

const prompt = (id: string, node_id = 'survey'): PermissionPrompt => ({
  id,
  run_id: 'r1',
  node_id,
  tool_name: 'Bash',
  input: { command: 'echo hi', description: 'say hi' },
  summary: 'echo hi',
  suggestions: [],
  ts: '2026-11-07T10:15:00Z',
});

const button = (name: string): HTMLButtonElement => {
  const b = [...document.querySelectorAll('button')].find((x) => x.textContent?.trim() === name);
  if (!b) throw new Error(`no button ${name}`);
  return b;
};
const dialog = (): HTMLElement | null => document.querySelector('[role="alertdialog"]');
const press = async (target: Element, key: string): Promise<void> => {
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  });
};
const click = async (el: HTMLElement): Promise<void> => act(async () => el.click());
const typeInto = async (input: HTMLInputElement, value: string): Promise<void> => {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
    setter?.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
};

async function view(busy = false, armDelayMs = 0) {
  const onAnswer = vi.fn<(a: PermissionAnswer) => void>();
  const onHide = vi.fn();
  await render(
    <PermissionDialogView prompt={prompt('p1')} index={0} total={3} nodeTitle="Survey" busy={busy} armDelayMs={armDelayMs} onAnswer={onAnswer} onHide={onHide} />,
  );
  return { onAnswer, onHide };
}

describe('PermissionDialogView', () => {
  it('shows node title and id, tool, summary, raw input, queue position; aria wired', async () => {
    await view();
    const d = dialog();
    expect(d?.getAttribute('aria-modal')).toBe('true');
    expect(document.getElementById(d?.getAttribute('aria-labelledby') ?? '')?.textContent).toContain('Allow Bash?');
    expect(d?.getAttribute('aria-describedby')).toBeTruthy();
    expect(d?.textContent).toContain('Survey');
    expect(d?.textContent).toContain('survey');
    expect(document.querySelector('[aria-label="request summary"]')?.textContent).toBe('echo hi');
    expect(document.querySelector('[aria-label="raw input"]')?.textContent).toBe(prettyInput(prompt('p1').input));
    expect(document.querySelector('[aria-label="queue position"]')?.textContent).toBe('1 of 3');
    expect(document.activeElement?.textContent).toBe('Allow once');
  });

  it.each([
    ['Allow once', { id: 'p1', allow: true, scope: 'once' }],
    ['Allow for this node', { id: 'p1', allow: true, scope: 'node' }],
    ['Always for this graph', { id: 'p1', allow: true, scope: 'graph' }],
    ['Deny', { id: 'p1', allow: false, scope: 'once' }],
  ])('%s maps to %o', async (name, expected) => {
    const { onAnswer } = await view();
    await click(button(name));
    expect(onAnswer).toHaveBeenCalledWith(expected);
  });

  it('Deny carries the typed message', async () => {
    const { onAnswer } = await view();
    const input = document.querySelector('input') as HTMLInputElement;
    await typeInto(input, '  use Read instead ');
    await click(button('Deny'));
    expect(onAnswer).toHaveBeenCalledWith({ id: 'p1', allow: false, scope: 'once', message: 'use Read instead' });
  });

  it('Enter allows once, Esc denies', async () => {
    const { onAnswer } = await view();
    const d = dialog() as HTMLElement;
    await press(d, 'Enter');
    expect(onAnswer).toHaveBeenLastCalledWith({ id: 'p1', allow: true, scope: 'once' });
    await press(d, 'Escape');
    expect(onAnswer).toHaveBeenLastCalledWith({ id: 'p1', allow: false, scope: 'once' });
  });

  it('Enter in the message field denies with the message; Enter on a button keeps its native meaning', async () => {
    const { onAnswer } = await view();
    const input = document.querySelector('input') as HTMLInputElement;
    await typeInto(input, 'no');
    await press(input, 'Enter');
    expect(onAnswer).toHaveBeenLastCalledWith({ id: 'p1', allow: false, scope: 'once', message: 'no' });
    onAnswer.mockClear();
    await press(button('Always for this graph'), 'Enter');
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it('Tab wraps inside the dialog', async () => {
    await view();
    const allowOnce = button('Allow once');
    allowOnce.focus();
    await press(allowOnce, 'Tab');
    expect(dialog()?.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(allowOnce);
  });

  it('busy disables every answer, by click and by key', async () => {
    const { onAnswer } = await view(true);
    expect(button('Allow once').disabled).toBe(true);
    await press(dialog() as HTMLElement, 'Enter');
    await press(dialog() as HTMLElement, 'Escape');
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it('ignores Enter and the Allow buttons for ARM_DELAY_MS after the prompt is shown; Deny works at once', async () => {
    vi.useFakeTimers();
    try {
      const { onAnswer } = await view(false, ARM_DELAY_MS);
      const allowButtons = ['Allow once', 'Allow for this node', 'Always for this graph'].map(button);
      for (const b of allowButtons) {
        expect(b.getAttribute('aria-disabled')).toBe('true');
        expect(b.classList.contains('opacity-40')).toBe(true);
      }
      expect(document.activeElement?.textContent).toBe('Allow once');
      await press(dialog() as HTMLElement, 'Enter');
      for (const b of allowButtons) await click(b);
      expect(onAnswer).not.toHaveBeenCalled();

      await act(async () => vi.advanceTimersByTime(ARM_DELAY_MS - 1));
      await press(dialog() as HTMLElement, 'Enter');
      expect(onAnswer).not.toHaveBeenCalled();

      await act(async () => vi.advanceTimersByTime(1));
      expect(button('Allow once').getAttribute('aria-disabled')).toBe('false');
      expect(button('Allow once').classList.contains('opacity-40')).toBe(false);
      await press(dialog() as HTMLElement, 'Enter');
      expect(onAnswer).toHaveBeenLastCalledWith({ id: 'p1', allow: true, scope: 'once' });
    } finally {
      vi.useRealTimers();
    }
  });

  it('Deny and Esc are not delayed', async () => {
    const { onAnswer } = await view(false, ARM_DELAY_MS);
    await press(dialog() as HTMLElement, 'Escape');
    expect(onAnswer).toHaveBeenLastCalledWith({ id: 'p1', allow: false, scope: 'once' });
  });

  it('the next queued prompt is delayed again (a repeated Enter cannot allow it)', async () => {
    vi.useFakeTimers();
    try {
      const store = createGraphStore(createMockIpc({ seed: false }), { autosaveMs: 60_000 });
      installStore(store);
      const answer = vi.spyOn(store.getState(), 'answerPermission');
      await render(<PermissionDialog />);
      await act(async () => {
        store.getState().addPermissionPrompt(prompt('p1'));
        store.getState().addPermissionPrompt(prompt('p2'));
      });
      await act(async () => vi.advanceTimersByTime(ARM_DELAY_MS));
      await act(async () => store.getState().removePermissionPrompt('p1'));
      expect(document.querySelector('[aria-label="queue position"]')?.textContent).toBe('1 of 1');
      await press(dialog() as HTMLElement, 'Enter');
      expect(answer).not.toHaveBeenCalled();
      expect(button('Allow once').getAttribute('aria-disabled')).toBe('true');
    } finally {
      vi.useRealTimers();
    }
  });

  it('Decide later hides', async () => {
    const { onHide } = await view();
    await click(button('Decide later'));
    expect(onHide).toHaveBeenCalled();
  });

  it('denyAnswer omits an empty message', () => {
    expect(denyAnswer('x', '   ')).toEqual({ id: 'x', allow: false, scope: 'once' });
  });
});

describe('PermissionDialog with the store', () => {
  async function mounted() {
    const store = createGraphStore(createMockIpc({ trustAll: true }), { autosaveMs: 60_000 });
    installStore(store);
    await store.getState().openGraph(MOCK_EXAMPLE_DIR);
    await render(
      <ReactFlowProvider>
        <ApprovalsBadge />
        <PermissionDialog />
      </ReactFlowProvider>,
    );
    return store;
  }

  it('moves focus in on open and back where it was once the queue is empty', async () => {
    const store = await mounted();
    const outside = document.createElement('button');
    outside.textContent = 'outside';
    document.body.appendChild(outside);
    outside.focus();
    await act(async () => store.getState().addPermissionPrompt(prompt('p1')));
    expect(document.activeElement?.textContent).toBe('Allow once');
    await act(async () => store.getState().removePermissionPrompt('p1'));
    expect(document.activeElement).toBe(outside);
  });

  it('shows nothing and no badge while the queue is empty', async () => {
    await mounted();
    expect(dialog()).toBeNull();
    expect(document.querySelector('button')).toBeNull();
  });

  it('shows the oldest prompt first with "1 of N" and the badge count; hide and reopen via the badge', async () => {
    const store = await mounted();
    await act(async () => {
      store.getState().addPermissionPrompt(prompt('p1', 'survey'));
      store.getState().addPermissionPrompt(prompt('p2', 'design'));
    });
    expect(document.querySelector('[aria-label="queue position"]')?.textContent).toBe('1 of 2');
    expect(dialog()?.textContent).toContain('Literature survey');
    const badge = document.querySelector('button[aria-label="2 approvals needed"]') as HTMLButtonElement;
    expect(badge.textContent).toBe('2 approvals needed');
    expect(badge.className).toContain('bg-[#f59e0b]');

    await click(button('Decide later'));
    expect(dialog()).toBeNull();
    await click(badge);
    expect(dialog()).not.toBeNull();

    await act(async () => store.getState().removePermissionPrompt('p1'));
    expect(document.querySelector('[aria-label="queue position"]')?.textContent).toBe('1 of 1');
    expect(dialog()?.textContent).toContain('design');
    expect(document.querySelector('button[aria-label="1 approval needed"]')).not.toBeNull();
  });
});

describe('waiting card and log colours', () => {
  it('the card data marks nodes with a pending prompt', () => {
    const g = { version: 1, name: 'g', settings: {} as never, nodes: [{ id: 'a', harness: null, position: { x: 0, y: 0 } }], edges: [] } as never;
    expect(graphToFlowNodes(g, null, new Set(), (h) => h, new Set(['a']))[0]?.data.waiting).toBe(true);
    expect(graphToFlowNodes(g, null)[0]?.data.waiting).toBe(false);
  });

  it('permission_prompt and permission_answered are amber in the log', () => {
    expect(lineTextColor('permission_prompt')).toBe('#f59e0b');
    expect(lineTextColor('permission_answered')).toBe('#f59e0b');
    expect(lineTextColor('text')).toBeNull();
    expect(kindLabel('permission_prompt')).toBe('ASK');
    expect(kindLabel('permission_answered')).toBe('ANSWER');
    expect(kindLabel('permission_denied')).toBe('DENIED');
  });
});
