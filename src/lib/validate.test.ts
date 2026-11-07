import { describe, expect, expectTypeOf, it } from 'vitest';
import example from '../../examples/hello-pipeline/graph.json';
import type { Graph, RunTarget } from '../types/contract';
import { findCycle } from './graph';
import { validateGraph } from './validate';

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

describe('contract types', () => {
  it('compile against the expected shapes', () => {
    expectTypeOf<RunTarget>().toEqualTypeOf<'all' | { from: string } | { failed_of: string }>();
    expectTypeOf<Graph['nodes'][number]['allowed_tools']>().toEqualTypeOf<string[] | null>();
    expectTypeOf(validateGraph).returns.toEqualTypeOf<Graph | Error>();
  });
});

describe('validateGraph', () => {
  it('accepts the example graph unchanged (plus the defaulted prompt field)', () => {
    const g = validateGraph(example);
    expect(g).not.toBeInstanceOf(Error);
    expect(g).toEqual({ ...example, nodes: example.nodes.map((n) => ({ prompt: '', ...n })) });
  });

  it('fills defaults for a minimal node like serde(default)', () => {
    const g = validateGraph({ version: 1, name: 'old', nodes: [{ id: 'a' }] });
    if (g instanceof Error) throw g;
    expect(g.nodes[0]).toMatchObject({ model: 'inherit', permission_mode: 'inherit', allowed_tools: null, deliverables: [] });
    expect(g.settings.max_parallel).toBe(2);
    expect(g.edges).toEqual([]);
  });

  it.each([
    ['wrong version', (g: Graph) => ({ ...g, version: 2 }), /version/],
    ['duplicate id', (g: Graph) => ({ ...g, nodes: [...g.nodes, g.nodes[0]] }), /duplicate node id "survey"/],
    ['dangling edge', (g: Graph) => ({ ...g, edges: [...g.edges, { id: 'e9', from: 'report', to: 'ghost' }] }), /unknown node "ghost"/],
    ['cycle', (g: Graph) => ({ ...g, edges: [...g.edges, { id: 'e9', from: 'report', to: 'survey' }] }), /cycle/],
    ['bad model', (g: Graph) => ({ ...g, nodes: [{ ...g.nodes[0], model: 'gpt' }] }), /model must be one of/],
  ])('rejects %s', (_name, mutate, pattern) => {
    const result = validateGraph(mutate(clone(example) as unknown as Graph));
    expect(result).toBeInstanceOf(Error);
    expect((result as Error).message).toMatch(pattern);
  });

  it('rejects non-objects', () => {
    expect(validateGraph(null)).toBeInstanceOf(Error);
    expect(validateGraph([])).toBeInstanceOf(Error);
  });
});

describe('findCycle', () => {
  const nodes = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];

  it('returns null for a DAG', () => {
    expect(findCycle(nodes, [{ from: 'a', to: 'b' }, { from: 'a', to: 'c' }, { from: 'b', to: 'c' }])).toBeNull();
  });

  it('returns the nodes on a cycle', () => {
    expect(findCycle(nodes, [{ from: 'a', to: 'b' }, { from: 'b', to: 'c' }, { from: 'c', to: 'b' }])).toEqual(['b', 'c']);
  });

  it('detects a self-loop', () => {
    expect(findCycle(nodes, [{ from: 'a', to: 'a' }])).toEqual(['a']);
  });
});
