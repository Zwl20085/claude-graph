/**
 * Graph <-> React Flow mapping. Pure.
 *
 * Graph -> RF: one `brief` node per graph node (data carries the Node and its
 * status in the shown run), one `wire` edge per graph edge (data carries the
 * source status). RF -> Graph: positions are read back from RF nodes; node
 * content always comes from the graph (RF never edits node content).
 */
import type { Edge as FlowEdge, Node as FlowNode } from '@xyflow/react';
import type { Graph, Node, NodeStatus, Position, RunState } from '../types/contract';
import { applyPositions } from './graphOps';

export interface BriefNodeData extends Record<string, unknown> {
  node: Node;
  status: NodeStatus | null;
  reason: string | null;
}
export type BriefFlowNode = FlowNode<BriefNodeData, 'brief'>;

export interface WireEdgeData extends Record<string, unknown> {
  sourceStatus: NodeStatus | null;
}
export type WireFlowEdge = FlowEdge<WireEdgeData, 'wire'>;

function statusOf(run: RunState | null, id: string): NodeStatus | null {
  return run?.nodes[id]?.status ?? null;
}

export function graphToFlowNodes(graph: Graph, run: RunState | null): BriefFlowNode[] {
  return graph.nodes.map((node) => ({
    id: node.id,
    type: 'brief',
    position: { x: node.position.x, y: node.position.y },
    data: { node, status: statusOf(run, node.id), reason: run?.nodes[node.id]?.reason ?? null },
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
