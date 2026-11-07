// Thin placeholder shell: the frontend engineer replaces this file.
import { useEffect, useMemo, useState } from 'react';
import {
  Background,
  BackgroundVariant,
  Controls,
  MiniMap,
  ReactFlow,
  type Edge as FlowEdge,
  type Node as FlowNode,
} from '@xyflow/react';
import example from '../examples/hello-pipeline/graph.json';
import { checkClaude } from './lib/ipc';
import { validateGraph } from './lib/validate';
import type { AppError, ClaudeCheck, Graph } from './types/contract';

type CheckState = { kind: 'loading' } | { kind: 'ok'; check: ClaudeCheck } | { kind: 'error'; message: string };

function toFlow(graph: Graph): { nodes: FlowNode[]; edges: FlowEdge[] } {
  return {
    nodes: graph.nodes.map((n) => ({ id: n.id, position: n.position, data: { label: n.title || n.id } })),
    edges: graph.edges.map((e) => ({ id: e.id, source: e.from, target: e.to })),
  };
}

function ClaudeBanner({ state }: { state: CheckState }) {
  if (state.kind === 'loading') return <span className="text-neutral-400">checking claude…</span>;
  if (state.kind === 'error') return <span className="text-amber-400">claude check failed: {state.message}</span>;
  const { found, path, version } = state.check;
  return found ? (
    <span className="text-emerald-400">
      claude {version ?? '(version unknown)'} · {path}
    </span>
  ) : (
    <span className="text-red-400">claude CLI not found{path ? ` at ${path}` : ' on PATH'}</span>
  );
}

export default function App() {
  const graph = useMemo(() => validateGraph(example), []);
  const flow = useMemo(() => (graph instanceof Error ? { nodes: [], edges: [] } : toFlow(graph)), [graph]);
  const [check, setCheck] = useState<CheckState>({ kind: 'loading' });

  useEffect(() => {
    checkClaude()
      .then((c) => setCheck({ kind: 'ok', check: c }))
      .catch((e: AppError) => setCheck({ kind: 'error', message: e.message }));
  }, []);

  return (
    <div className="flex h-screen flex-col">
      <header className="flex items-center gap-4 border-b border-neutral-700 px-4 py-2 text-sm">
        <span className="font-semibold">Claude Graph</span>
        <span className="text-neutral-400">{graph instanceof Error ? graph.message : graph.name}</span>
        <span className="ml-auto">
          <ClaudeBanner state={check} />
        </span>
      </header>
      <main className="flex-1">
        <ReactFlow nodes={flow.nodes} edges={flow.edges} colorMode="dark" fitView>
          <Background variant={BackgroundVariant.Dots} gap={16} size={1} color="#3a3a3a" bgColor="#1e1e1e" />
          <MiniMap position="bottom-left" pannable zoomable />
          <Controls position="bottom-left" style={{ marginLeft: 220 }} />
        </ReactFlow>
      </main>
    </div>
  );
}
