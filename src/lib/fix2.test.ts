/** Fix pass 2 (FIX2_CONTRACT): permission mode, denials, blocked status, ids following titles, slash line. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import example from '../../examples/hello-pipeline/graph.json';
import { isRunState, NODE_EVENT_KINDS, NODE_STATUSES, PERMISSION_MODES, type Graph, type NodeEvent, type RunState } from '../types/contract';
import { deniedToolName, idIsEditable } from '../store/runHelpers';
import { renderBrief, SLASH_COMMAND_LINE } from './brief';
import { addNode, connect, emptyGraph, followsTitle, retitleNode, targetSet, updateNode } from './graphOps';
import { createMockIpc, MOCK_EXAMPLE_DIR } from './ipc.mock';
import { composerState } from './messages';
import { FAKE_BLOCKED, FAKE_DENY, MOCK_BLOCKED_QUESTION, MOCK_DENIED_TOOL } from './mockRunner';
import { edgeVisual, permissionLabel, ringStyle, STATUS_COLORS, statusLabel } from './status';
import { trustItems } from './trust';
import { validateGraph } from './validate';

const load = (): Graph => {
  const g = validateGraph(JSON.parse(JSON.stringify(example)));
  if (g instanceof Error) throw g;
  return g;
};

describe('permission mode claude_settings', () => {
  it('is a contract value, the new-graph default, labelled as recommended and listed first', () => {
    expect(PERMISSION_MODES).toContain('claude_settings');
    expect(emptyGraph('g').settings.default_permission_mode).toBe('claude_settings');
    expect(PERMISSION_MODES.filter((m) => m !== 'inherit').map(permissionLabel)).toEqual([
      'Claude Code settings (recommended)',
      'Ask me in the app',
      'acceptEdits',
      'default',
      'plan',
      'dontAsk',
      'bypassPermissions',
    ]);
  });

  it('is not elevated (no trust item) and round-trips through validation', () => {
    const g = updateNode(emptyGraph('g'), 'x', {});
    const { graph, id } = addNode(g, 'A', { x: 0, y: 0 });
    const withMode = updateNode(graph, id, { permission_mode: 'claude_settings' });
    expect(trustItems(withMode)).toEqual([]);
    const parsed = validateGraph(JSON.parse(JSON.stringify(withMode)));
    expect(parsed instanceof Error ? parsed.message : parsed.nodes[0]?.permission_mode).toBe('claude_settings');
  });

  it('the mock create_graph writes the new default', async () => {
    const api = createMockIpc({ seed: false });
    expect((await api.createGraph('/mock/new', 'n')).settings.default_permission_mode).toBe('claude_settings');
  });
});

describe('blocked status', () => {
  it('has the amber colour, its own label, an amber ring and edge', () => {
    expect(NODE_STATUSES).toContain('blocked');
    expect(STATUS_COLORS.blocked).toBe('#f59e0b');
    expect(statusLabel('blocked')).toBe('blocked');
    expect(ringStyle('blocked')).toEqual({ borderColor: '#f59e0b', borderStyle: 'solid', className: '' });
    expect(edgeVisual('blocked').stroke).toBe('#f59e0b');
    expect(composerState('blocked').enabled).toBe(true);
  });

  it('rerun failed includes blocked nodes and their dependents', () => {
    const g = load();
    const prev: RunState = {
      run_id: 'R', graph_dir: '/g', started_at: 't', finished_at: 't', status: 'failed',
      nodes: {
        survey: { status: 'succeeded', started_at: null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null },
        design: { status: 'blocked', started_at: null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: 'q' },
      },
    };
    expect(targetSet(g, { failed_of: 'R' }, () => prev)).toEqual(['design', 'report']);
  });
});

describe('permission denials in the contract', () => {
  it('permission_denied is an event kind; run states with or without permission_denials are accepted', () => {
    expect(NODE_EVENT_KINDS).toContain('permission_denied');
    const base = { run_id: 'r', graph_dir: '/g', started_at: 't', finished_at: null, status: 'running' };
    expect(isRunState({ ...base, nodes: { a: { status: 'blocked', reason: null } } })).toBe(true);
    expect(isRunState({ ...base, nodes: { a: { status: 'failed', reason: null, permission_denials: ['Bash'] } } })).toBe(true);
    expect(isRunState({ ...base, nodes: { a: { status: 'failed', reason: null, permission_denials: [1] } } })).toBe(false);
  });

  it('takes the tool name from the event text', () => {
    expect(deniedToolName('WebSearch: "graph engineering LLM agent…"')).toBe('WebSearch');
    expect(deniedToolName('Bash')).toBe('Bash');
    expect(deniedToolName('')).toBe('a tool');
  });
});

describe('mock runner: FAKE_BLOCKED and FAKE_DENY', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  const setup = () => {
    const api = createMockIpc({ stepMs: 10, trustAll: true });
    const states: RunState[] = [];
    const events: NodeEvent[] = [];
    void api.onRunState((s) => states.push(s));
    void api.onNodeEvent((e) => events.push(e));
    return { api, states, events };
  };

  it('a FAKE_BLOCKED prompt ends blocked, skips dependents and fails the run; a reply promotes it', async () => {
    const { api, states } = setup();
    const g = updateNode(await api.openGraph(MOCK_EXAMPLE_DIR), 'survey', { prompt: `do it ${FAKE_BLOCKED}` });
    await api.saveGraph(MOCK_EXAMPLE_DIR, g);
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await vi.runAllTimersAsync();
    const final = states[states.length - 1] as RunState;
    expect(final.status).toBe('failed');
    expect(final.nodes.survey).toMatchObject({ status: 'blocked', reason: MOCK_BLOCKED_QUESTION });
    expect(final.nodes.design).toMatchObject({ status: 'skipped', reason: 'upstream survey blocked' });
    const result = await api.readNodeResult(MOCK_EXAMPLE_DIR, runId, 'survey');
    expect(result.result_md).toContain('## Status: BLOCKED');
    expect(result.summary?.status).toBe('blocked');

    await api.sendMessage(MOCK_EXAMPLE_DIR, runId, 'survey', 'use dataset A');
    await vi.runAllTimersAsync();
    const after = (await api.getRun(MOCK_EXAMPLE_DIR, runId)).nodes.survey;
    expect(after).toMatchObject({ status: 'succeeded', reason: null });
  });

  it('list_runs counts blocked nodes', async () => {
    const { api } = setup();
    const g = updateNode(await api.openGraph(MOCK_EXAMPLE_DIR), 'survey', { prompt: FAKE_BLOCKED });
    await api.saveGraph(MOCK_EXAMPLE_DIR, g);
    await api.startRun(MOCK_EXAMPLE_DIR, 'all');
    await vi.runAllTimersAsync();
    const [summary] = await api.listRuns(MOCK_EXAMPLE_DIR);
    expect(summary?.counts).toMatchObject({ blocked: 1, skipped: 2, failed: 0 });
  });

  it('FAKE_FAIL is a real failure: no result.md, error summary, run failed', async () => {
    const { api, states } = setup();
    const g = updateNode(await api.openGraph(MOCK_EXAMPLE_DIR), 'report', { extra_context: 'FAKE_FAIL' });
    await api.saveGraph(MOCK_EXAMPLE_DIR, g);
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, { from: 'report' });
    await vi.runAllTimersAsync();
    expect(states[states.length - 1]?.status).toBe('failed');
    expect(states[states.length - 1]?.nodes.report?.status).toBe('failed');
    const result = await api.readNodeResult(MOCK_EXAMPLE_DIR, runId, 'report');
    expect(result.result_md).toBeNull();
    expect(result.summary).toMatchObject({ status: 'failed', is_error: true });
  });

  it('a FAKE_DENY prompt emits a permission_denied event and records the denial', async () => {
    const { api, events } = setup();
    const g = updateNode(await api.openGraph(MOCK_EXAMPLE_DIR), 'survey', { prompt: `search ${FAKE_DENY}` });
    await api.saveGraph(MOCK_EXAMPLE_DIR, g);
    const runId = await api.startRun(MOCK_EXAMPLE_DIR, { from: 'survey' });
    await vi.runAllTimersAsync();
    const denied = events.filter((e) => e.kind === 'permission_denied');
    expect(denied.map((e) => [e.node_id, deniedToolName(e.text)])).toEqual([['survey', MOCK_DENIED_TOOL]]);
    expect((await api.getRun(MOCK_EXAMPLE_DIR, runId)).nodes.survey?.permission_denials).toEqual([MOCK_DENIED_TOOL]);
    expect((await api.readNodeResult(MOCK_EXAMPLE_DIR, runId, 'survey')).summary?.permission_denials).toEqual([MOCK_DENIED_TOOL]);
  });
});

describe('node ids follow the title', () => {
  const twoNodes = () => {
    const a = addNode(emptyGraph('g'), 'New node', { x: 0, y: 0 });
    const b = addNode(a.graph, 'Survey', { x: 0, y: 0 });
    return { graph: b.graph, a: a.id, b: b.id };
  };

  it('auto-follows: a new-node id and a slug(title) id are re-derived on every title change', () => {
    const { graph, a } = twoNodes();
    expect(a).toBe('new-node');
    const r1 = retitleNode(graph, a, 'Data cleaning', true);
    expect(r1.id).toBe('data-cleaning');
    const r2 = retitleNode(r1.graph, r1.id, 'Data cleaning v2', true);
    expect(r2.id).toBe('data-cleaning-v2');
    expect(r2.graph.nodes.find((n) => n.id === 'data-cleaning-v2')?.title).toBe('Data cleaning v2');
    expect(graph.nodes[0]?.id).toBe('new-node'); // input untouched
  });

  it('adds a uniqueness suffix (case-insensitive) and keeps following through it', () => {
    const { graph, a } = twoNodes();
    const r1 = retitleNode(graph, a, 'Survey', true);
    expect(r1.id).toBe('survey-2');
    const r2 = retitleNode(r1.graph, r1.id, 'Survey ', true);
    expect(r2.id).toBe('survey-2');
    const r3 = retitleNode(r2.graph, r2.id, 'Survey B', true);
    expect(r3.id).toBe('survey-b');
  });

  it('stops following after a manual id edit', () => {
    expect(followsTitle('my-own-id', 'Survey')).toBe(false);
    const { graph, b } = twoNodes();
    const renamed = { ...graph, nodes: graph.nodes.map((n) => (n.id === b ? { ...n, id: 'my-own-id' } : n)) };
    const r = retitleNode(renamed, 'my-own-id', 'Something else', true);
    expect(r.id).toBe('my-own-id');
    expect(r.graph.nodes.find((n) => n.id === 'my-own-id')?.title).toBe('Something else');
  });

  it('does not rename once the node has run (mayRename false)', () => {
    const { graph, b } = twoNodes();
    const r = retitleNode(graph, b, 'Review', false);
    expect(r.id).toBe(b);
    expect(r.graph.nodes.find((n) => n.id === b)?.title).toBe('Review');
  });

  it('rewrites edges and continue_session_of', () => {
    const { graph, a, b } = twoNodes();
    const linked = connect(graph, b, a);
    if (!linked.ok) throw new Error(linked.error);
    const g = updateNode(linked.graph, a, { continue_session_of: b });
    const r = retitleNode(g, b, 'Literature review', true);
    expect(r.id).toBe('literature-review');
    expect(r.graph.edges).toEqual([{ id: 'e1', from: 'literature-review', to: a }]);
    expect(r.graph.nodes.find((n) => n.id === a)?.continue_session_of).toBe('literature-review');
  });

  it('idIsEditable: fresh nodes, or nodes absent from every loaded run and the shown run', () => {
    const run = { nodes: { ran: {} } } as unknown as RunState;
    expect(idIsEditable({ freshNodeIds: ['x'], ranNodeIds: null, runState: null }, 'x')).toBe(true);
    expect(idIsEditable({ freshNodeIds: [], ranNodeIds: null, runState: null }, 'x')).toBe(false);
    expect(idIsEditable({ freshNodeIds: [], ranNodeIds: ['old'], runState: null }, 'x')).toBe(true);
    expect(idIsEditable({ freshNodeIds: [], ranNodeIds: ['old'], runState: null }, 'old')).toBe(false);
    expect(idIsEditable({ freshNodeIds: [], ranNodeIds: [], runState: run }, 'ran')).toBe(false);
  });
});

describe('brief preview: slash command line', () => {
  it('adds the line under ## Task for a /command prompt, before the inputs', () => {
    const { graph, id } = addNode(emptyGraph('g'), 'Solo', { x: 0, y: 0 });
    const text = renderBrief(updateNode(graph, id, { prompt: '/model-tiering\nSurvey graph engineering.' }), id, { graphDir: '/w/g' });
    expect(text).toContain(`## Task\n${SLASH_COMMAND_LINE}\n/model-tiering\nSurvey graph engineering.\n\n## Inputs from upstream nodes\n`);
    expect(text.indexOf('Working directory: /w/g\n')).toBeLessThan(text.indexOf('## Task\n'));
  });

  it('no slash, no line', () => {
    const { graph, id } = addNode(emptyGraph('g'), 'Solo', { x: 0, y: 0 });
    const text = renderBrief(updateNode(graph, id, { prompt: 'Fix /etc parsing' }), id);
    expect(text).not.toContain(SLASH_COMMAND_LINE);
    expect(text).toContain('## Task\nFix /etc parsing\n\n## Inputs');
  });
});
