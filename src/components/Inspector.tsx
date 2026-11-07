/** Right panel for the selected node: Prompt | Brief | Messages | Results. */
import { useEffect, useState } from 'react';
import { Eye } from 'lucide-react';
import type { Node } from '../types/contract';
import { useGraphStore } from '../store/graphStore';
import { BriefForm } from './BriefForm';
import { MessagesTab } from './MessagesTab';
import { PreviewModal } from './PreviewModal';
import { PromptTab } from './PromptTab';
import { ResultsTab } from './ResultsTab';
import { btnCls, inputCls } from './ui';

const TABS = ['Prompt', 'Brief (recommended)', 'Messages', 'Results'] as const;
type Tab = (typeof TABS)[number];

function IdField({ node }: { node: Node }) {
  const fresh = useGraphStore((s) => s.freshNodeIds.includes(node.id));
  const rename = useGraphStore((s) => s.renameNodeId);
  const [draft, setDraft] = useState(node.id);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setDraft(node.id), [node.id]);
  const commit = (): void => {
    const err = rename(node.id, draft.trim());
    setError(err);
    if (err) setDraft(node.id);
  };
  return (
    <div>
      <input
        className={`${inputCls} font-mono text-[12px]`}
        value={draft}
        readOnly={!fresh}
        title={fresh ? 'Editable until the node first runs' : 'Node ids are fixed once created and run'}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => fresh && commit()}
        onKeyDown={(e) => e.key === 'Enter' && fresh && commit()}
      />
      {error && <p className="mt-0.5 text-[11px] text-[#ef4444]">{error}</p>}
    </div>
  );
}

export function Inspector() {
  const node = useGraphStore((s) => s.graph?.nodes.find((n) => n.id === s.selectedNodeId) ?? null);
  const update = useGraphStore((s) => s.updateNode);
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
          onChange={(e) => update(node.id, { title: e.target.value })}
        />
        <IdField key={node.id} node={node} />
      </div>
      <div className="flex border-b border-[#3c3c3c] text-[12px]">
        {TABS.map((t) => (
          <button
            key={t}
            className={`px-2.5 py-1.5 ${tab === t ? 'border-b-2 border-[#3b82f6] text-white' : 'text-[#9a9a9a] hover:text-[#d4d4d4]'}`}
            onClick={() => setTab(t)}
          >
            {t}
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-2">
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
