/**
 * Right panel for the selected node: Prompt · Brief · Chat · Results. The
 * active tab is kept across node changes (store `inspectorTab`); the header
 * shows the node's status and last-run cost; the id sits under "Advanced".
 */
import { useEffect, useState } from 'react';
import { Eye } from 'lucide-react';
import { sessionsOf, type Node } from '../types/contract';
import { formatCost, STATUS_COLORS, statusLabel } from '../lib/status';
import { isSessionAlive, sessionOfNode } from '../lib/sessions';
import { useGraphStore } from '../store/graphStore';
import type { InspectorTab } from '../store/uiSlice';
import { idIsEditable } from '../store/runHelpers';
import { BriefForm } from './BriefForm';
import { MessagesTab } from './MessagesTab';
import { PreviewModal } from './PreviewModal';
import { PromptTab } from './PromptTab';
import { ResultsTab } from './ResultsTab';
import { btnCls, inputCls } from './ui';

const TABS: ReadonlyArray<{ id: InspectorTab; label: string }> = [
  { id: 'prompt', label: 'Prompt' },
  { id: 'brief', label: 'Brief' },
  { id: 'chat', label: 'Chat' },
  { id: 'results', label: 'Results' },
];

export const SESSION_ENDED_TEXT = 'The session window of this node closed before it finished.';

/** Shown when the node's session in the shown run ended with the node not done. */
function SessionClosedBanner({ nodeId }: { nodeId: string }) {
  const runState = useGraphStore((s) => s.runState);
  const startRun = useGraphStore((s) => s.startRun);
  const session = sessionOfNode(sessionsOf(runState), nodeId);
  const status = runState?.nodes[nodeId]?.status;
  const stopped = status === 'failed' || status === 'cancelled' || status === 'skipped';
  if (!runState || !session || isSessionAlive(session) || !stopped || session.status === 'starting') return null;
  return (
    <div role="status" aria-label="session closed" className="space-y-1 rounded border border-[#b45309] px-2 py-1.5 text-[12px] text-[#f59e0b]">
      <p>{SESSION_ENDED_TEXT}</p>
      <button className={`${btnCls} border border-[#b45309]`} onClick={() => void startRun({ from: nodeId })}>
        Run from here
      </button>
    </div>
  );
}

/** Status chip and last-run cost of the node in the shown run. */
function NodeHeaderStatus({ nodeId }: { nodeId: string }) {
  const run = useGraphStore((s) => s.runState?.nodes[nodeId] ?? null);
  if (run === null) return null;
  return (
    <div className="flex items-center gap-2 text-[11px]">
      <span className="rounded px-1.5 font-semibold text-black" style={{ background: STATUS_COLORS[run.status] }} aria-label={`status ${statusLabel(run.status)}`}>
        {statusLabel(run.status)}
      </span>
      <span className="text-[#9a9a9a]" title="Cost of this node in the shown run">
        last run {formatCost(run.cost_usd)}
      </span>
    </div>
  );
}

export const ID_HINT = 'Follows the title until you edit it, or until the node has run.';
export const BLOCKED_TEXT =
  'This node stopped and asked you something. Read its reply and answer in the Chat tab; a DONE reply will mark it succeeded.';

export const WAITING_TEXT = 'This node asked you something. Answer it in the Chat tab (or in its session window).';

/** Shown while the selected node waits for the user in the shown run. */
function WaitingBanner({ onOpenMessages }: { onOpenMessages: () => void }) {
  const color = STATUS_COLORS.waiting;
  return (
    <div role="status" aria-label="node needs you" className="space-y-1 rounded border px-2 py-1.5 text-[12px]" style={{ borderColor: color, color }}>
      <p>{WAITING_TEXT}</p>
      <button className={`${btnCls} border`} style={{ borderColor: color }} onClick={onOpenMessages} aria-label="open Chat to answer">
        Answer in Chat
      </button>
    </div>
  );
}

/** Shown while the selected node is blocked in the shown run. */
function BlockedBanner({ onOpenMessages }: { onOpenMessages: () => void }) {
  const color = STATUS_COLORS.blocked;
  return (
    <div role="status" aria-label="node blocked" className="space-y-1 rounded border px-2 py-1.5 text-[12px]" style={{ borderColor: color, color }}>
      <p>{BLOCKED_TEXT}</p>
      <button className={`${btnCls} border`} style={{ borderColor: color }} onClick={onOpenMessages} aria-label="open Chat">
        Open Chat
      </button>
    </div>
  );
}

