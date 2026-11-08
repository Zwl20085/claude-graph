/** Harness profiles in the UI: store loading, the Manage dialog, node select, card badge, session chip label. */
import { act, type ComponentProps } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ReactFlowProvider } from '@xyflow/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StoreApi } from 'zustand/vanilla';
import type { Api } from '../lib/api';
import { graphToFlowNodes } from '../lib/flow';
import { BUILTIN_HARNESSES, harnessLabel, NOT_CLAUDE_MODEL_NOTE, SECRETS_NOTE } from '../lib/harness';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { createGraphStore, installStore, type GraphStore } from '../store/graphStore';
import type { RunState, TerminalSession } from '../types/contract';
import { BriefNode } from './BriefNode';
import { HarnessDialog } from './HarnessDialog';
import { PromptTab } from './PromptTab';
import { SessionChips } from './SessionChips';
import { SettingsPopover } from './SettingsPopover';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let store: StoreApi<GraphStore>;
let api: Api;
const s = (): GraphStore => store.getState();

async function render(el: JSX.Element): Promise<void> {
  const host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root?.render(el));
}
const q = <T extends Element = HTMLElement>(sel: string): T | null => document.querySelector<T>(sel);
const button = (text: string): HTMLButtonElement | undefined =>
  [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === text);
const click = async (el: Element | null | undefined): Promise<void> => {
  if (!el) throw new Error('element not found');
  await act(async () => (el as HTMLElement).click());
};
/** The control inside the Field whose caption is `label`. */
function control<T extends HTMLElement>(label: string, tag = 'input, textarea, select'): T {
  const caption = [...document.querySelectorAll('span[id$="-label"]')].find((el) => el.textContent === label);
  const el = caption?.parentElement?.querySelector<T>(tag);
  if (!el) throw new Error(`no control for ${label}`);
  return el;
}
async function type(el: HTMLInputElement | HTMLTextAreaElement, value: string): Promise<void> {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  await act(async () => {
    Object.getOwnPropertyDescriptor(proto, 'value')?.set?.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
  });
}
async function choose(el: HTMLSelectElement, value: string): Promise<void> {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set?.call(el, value);
    el.dispatchEvent(new Event('change', { bubbles: true }));
  });
}

beforeEach(async () => {
  api = createMockIpc({ trustAll: true, stepMs: 1, exampleRunMode: 'as_file' });
  store = createGraphStore(api, { autosaveMs: 60_000 });
  installStore(store);
  expect(await s().openGraph(MOCK_EXAMPLE_DIR)).toBe(true);
});
afterEach(async () => {
  vi.restoreAllMocks();
  if (root) await act(async () => root?.unmount());
  root = null;
  document.body.innerHTML = '';
});

describe('harness store slice', () => {
  it('starts with the built-ins, init loads the machine list, save/delete replace it', async () => {
    expect(s().harnesses).toBe(BUILTIN_HARNESSES);
    const list = vi.spyOn(api, 'listHarnesses');
    await s().init();
    expect(list).toHaveBeenCalledTimes(1);
    expect(s().harnesses.map((p) => p.id)).toEqual(['claude', 'claude-deepseek', 'codex', 'deepseek-harness']);
    const mine = { ...BUILTIN_HARNESSES[0]!, id: 'mine', label: 'Mine', kind: 'custom' as const, command: 'mine', builtin: false };
    expect(await s().saveHarness(mine)).toBeNull();
    expect(s().harnesses.map((p) => p.id)).toContain('mine');
    expect(await s().deleteHarness('mine')).toBe(true);
    expect(s().harnesses.map((p) => p.id)).not.toContain('mine');
  });
  it('failures become error toasts', async () => {
    expect(await s().deleteHarness('claude')).toBe(false);
    vi.spyOn(api, 'listHarnesses').mockRejectedValue({ code: 'io', message: 'disk' });
    await s().loadHarnesses();
    expect(s().toasts.map((t) => t.text)).toEqual([
      expect.stringMatching(/Could not delete harness claude/),
      'Could not load harness profiles: disk',
    ]);
  });
});

