import { describe, expect, it } from 'vitest';
import example from '../../examples/hello-pipeline/graph.json';
import type { Graph } from '../types/contract';
import * as ops from './graphOps';
import { validateGraph } from './validate';

const last = <T>(a: readonly T[]): T | undefined => a[a.length - 1];

const load = (): Graph => {
  const g = validateGraph(JSON.parse(JSON.stringify(example)));
  if (g instanceof Error) throw g;
  return g;
};
const frozen = (): Graph => {
  const g = load();
  return JSON.parse(JSON.stringify(g), (_k, v) => (typeof v === 'object' && v !== null ? Object.freeze(v) : v)) as Graph;
};

describe('connect (cycle refusal)', () => {
  it('adds a new edge with the next id', () => {
    const r = ops.connect(frozen(), 'survey', 'report');
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.edgeId).toBe('e3');
      expect(last(r.graph.edges)).toEqual({ id: 'e3', from: 'survey', to: 'report' });
    }
  });

  it.each([
    ['cycle', 'report', 'survey', /cycle/],
    ['short cycle', 'design', 'survey', /cycle/],
    ['duplicate', 'survey', 'design', /already exists/],
    ['self loop', 'design', 'design', /itself/],
    ['unknown node', 'survey', 'ghost', /existing nodes/],
  ])('refuses %s', (_n, from, to, pattern) => {
    const g = frozen();
    const r = ops.connect(g, from, to);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toMatch(pattern);
    expect(g.edges).toHaveLength(2);
  });

  it('nextEdgeId skips taken and non-numeric ids', () => {
    expect(ops.nextEdgeId([{ id: 'e2', from: 'a', to: 'b' }, { id: 'x', from: 'a', to: 'b' }])).toBe('e3');
    expect(ops.nextEdgeId([])).toBe('e1');
  });
});

describe('node edits', () => {
  it('addNode generates a unique slug id with defaults', () => {
    const { graph, id } = ops.addNode(frozen(), 'Survey', { x: 16, y: 32 });
    expect(id).toBe('survey-2');
    expect(last(graph.nodes)).toMatchObject({ id, title: 'Survey', model: 'inherit', allowed_tools: null, position: { x: 16, y: 32 } });
  });

  it('updateNode patches without changing the id; unknown id is a no-op', () => {
    const g = frozen();
    const next = ops.updateNode(g, 'design', { goal: 'new', id: 'evil' } as ops.NodePatch);
    expect(next.nodes[1]).toMatchObject({ id: 'design', goal: 'new' });
    expect(ops.updateNode(g, 'nope', { goal: 'x' })).toBe(g);
  });

  it('applyPositions returns the same graph when nothing moved', () => {
    const g = frozen();
    expect(ops.applyPositions(g, { survey: { x: 80, y: 160 }, ghost: { x: 1, y: 1 } })).toBe(g);
    expect(ops.applyPositions(g, { survey: { x: 0, y: 0 } }).nodes[0]?.position).toEqual({ x: 0, y: 0 });
  });

  it('updateSettings merges', () => {
    expect(ops.updateSettings(frozen(), { max_parallel: 5 }).settings).toMatchObject({ max_parallel: 5, default_model: 'sonnet' });
  });

  it('upstreamOf lists direct parents', () => {
    expect(ops.upstreamOf(load(), 'design')).toEqual(['survey']);
  });
});

describe('removeElements', () => {
  it('drops a node and every edge touching it', () => {
    const g = ops.removeElements(frozen(), ['design'], []);
    expect(g.nodes.map((n) => n.id)).toEqual(['survey', 'report']);
    expect(g.edges).toEqual([]);
  });

  it('clears continue_session_of that no longer names a direct upstream', () => {
    const base = ops.updateNode(load(), 'design', { continue_session_of: 'survey' });
    const g = ops.removeElements(base, [], ['e1']);
    expect(g.nodes[1]?.continue_session_of).toBeNull();
  });

  it('is a no-op for unknown ids', () => {
    const g = frozen();
    expect(ops.removeElements(g, ['x'], ['y'])).toBe(g);
  });
});

describe('renameNode', () => {
  it('renames references in edges and continue_session_of', () => {
    const base = ops.updateNode(load(), 'design', { continue_session_of: 'survey' });
    const r = ops.renameNode(base, 'survey', 'lit-survey');
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.graph.nodes[0]?.id).toBe('lit-survey');
    expect(r.graph.nodes[1]?.continue_session_of).toBe('lit-survey');
    expect(r.graph.edges[0]).toEqual({ id: 'e1', from: 'lit-survey', to: 'design' });
  });

  it.each([
    ['taken', 'design', /already used/],
    ['bad id', 'Bad Id', /letters, digits/],
  ])('refuses %s', (_n, newId, pattern) => {
    const r = ops.renameNode(load(), 'survey', newId);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toMatch(pattern);
  });

  it('same id is ok; unknown node is an error', () => {
    expect(ops.renameNode(load(), 'survey', 'survey').ok).toBe(true);
    expect(ops.renameNode(load(), 'nope', 'x').ok).toBe(false);
  });

  it('emptyGraph validates', () => {
    expect(validateGraph(ops.emptyGraph('x'))).toEqual(ops.emptyGraph('x'));
  });
});

describe('small helpers', () => {
  it('parseToolList splits commas and maps empty to null', () => {
    expect(ops.parseToolList(' Read, Bash(python:*) ,, ')).toEqual(['Read', 'Bash(python:*)']);
    expect(ops.parseToolList('  ')).toBeNull();
  });

  it('isEmptyNode / emptyNodeIds', () => {
    const g = ops.addNode(load(), 'x', { x: 0, y: 0 }).graph;
    expect(ops.emptyNodeIds(g, g.nodes.map((n) => n.id))).toEqual(['x']);
    expect(ops.isEmptyNode({ prompt: ' hi ', goal: '' })).toBe(false);
  });
});
