/** Right panel for the selected node: Prompt | Brief | Messages | Results. */
import { useEffect, useState } from 'react';
import { Eye } from 'lucide-react';
import type { Node } from '../types/contract';
import { STATUS_COLORS } from '../lib/status';
import { useGraphStore } from '../store/graphStore';
import { idIsEditable } from '../store/runHelpers';
import { BriefForm } from './BriefForm';
import { MessagesTab } from './MessagesTab';
import { PreviewModal } from './PreviewModal';
import { PromptTab } from './PromptTab';
import { ResultsTab } from './ResultsTab';
import { btnCls, inputCls } from './ui';

const TABS = ['Prompt', 'Brief (recommended)', 'Messages', 'Results'] as const;
type Tab = (typeof TABS)[number];

export const ID_HINT = 'Follows the title until you edit it, or until the node has run.';
export const BLOCKED_TEXT =
  'This node stopped and asked you something. Read its reply and answer in the Messages tab; a DONE reply will mark it succeeded.';

/** Shown while the selected node is blocked in the shown run. */
function BlockedBanner({ onOpenMessages }: { onOpenMessages: () => void }) {
  const color = STATUS_COLORS.blocked;
  return (
    <div role="status" aria-label="node blocked" className="space-y-1 rounded border px-2 py-1.5 text-[12px]" style={{ borderColor: color, color }}>
      <p>{BLOCKED_TEXT}</p>
      <button className={`${btnCls} border`} style={{ borderColor: color }} onClick={onOpenMessages} aria-label="open Messages">
        Open Messages
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
  const [tab, setTab] = useState<Tab>('Prompt');
  const [previewOpen, setPreviewOpen] = useState(false);

  if (!node) {
    return <div className="p-4 text-[12px] text-[#777]">Select a node, or double-click the canvas to add one.</div>;
  }
  const withPreview = tab === 'Prompt' || tab === 'Brief (recommended)';
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
        <IdField key={node.id} node={node} />
        {blocked && <BlockedBanner onOpenMessages={() => setTab('Messages')} />}
      </div>
      <div className="flex border-b border-[#3c3c3c] text-[12px]" role="tablist" aria-label="Node editor">
        {TABS.map((t) => (
          <button
            key={t}
            role="tab"
            id={`tab-${t.split(" ")[0]}`}
            aria-selected={tab === t}
            aria-controls="inspector-tabpanel"
            className={`px-2.5 py-1.5 ${tab === t ? 'border-b-2 border-[#3b82f6] text-white' : 'text-[#9a9a9a] hover:text-[#d4d4d4]'}`}
            onClick={() => setTab(t)}
          >
            {t}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-2" role="tabpanel" id="inspector-tabpanel" aria-labelledby={`tab-${tab.split(" ")[0]}`}>
        {tab === 'Prompt' && <PromptTab node={node} />}
        {tab === 'Brief (recommended)' && <BriefForm key={node.id} node={node} />}
        {tab === 'Messages' && <MessagesTab key={node.id} nodeId={node.id} />}
        {tab === 'Results' && <ResultsTab key={node.id} nodeId={node.id} />}
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
