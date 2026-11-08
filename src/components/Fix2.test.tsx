/** Fix pass 2 UI: permission select labels, Results "Denied tools", blocked banner, DENIED log label, card badge. */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api } from '../lib/api';
import { graphToFlowNodes } from '../lib/flow';
import { createMockIpc, MOCK_EXAMPLE_DIR } from '../lib/ipc.mock';
import { DENIED_HINT } from '../lib/status';
import { createGraphStore, installStore } from '../store/graphStore';
import { Inspector } from './Inspector';
import { LogPanel } from './LogPanel';
import { PromptTab } from './PromptTab';
import { ResultsTab } from './ResultsTab';
import { SettingsPopover } from './SettingsPopover';

// Components call the app-wide `api`: make it a fast, trusting mock for this file.
vi.mock('../lib/api', async () => {
  const mock = await import('../lib/ipc.mock');
  return { api: mock.createMockIpc({ stepMs: 1, trustAll: true }), MOCK_MODE: true, isTauri: () => false };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const DIR = MOCK_EXAMPLE_DIR;
let root: Root | null = null;
const cleanups: Array<() => void> = [];
afterEach(async () => {
  cleanups.splice(0).forEach((off) => off());
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
const wait = (ms: number) => act(async () => new Promise((r) => setTimeout(r, ms)));
const optionTexts = (select: Element | null | undefined): string[] =>
  [...(select?.querySelectorAll('option') ?? [])].map((o) => o.textContent ?? '');

/** Store on the (mocked) app api with the example open; `prompt` set on survey, then one run from survey to the end. */
async function ranStore(prompt: string) {
  const store = createGraphStore(api, { autosaveMs: 60_000 });
  installStore(store);
  const offState = await api.onRunState((rs) => store.getState().applyRunState(rs));
  const offEvent = await api.onNodeEvent((ev) => store.getState().appendEvent(ev));
  cleanups.push(offState, offEvent);
  await store.getState().openGraph(DIR);
  store.getState().updateNode('survey', { prompt });
  expect(await store.getState().startRun({ from: 'survey' })).toBe(true);
  for (let i = 0; i < 50 && store.getState().runState?.status === 'running'; i += 1) {
    await new Promise((r) => setTimeout(r, 5));
  }
  return store;
}

describe('permission selects', () => {
  it('node and graph-default selects show "Claude Code settings (recommended)" first', async () => {
    const store = createGraphStore(createMockIpc({ trustAll: true }), { autosaveMs: 60_000 });
    installStore(store);
    await store.getState().openGraph(DIR);
    const node = store.getState().graph?.nodes[0];
    if (!node) throw new Error('no node');
    await render(
      <>
        <PromptTab node={node} />
        <SettingsPopover />
      </>,
    );
    const selects = [...document.querySelectorAll('select')];
    const nodeSelect = selects.find((s) => (s as HTMLSelectElement).value === 'inherit');
    expect(optionTexts(nodeSelect)).toEqual([
      'Claude Code settings (recommended)', 'Ask me in the app', 'acceptEdits', 'default', 'plan', 'dontAsk', 'bypassPermissions', 'inherit',
    ]);
    const graphSelect = selects.find((s) => optionTexts(s).length === 7);
    expect(optionTexts(graphSelect)[0]).toBe('Claude Code settings (recommended)');
    expect((graphSelect?.querySelector('option') as HTMLOptionElement).value).toBe('claude_settings');
  });
});

describe('denials in the UI', () => {
  it('Results tab shows the Denied tools line; the log labels the event DENIED; the card data is flagged', async () => {
    const store = await ranStore('search FAKE_DENY');
    const runId = store.getState().runState?.run_id as string;
    await act(async () => store.getState().select('survey'));
    await render(
      <>
        <ResultsTab nodeId="survey" />
        <LogPanel />
      </>,
    );
    await wait(20);
    // The latest run of the node opens by itself.
    expect(document.querySelector('[aria-label="runs of this node"]')?.textContent).toContain(runId);
    const line = document.querySelector('[aria-label="denied tools"]');
    expect(line?.textContent).toBe(`Denied tools: WebSearch. ${DENIED_HINT}`);
    expect(document.body.textContent).toContain('DENIED');

    const s = store.getState();
    const graph = s.graph;
    if (!graph) throw new Error('no graph');
    const flags = graphToFlowNodes(graph, s.runState).map((n) => [n.id, n.data.denied]);
    expect(flags).toEqual([['survey', true], ['design', false], ['report', false]]);
    const live = graphToFlowNodes(graph, { ...(s.runState as NonNullable<typeof s.runState>), nodes: {} }, new Set(['design']));
    expect(live.find((n) => n.id === 'design')?.data.denied).toBe(true);
  });
});

describe('blocked node in the inspector', () => {
  it('shows the banner and its button opens the Chat tab', async () => {
    const store = await ranStore('ask me FAKE_BLOCKED');
    expect(store.getState().runState?.nodes.survey?.status).toBe('blocked');
    await act(async () => store.getState().select('survey'));
    await render(<Inspector />);
    const banner = document.querySelector('[aria-label="node blocked"]');
    expect(banner?.textContent).toContain(
      'This node stopped and asked you something. Read its reply and answer in the Chat tab; a DONE reply will mark it succeeded.',
    );
    await act(async () => (document.querySelector('button[aria-label="open Chat"]') as HTMLButtonElement).click());
    const selected = document.querySelector('[role="tab"][aria-selected="true"]');
    expect(selected?.textContent).toBe('Chat');

    await act(async () => store.getState().select('report'));
    expect(document.querySelector('[aria-label="node blocked"]')).toBeNull();
  });

  it('shows the id helper text while the id follows the title', async () => {
    const store = createGraphStore(createMockIpc({ trustAll: true }), { autosaveMs: 60_000 });
    installStore(store);
    await store.getState().openGraph(DIR);
    store.getState().addNodeAt({ x: 0, y: 0 });
    await render(<Inspector />);
    expect(document.body.textContent).toContain('Follows the title until you edit it, or until the node has run.');
  });
});
