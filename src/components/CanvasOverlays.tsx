/**
 * Canvas overlays (UX pass, v0.6): the empty-canvas card ("Add node", "Add a
 * starter 3-node chain") and the status legend popover next to the MiniMap.
 */
import { useState } from 'react';
import { CircleHelp, Plus, Workflow, X } from 'lucide-react';
import { NODE_STATUSES, type NodeStatus } from '../types/contract';
import { STATUS_COLORS, statusLabel } from '../lib/status';
import { graphStore } from '../store/graphStore';
import { btnCls } from './ui';

export const GRID_STEP = 16;
export const STARTER_TITLES = ['Gather', 'Do the work', 'Check'] as const;

/** Three connected nodes, left to right; returns their ids. */
export function addStarterChain(): string[] {
  const s = graphStore().getState();
  const before = new Set(s.graph?.nodes.map((n) => n.id) ?? []);
  STARTER_TITLES.forEach((title, i) => {
    const id = s.addNodeAt({ x: i * 20 * GRID_STEP, y: 0 });
    if (id !== null) graphStore().getState().setTitle(id, title);
  });
  const ids = (graphStore().getState().graph?.nodes ?? []).map((n) => n.id).filter((id) => !before.has(id));
  ids.slice(1).forEach((id, i) => graphStore().getState().connect(ids[i] ?? '', id));
  return ids;
}

export function EmptyCanvas({ onAddNode }: { onAddNode: () => void }) {
  return (
    <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center">
      <div role="region" aria-label="empty graph" className="pointer-events-auto max-w-[360px] space-y-2 rounded border border-[#3c3c3c] bg-[#252526]/95 p-4 text-center text-[12px] shadow-xl">
        <p className="text-[13px] font-semibold text-[#e5e5e5]">This graph is empty</p>
        <p className="text-[#9a9a9a]">A node is one task for the agent; a wire says which task runs first. Double-click the canvas to add a node.</p>
        <div className="flex justify-center gap-2">
          <button className={`${btnCls} border border-[#3b82f6] text-white`} onClick={onAddNode}>
            <Plus size={13} />Add node
          </button>
          <button className={`${btnCls} border border-[#3c3c3c]`} onClick={() => addStarterChain()}>
            <Workflow size={13} />Add a starter 3-node chain
          </button>
        </div>
        <p className="text-[11px] text-[#777]">Run all (Ctrl+Enter) sends each node to its agent in order.</p>
      </div>
    </div>
  );
}

export const STATUS_MEANINGS: Record<NodeStatus, string> = {
  pending: 'not started',
  queued: 'waiting for its turn or for its message to be taken',
  running: 'the agent is working on it',
  waiting: 'it asked you something: answer in Chat',
  succeeded: 'done, deliverables written',
  failed: 'the agent or the CLI failed',
  blocked: 'it stopped and reported BLOCKED',
  skipped: 'not run (an upstream node stopped the chain)',
  cancelled: 'cancelled by you',
};

export function StatusLegend() {
  const [open, setOpen] = useState(false);
  return (
    <div className="absolute bottom-2 left-[260px] z-10">
      <button
        className={`${btnCls} border border-[#3c3c3c] bg-[#252526]`}
        aria-label="status legend"
        aria-expanded={open}
        title="What the colours mean"
        onClick={() => setOpen(!open)}
      >
        <CircleHelp size={13} />
      </button>
      {open && (
        <div role="dialog" aria-label="Status legend" className="absolute bottom-8 left-0 w-[300px] rounded border border-[#3c3c3c] bg-[#252526] p-2 text-[11px] shadow-xl">
          <div className="mb-1 flex items-center">
            <span className="flex-1 font-semibold">Node status</span>
            <button className={btnCls} aria-label="close legend" onClick={() => setOpen(false)}>
              <X size={12} />
            </button>
          </div>
          <ul className="space-y-0.5">
            {NODE_STATUSES.map((s) => (
              <li key={s} className="flex items-center gap-2">
                <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ background: STATUS_COLORS[s] }} />
                <span className="w-[72px] shrink-0 text-[#d4d4d4]">{statusLabel(s)}</span>
                <span className="text-[#9a9a9a]">{STATUS_MEANINGS[s]}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
