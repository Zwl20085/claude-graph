import { describe, expect, it } from 'vitest';
import example from '../../examples/hello-pipeline/graph.json';
import type { Graph, RunState } from '../types/contract';
import { flowPositions, flowToGraph, graphToFlowEdges, graphToFlowNodes, mergeFlowEdges, mergeFlowNodes } from './flow';
import { validateGraph } from './validate';

const load = (): Graph => {
  const g = validateGraph(JSON.parse(JSON.stringify(example)));
  if (g instanceof Error) throw g;
  return g;
};

const run: RunState = {
  run_id: 'r1',
  graph_dir: '/g',
  started_at: '2026-11-07T00:00:00Z',
  finished_at: null,
  status: 'running',
  nodes: {
    survey: { status: 'running', started_at: null, finished_at: null, session_id: null, cost_usd: null, num_turns: null, reason: null },
  },
};

describe('graph -> React Flow', () => {
  it('maps every node with its position, data and type', () => {
    const g = load();
    const nodes = graphToFlowNodes(g, null);
    expect(nodes.map((n) => n.id)).toEqual(['survey', 'design', 'report']);
    expect(nodes[0]).toMatchObject({ type: 'brief', position: { x: 80, y: 160 }, data: { status: null } });
    expect(nodes[0]?.data.node).toBe(g.nodes[0]);
  });

  it('copies positions so RF cannot mutate the graph', () => {
    const g = load();
    const nodes = graphToFlowNodes(g, null);
    expect(nodes[0]?.position).not.toBe(g.nodes[0]?.position);
  });

  it('carries run status onto nodes and source status onto edges', () => {
    const g = load();
    expect(graphToFlowNodes(g, run).map((n) => n.data.status)).toEqual(['running', null, null]);
    const edges = graphToFlowEdges(g, run);
    expect(edges[0]).toMatchObject({ id: 'e1', type: 'wire', source: 'survey', target: 'design', data: { sourceStatus: 'running' } });
    expect(edges[1]?.data?.sourceStatus).toBeNull();
  });
});

describe('React Flow -> graph', () => {
  it('round-trips unchanged', () => {
    const g = load();
    expect(flowToGraph(g, graphToFlowNodes(g, null), graphToFlowEdges(g, null))).toEqual(g);
  });

  it('takes moved positions and keeps everything else', () => {
    const g = load();
    const nodes = graphToFlowNodes(g, null).map((n) => (n.id === 'design' ? { ...n, position: { x: 512, y: 48 } } : n));
    const back = flowToGraph(g, nodes, graphToFlowEdges(g, null));
    expect(back.nodes[1]).toEqual({ ...g.nodes[1], position: { x: 512, y: 48 } });
    expect(back.nodes[0]).toBe(g.nodes[0]);
    expect(g.nodes[1]?.position).toEqual({ x: 420, y: 160 });
  });

  it('drops edges that reference removed or unknown nodes', () => {
    const g = load();
    const nodes = graphToFlowNodes(g, null).filter((n) => n.id !== 'report');
    const edges = [...graphToFlowEdges(g, null), { id: 'x', source: 'ghost', target: 'survey' }];
    const back = flowToGraph(g, nodes, edges);
    expect(back.nodes.map((n) => n.id)).toEqual(['survey', 'design']);
    expect(back.edges).toEqual([{ id: 'e1', from: 'survey', to: 'design' }]);
  });

  it('flowPositions keys positions by id', () => {
    expect(flowPositions([{ id: 'a', position: { x: 1, y: 2 } }])).toEqual({ a: { x: 1, y: 2 } });
  });
});

describe('merge with previous RF state', () => {
  it('keeps selection, measured size and the live position of a dragged node', () => {
    const g = load();
    const prev = graphToFlowNodes(g, null).map((n) =>
      n.id === 'survey'
        ? { ...n, selected: true, dragging: true, position: { x: 999, y: 1 }, measured: { width: 260, height: 120 } }
        : n,
    );
    const merged = mergeFlowNodes(graphToFlowNodes(g, run), prev);
    expect(merged[0]).toMatchObject({ selected: true, position: { x: 999, y: 1 }, data: { status: 'running' } });
    expect(merged[1]?.position).toEqual({ x: 420, y: 160 });
  });

  it('uses the graph position when not dragging, and accepts new nodes', () => {
    const g = load();
    const prev = graphToFlowNodes(g, null).slice(0, 1).map((n) => ({ ...n, position: { x: 0, y: 0 } }));
    const merged = mergeFlowNodes(graphToFlowNodes(g, null), prev);
    expect(merged[0]?.position).toEqual({ x: 80, y: 160 });
    expect(merged).toHaveLength(3);
  });

  it('keeps edge selection', () => {
    const g = load();
    const prev = graphToFlowEdges(g, null).map((e) => (e.id === 'e2' ? { ...e, selected: true } : e));
    expect(mergeFlowEdges(graphToFlowEdges(g, null), prev).map((e) => e.selected ?? false)).toEqual([false, true]);
  });
});