/** Id editor: the typed text and its error stay visible until fixed (Escape reverts). */
function IdField({ node }: { node: Node }) {
  const fresh = useGraphStore((s) => idIsEditable(s, node.id));
  const rename = useGraphStore((s) => s.renameNodeId);
  const [draft, setDraft] = useState(node.id);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setDraft(node.id);
    setError(null);
  }, [node.id]);
  const commit = (): void => {
    const next = draft.trim();
    if (next === node.id) return setError(null);
    setError(rename(node.id, next));
  };
  return (
    <div>
      <input
        className={`${inputCls} font-mono text-[12px] ${error ? 'border-[#ef4444]' : ''}`}
        value={draft}
        readOnly={!fresh}
        aria-label="Node id"
        aria-invalid={error !== null}
        title={fresh ? 'Editable until the node first runs' : 'Node ids are fixed once created and run'}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => fresh && commit()}
        onKeyDown={(e) => {
          if (!fresh) return;
          if (e.key === 'Enter') commit();
          if (e.key === 'Escape') {
            setDraft(node.id);
            setError(null);
          }
        }}
      />
      {fresh && <p className="mt-0.5 text-[11px] text-[#8a8a8a]">{ID_HINT}</p>}
      {error && <p role="alert" className="mt-0.5 text-[11px] text-[#ef4444]">{error}</p>}
    </div>
  );
}

export function Inspector() {
  const node = useGraphStore((s) => s.graph?.nodes.find((n) => n.id === s.selectedNodeId) ?? null);
  const setTitle = useGraphStore((s) => s.setTitle);
  const blocked = useGraphStore((s) => node !== null && s.runState?.nodes[node.id]?.status === 'blocked');
  const waiting = useGraphStore((s) => node !== null && s.runState?.nodes[node.id]?.status === 'waiting');
  const tab = useGraphStore((s) => s.inspectorTab);
  const setTab = useGraphStore((s) => s.setInspectorTab);
  const [previewOpen, setPreviewOpen] = useState(false);

  if (!node) {
    return <div className="p-4 text-[12px] text-[#777]">Select a node, or double-click the canvas to add one.</div>;
  }
  const withPreview = tab === 'prompt' || tab === 'brief';
  return (
    <div className="flex h-full flex-col">
      <div className="space-y-1.5 border-b border-[#3c3c3c] p-2">
        <input
          className={`${inputCls} text-[14px] font-semibold`}
          value={node.title}
          placeholder="Title"
          aria-label="Node title"
          onChange={(e) => setTitle(node.id, e.target.value)}
        />
        <NodeHeaderStatus nodeId={node.id} />
        <details className="text-[11px] text-[#9a9a9a]">
          <summary className="cursor-pointer select-none">Advanced</summary>
          <div className="mt-1">
            <IdField key={node.id} node={node} />
          </div>
        </details>
        {blocked && <BlockedBanner onOpenMessages={() => setTab('chat')} />}
        {waiting && tab !== 'chat' && <WaitingBanner onOpenMessages={() => setTab('chat')} />}
        <SessionClosedBanner nodeId={node.id} />
      </div>
      <div className="flex border-b border-[#3c3c3c] text-[12px]" role="tablist" aria-label="Node editor">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            id={`tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls="inspector-tabpanel"
            className={`px-2.5 py-1.5 ${tab === t.id ? 'border-b-2 border-[#3b82f6] text-white' : 'text-[#9a9a9a] hover:text-[#d4d4d4]'}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-2" role="tabpanel" id="inspector-tabpanel" aria-labelledby={`tab-${tab}`}>
        {tab === 'prompt' && <PromptTab node={node} />}
        {tab === 'brief' && <BriefForm key={node.id} node={node} />}
        {tab === 'chat' && <MessagesTab key={node.id} nodeId={node.id} />}
        {tab === 'results' && <ResultsTab key={node.id} nodeId={node.id} />}
        {withPreview && (
          <button className={`${btnCls} mt-3 border border-[#3c3c3c]`} onClick={() => setPreviewOpen(true)}>
            <Eye size={13} />Preview brief
          </button>
        )}
      </div>
      {previewOpen && <PreviewModal nodeId={node.id} onClose={() => setPreviewOpen(false)} />}
    </div>
  );
}