describe('Settings: graph harness and Manage dialog', () => {
  it('the Harness select sets settings.harness; Manage opens the dialog', async () => {
    await render(<SettingsPopover />);
    await choose(control<HTMLSelectElement>('Harness', 'select'), 'codex');
    expect(s().graph?.settings.harness).toBe('codex');
    await click(q('[aria-label="manage harnesses"]'));
    expect(q('[role="dialog"][aria-modal="true"]')?.textContent).toContain('Harnesses');
  });

  it('lists profiles with kind badge and built-in marker; shows the secrets note', async () => {
    await render(<HarnessDialog onClose={() => undefined} />);
    const rows = [...document.querySelectorAll('nav[aria-label="Harness profiles"] li')];
    expect(rows).toHaveLength(4);
    expect(rows[2]?.querySelector('[aria-label="kind codex"]')?.textContent).toBe('Codex CLI');
    expect(rows.every((r) => r.querySelector('[aria-label="built in"]'))).toBe(true);
    expect(q('[aria-label="secrets note"]')?.textContent).toBe(SECRETS_NOTE);
    // Built-ins: label read-only, no delete.
    expect(control<HTMLInputElement>('Label').readOnly).toBe(true);
    expect(q('[aria-label^="delete harness"]')).toBeNull();
  });

  it('new custom: validation errors show inline and block the save; a fixed form saves and lists it', async () => {
    const save = vi.spyOn(api, 'saveHarness');
    await render(<HarnessDialog onClose={() => undefined} />);
    await click(button('New custom harness'));
    expect(control<HTMLInputElement>('Id').value).toBe('custom-1');
    await type(control<HTMLInputElement>('Command'), 'mycli.exe');
    await type(control<HTMLTextAreaElement>('Environment'), 'BAD-KEY=1');
    await click(button('Save'));
    expect(q('form [role="alert"]')?.textContent).toMatch(/^env: line 1/);
    expect(save).not.toHaveBeenCalled();

    await type(control<HTMLTextAreaElement>('Environment'), 'API_KEY=${MY_KEY}');
    await type(control<HTMLInputElement>('Id'), 'Bad Id');
    await click(button('Save'));
    expect(q('form [role="alert"]')?.textContent).toMatch(/^id:/);

    await type(control<HTMLInputElement>('Id'), 'my-cli');
    await type(control<HTMLTextAreaElement>('Dialog markers'), 'Trust?\n\nAllow?');
    await click(button('Save'));
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0]?.[0]).toMatchObject({
      id: 'my-cli', kind: 'custom', command: 'mycli.exe', env: { API_KEY: '${MY_KEY}' }, dialog_markers: ['Trust?', 'Allow?'], builtin: false,
    });
    expect(s().harnesses.map((p) => p.id)).toContain('my-cli');
    expect(q('form')?.getAttribute('aria-label')).toBe('Edit harness my-cli');
    expect(control<HTMLInputElement>('Id').readOnly).toBe(true);
    expect(q('form [role="alert"]')).toBeNull();
  });

  it('Check shows the result inline; Delete removes a custom', async () => {
    await s().saveHarness({ ...BUILTIN_HARNESSES[0]!, id: 'gone', label: 'Gone', kind: 'custom', command: 'FAKE_MISSING', builtin: false });
    await render(<HarnessDialog onClose={() => undefined} />);
    await click(q('[aria-label="edit harness Codex CLI"]'));
    await click(button('Check'));
    expect(q('[aria-label="harness check result"]')?.textContent).toBe('codex 1.0.0 (mock)');
    await click(q('[aria-label="edit harness Gone"]'));
    await click(button('Check'));
    expect(q('[aria-label="harness check result"]')?.textContent).toBe('FAKE_MISSING was not found (mock)');
    await click(q('[aria-label="delete harness gone"]'));
    expect(s().harnesses.map((p) => p.id)).not.toContain('gone');
  });
});

