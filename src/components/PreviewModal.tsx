/** Rendered brief preview (render_brief, debounced 400 ms; local renderer in mock mode). */
import { useEffect, useState } from 'react';
import { api } from '../lib/api';
import { useDebouncedValue } from '../hooks/useDebouncedValue';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { Modal } from './ui';

export const PREVIEW_DEBOUNCE_MS = 400;

type Preview = { kind: 'loading' } | { kind: 'ok'; text: string } | { kind: 'error'; message: string };

export function PreviewModal({ nodeId, onClose }: { nodeId: string; onClose: () => void }) {
  const graph = useDebouncedValue(useGraphStore((s) => s.graph), PREVIEW_DEBOUNCE_MS);
  const [preview, setPreview] = useState<Preview>({ kind: 'loading' });

  useEffect(() => {
    if (!graph) return;
    let live = true;
    api
      .renderBrief(graph, nodeId)
      .then((text) => live && setPreview({ kind: 'ok', text }))
      .catch((err) => live && setPreview({ kind: 'error', message: errorMessage(err) }));
    return () => {
      live = false;
    };
  }, [graph, nodeId]);

  return (
    <Modal title={`Brief preview — ${nodeId}`} onClose={onClose}>
      {preview.kind === 'loading' && <p className="text-[12px] text-[#777]">Rendering…</p>}
      {preview.kind === 'error' && <p className="text-[12px] text-[#ef4444]">render_brief failed: {preview.message}</p>}
      {preview.kind === 'ok' && (
        <pre className="whitespace-pre-wrap font-mono text-[12px] leading-5 text-[#d4d4d4]">{preview.text}</pre>
      )}
    </Modal>
  );
}
