/**
 * Graph <-> React Flow mapping. Pure.
 *
 * Graph -> RF: one `brief` node per graph node (data carries the Node and its
 * status in the shown run), one `wire` edge per graph edge (data carries the
 * source status). RF -> Graph: positions are read back from RF nodes; node
 * content always comes from the graph (RF never edits node content).
 */
import type { Edge as FlowEdge, Node as FlowNode } from '@xyflow/react';
import { deniedTools, harnessOf, type Graph, type Node, type NodeStatus, type Position, type RunState } from '../types/contract';
import { applyPositions } from './graphOps';

export interface BriefNodeData extends Record<string, unknown> {
  node: Node;
  status: NodeStatus | null;
  reason: string | null;
  /** Some tool call of this node was denied in the shown run. */
  denied: boolean;
  /** Label of the node's own harness when it overrides the graph default; else null. */
  harnessLabel: string | null;
}
export type BriefFlowNode = FlowNode<BriefNodeData, 'brief'>;

export interface WireEdgeData extends Record<string, unknown> {
  sourceStatus: NodeStatus | null;
}
export type WireFlowEdge = FlowEdge<WireEdgeData, 'wire'>;

function statusOf(run: RunState | null, id: string): NodeStatus | null {
  return run?.nodes[id]?.status ?? null;
}

/**
 * `liveDenied`: ids with a permission_denied event seen live in `run` (the
 * persisted `permission_denials` only fill in when a turn ends).
 */
export function graphToFlowNodes(
  graph: Graph,
  run: RunState | null,
  liveDenied: ReadonlySet<string> = new Set(),
  labelOf: (harness: string) => string = (h) => h,
): BriefFlowNode[] {
  const graphHarness = harnessOf(graph.settings);
  return graph.nodes.map((node) => ({
    id: node.id,
    type: 'brief',
    position: { x: node.position.x, y: node.position.y },
    data: {
      node,
      status: statusOf(run, node.id),
      reason: run?.nodes[node.id]?.reason ?? null,
      denied: run !== null && (liveDenied.has(node.id) || deniedTools(run.nodes[node.id]).length > 0),
      harnessLabel: node.harness !== null && node.harness !== graphHarness ? labelOf(node.harness) : null,
    },
  }));
}

export function graphToFlowEdges(graph: Graph, run: RunState | null): WireFlowEdge[] {
  return graph.edges.map((edge) => ({
    id: edge.id,
    type: 'wire',
    source: edge.from,
    target: edge.to,
    data: { sourceStatus: statusOf(run, edge.from) },
  }));
}

/**
 * Carry UI-only state from the previous RF nodes into freshly mapped ones:
 * selection, measured size, and the live position of a node being dragged
 * (so a run:state update during a drag does not snap it back).
 */
export function mergeFlowNodes(next: BriefFlowNode[], prev: readonly BriefFlowNode[]): BriefFlowNode[] {
  const byId = new Map(prev.map((n) => [n.id, n]));
  return next.map((n) => {
    const old = byId.get(n.id);
    if (!old) return n;
    return {
      ...n,
      selected: old.selected,
      measured: old.measured,
      dragging: old.dragging,
      position: old.dragging ? old.position : n.position,
    };
  });
}

export function mergeFlowEdges(next: WireFlowEdge[], prev: readonly WireFlowEdge[]): WireFlowEdge[] {
  const selected = new Set(prev.filter((e) => e.selected).map((e) => e.id));
  return next.map((e) => (selected.has(e.id) ? { ...e, selected: true } : e));
}

/** Positions of RF nodes keyed by id. */
export function flowPositions(nodes: readonly Pick<FlowNode, 'id' | 'position'>[]): Record<string, Position> {
  return Object.fromEntries(nodes.map((n) => [n.id, { x: n.position.x, y: n.position.y }]));
}

/**
 * Rebuild a Graph from RF state: node content and settings from `base`,
 * positions from `nodes`, edges from `edges` (ids kept). Nodes or edges that
 * RF has but `base` lacks are ignored; RF never invents graph content.
 */
export function flowToGraph(
  base: Graph,
  nodes: readonly Pick<FlowNode, 'id' | 'position'>[],
  edges: readonly Pick<FlowEdge, 'id' | 'source' | 'target'>[],
): Graph {
  const known = new Set(base.nodes.map((n) => n.id));
  const keptNodes = new Set(nodes.map((n) => n.id));
  const positioned = applyPositions(base, flowPositions(nodes));
  return {
    ...positioned,
    nodes: positioned.nodes.filter((n) => keptNodes.has(n.id)),
    edges: edges
      .filter((e) => known.has(e.source) && known.has(e.target) && keptNodes.has(e.source) && keptNodes.has(e.target))
      .map((e) => ({ id: e.id, from: e.source, to: e.target })),
  };
}

/**
 * Store selection -> RF selection. When `id` is already among the selected RF
 * nodes (a user multi-select) nothing changes; otherwise exactly `id` (or
 * nothing, for null) is selected. Returns `nodes` itself when unchanged.
 */
export function selectOnly<T extends Pick<FlowNode, 'id' | 'selected'>>(nodes: T[], id: string | null): T[] {
  const selected = nodes.filter((n) => n.selected).map((n) => n.id);
  if (id !== null && selected.includes(id)) return nodes;
  if (id === null && selected.length === 0) return nodes;
  return nodes.map((n) => (Boolean(n.selected) === (n.id === id) ? n : { ...n, selected: n.id === id }));
}

/** Final positions from RF node changes: `position` changes that are not mid-drag (drag end, arrow keys). */
export function committedPositions(changes: readonly { type: string; id?: string; position?: Position; dragging?: boolean }[]): Record<string, Position> {
  const out: Record<string, Position> = {};
  for (const c of changes) {
    if (c.type === 'position' && c.dragging === false && c.position && c.id !== undefined) {
      out[c.id] = { x: c.position.x, y: c.position.y };
    }
  }
  return out;
}