describe('per-node harness', () => {
  it('Inherit + labels; a non-claude harness disables the model select with a note', async () => {
    await s().init();
    const node = () => s().graph?.nodes.find((n) => n.id === 'design');
    const Tab = () => {
      const n = node();
      return n ? <PromptTab node={n} /> : null;
    };
    const { useGraphStore } = await import('../store/graphStore');
    const Live = () => {
      useGraphStore((st) => st.graph);
      return <Tab />;
    };
    await render(<Live />);
    const select = control<HTMLSelectElement>('Harness', 'select');
    expect([...select.options].map((o) => o.textContent)).toEqual(['Inherit (Claude Code)', 'Claude Code', 'Claude Code via DeepSeek API', 'Codex CLI', 'DeepSeek Harness (dsh)']);
    expect(control<HTMLSelectElement>('Model', 'select').disabled).toBe(false);
    await choose(select, 'codex');
    expect(node()?.harness).toBe('codex');
    expect(control<HTMLSelectElement>('Model', 'select').disabled).toBe(true);
    expect(document.body.textContent).toContain(NOT_CLAUDE_MODEL_NOTE);
    await choose(control<HTMLSelectElement>('Harness', 'select'), 'claude-deepseek');
    expect(control<HTMLSelectElement>('Model', 'select').disabled).toBe(false);
    await choose(control<HTMLSelectElement>('Harness', 'select'), '');
    expect(node()?.harness).toBeNull();
  });

  it('the node card shows a harness badge only when the node overrides the default', async () => {
    const g = s().graph;
    if (!g) throw new Error('no graph');
    const withOverride = { ...g, nodes: g.nodes.map((n) => (n.id === 'design' ? { ...n, harness: 'codex' } : n)) };
    const flow = graphToFlowNodes(withOverride, null, new Set(), (id) => harnessLabel(BUILTIN_HARNESSES, id));
    const card = (i: number) => {
      const n = flow[i]!;
      const props = { id: n.id, data: n.data, selected: false, type: 'brief' } as unknown as ComponentProps<typeof BriefNode>;
      return <BriefNode key={n.id} {...props} />;
    };
    await render(<ReactFlowProvider>{card(0)}{card(1)}</ReactFlowProvider>);
    const badges = [...document.querySelectorAll('[aria-label^="harness "]')].map((b) => b.textContent);
    expect(badges).toEqual(['Codex CLI']);
  });
});

describe('session chips', () => {
  const session = (patch: Partial<TerminalSession>): TerminalSession => ({
    sid: 's1', title: 'Survey', node_ids: ['survey'], status: 'running', pid: 1, current_node: 'survey', ...patch,
  });
  const run = (sessions: TerminalSession[]): RunState => ({
    run_id: 'R1', graph_dir: MOCK_EXAMPLE_DIR, started_at: 't', finished_at: null, status: 'running', nodes: {}, mode: 'terminal', sessions,
  });

  it('show the harness label for non-claude sessions (title suffix moved to a badge)', async () => {
    await render(<SessionChips />);
    await act(async () =>
      s().applyRunState(run([session({}), session({ sid: 's2', title: 'Design (Codex CLI)', harness: 'codex', node_ids: ['design'] })])),
    );
    const chips = [...document.querySelectorAll('[aria-label="Claude Code sessions"] button')];
    expect(chips.map((c) => c.textContent)).toEqual(['Survey', 'DesignCodex CLI']);
    expect(chips[0]?.getAttribute('aria-label')).toMatch(/harness Claude Code$/);
    expect(chips[1]?.getAttribute('aria-label')).toMatch(/harness Codex CLI$/);
  });

  it('a dsh session says it is headless and chained; a waiting session says needs you', async () => {
    await render(<SessionChips />);
    await act(async () =>
      s().applyRunState(
        run([
          session({ title: 'Survey (DeepSeek Harness (dsh))', harness: 'deepseek-harness', pending_prompt: 'survey-p1' }),
        ]),
      ),
    );
    const chip = document.querySelector('[aria-label="Claude Code sessions"] button');
    expect(chip?.textContent).toBe('Surveyneeds youdsh (headless, chained)');
    expect(chip?.getAttribute('aria-label')).toContain('needs you');
  });
});
