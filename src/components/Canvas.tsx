/** The node editor. RF keeps UI state locally; graph content lives in the store. */
import { useCallback, useEffect, useState, type MouseEvent } from 'react';
import {
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Controls,
  MiniMap,
  ReactFlow,
  useReactFlow,
  type Connection,
  type EdgeChange,
  type NodeChange,
  type OnSelectionChangeParams,
} from '@xyflow/react';
import {
  committedPositions,
  flowPositions,
  graphToFlowEdges,
  selectOnly,
  graphToFlowNodes,
  mergeFlowEdges,
  mergeFlowNodes,
  type BriefFlowNode,
  type WireFlowEdge,
} from '../lib/flow';
import { MODEL_COLORS } from '../lib/status';
import { harnessLabel } from '../lib/harness';
import { graphStore, useGraphStore } from '../store/graphStore';
import type { DenialsByNode } from '../store/runHelpers';
import { waitingNodeIds } from '../store/permissionSlice';
import { BriefNode } from './BriefNode';
import { EmptyCanvas, StatusLegend } from './CanvasOverlays';
import { WireEdge } from './WireEdge';

export const GRID = 16;
const nodeTypes = { brief: BriefNode };
const edgeTypes = { wire: WireEdge };
const snap = (v: number): number => Math.round(v / GRID) * GRID;
const NO_DENIALS: DenialsByNode = {};

export function Canvas() {
  const graph = useGraphStore((s) => s.graph);
  const runState = useGraphStore((s) => s.runState);
  const selectedNodeId = useGraphStore((s) => s.selectedNodeId);
  const liveDenials = useGraphStore((s) => (s.eventsRunId !== null && s.eventsRunId === s.runState?.run_id ? s.liveDenials : NO_DENIALS));
  const harnesses = useGraphStore((s) => s.harnesses);
  const prompts = useGraphStore((s) => s.permissionPrompts);
  const { screenToFlowPosition } = useReactFlow();
  const [nodes, setNodes] = useState<BriefFlowNode[]>(() => (graph ? graphToFlowNodes(graph, runState) : []));
  const [edges, setEdges] = useState<WireFlowEdge[]>(() => (graph ? graphToFlowEdges(graph, runState) : []));

  useEffect(() => {
    if (!graph) return;
    const denied = new Set(Object.keys(liveDenials));
    const labelOf = (id: string): string => harnessLabel(harnesses, id);
    const waiting = waitingNodeIds(prompts, runState);
    setNodes((prev) => mergeFlowNodes(graphToFlowNodes(graph, runState, denied, labelOf, waiting), prev));
    setEdges((prev) => mergeFlowEdges(graphToFlowEdges(graph, runState), prev));
  }, [graph, runState, liveDenials, harnesses, prompts]);

  // M2: RF selection follows the store; a programmatic selection deselects the others.
  useEffect(() => setNodes((ns) => selectOnly(ns, selectedNodeId)), [selectedNodeId, graph]);

  const onNodesChange = useCallback((changes: NodeChange<BriefFlowNode>[]) => {
    setNodes((ns) => applyNodeChanges(changes, ns));
    // M1: keyboard (arrow) moves and drag ends arrive as position changes with dragging === false.
    const moved = committedPositions(changes);
    if (Object.keys(moved).length > 0) graphStore().getState().moveNodes(moved);
  }, []);
  const onEdgesChange = useCallback(
    (changes: EdgeChange<WireFlowEdge>[]) => setEdges((es) => applyEdgeChanges(changes, es)),
    [],
  );
  const onNodeDragStop = useCallback(
    (_e: unknown, _n: BriefFlowNode, dragged: BriefFlowNode[]) => graphStore().getState().moveNodes(flowPositions(dragged)),
    [],
  );
  const onConnect = useCallback((c: Connection) => {
    graphStore().getState().connect(c.source, c.target);
  }, []);
  const onDelete = useCallback(({ nodes: ns, edges: es }: { nodes: BriefFlowNode[]; edges: WireFlowEdge[] }) => {
    graphStore()
      .getState()
      .removeElements(
        ns.map((n) => n.id),
        es.map((e) => e.id),
      );
  }, []);
  const onSelectionChange = useCallback(({ nodes: ns }: OnSelectionChangeParams) => {
    const store = graphStore().getState();
    const next = ns[0]?.id ?? null;
    if (next !== store.selectedNodeId && (next !== null || ns.length === 0)) store.select(next);
  }, []);

  const onDoubleClick = (e: MouseEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    if (!target.closest('.react-flow__pane') || target.closest('.react-flow__node')) return;
    const p = screenToFlowPosition({ x: e.clientX, y: e.clientY });
    graphStore().getState().addNodeAt({ x: snap(p.x), y: snap(p.y) });
  };

  const addCentered = (): void => {
    const p = screenToFlowPosition({ x: window.innerWidth * 0.4, y: window.innerHeight * 0.4 });
    graphStore().getState().addNodeAt({ x: snap(p.x), y: snap(p.y) });
  };

  return (
    <div className="relative h-full w-full" onDoubleClick={onDoubleClick}>
      {(graph?.nodes.length ?? 0) === 0 && <EmptyCanvas onAddNode={addCentered} />}
      <StatusLegend />
      <ReactFlow<BriefFlowNode, WireFlowEdge>
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onNodeDragStop={onNodeDragStop}
        onConnect={onConnect}
        onDelete={onDelete}
        onSelectionChange={onSelectionChange}
        isValidConnection={(c) => c.source !== c.target}
        deleteKeyCode="Delete"
        zoomOnDoubleClick={false}
        snapToGrid
        snapGrid={[GRID, GRID]}
        colorMode="dark"
        fitView
        fitViewOptions={{ padding: 0.2, maxZoom: 1 }}
        proOptions={{ hideAttribution: true }}
      >
        <Background variant={BackgroundVariant.Dots} gap={GRID} size={1} color="#3a3a3a" bgColor="#1e1e1e" />
        <MiniMap
          position="bottom-left"
          pannable
          zoomable
          bgColor="#252526"
          maskColor="rgba(0,0,0,0.5)"
          nodeColor={(n) => MODEL_COLORS[(n as BriefFlowNode).data.node.model]}
        />
        <Controls position="bottom-left" style={{ marginLeft: 220 }} />
      </ReactFlow>
    </div>
  );
}
