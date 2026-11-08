/** In-app toasts, auto-dismissed after a few seconds (errors stay longer). */
import { useEffect } from 'react';
import { X } from 'lucide-react';
import { useGraphStore, type Toast, type ToastKind } from '../store/graphStore';

const LIFETIME_MS: Record<ToastKind, number> = { info: 3500, success: 3500, warning: 9000, error: 7000 };
const COLORS: Record<ToastKind, string> = { info: '#3b82f6', success: '#22c55e', warning: '#f59e0b', error: '#ef4444' };

function ToastItem({ toast }: { toast: Toast }) {
  const dismiss = useGraphStore((s) => s.dismissToast);
  const select = useGraphStore((s) => s.select);
  useEffect(() => {
    const handle = setTimeout(() => dismiss(toast.id), LIFETIME_MS[toast.kind]);
    return () => clearTimeout(handle);
  }, [toast.id, toast.kind, dismiss]);
  return (
    <div
      role={toast.kind === 'error' ? 'alert' : 'status'}
      className="flex max-w-[420px] items-start gap-2 rounded border bg-[#252526] px-3 py-2 text-[13px] shadow-lg"
      style={{ borderColor: COLORS[toast.kind] }}
    >
      <span className="flex-1 whitespace-pre-wrap">{toast.text}</span>
      {toast.nodeId !== undefined && (
        <button
          className="shrink-0 rounded border border-[#3c3c3c] px-1.5 text-[12px] hover:bg-[#37373d]"
          aria-label={`show node ${toast.nodeId}`}
          onClick={() => {
            select(toast.nodeId ?? null);
            dismiss(toast.id);
          }}
        >
          Show
        </button>
      )}
      <button aria-label="dismiss notification" onClick={() => dismiss(toast.id)}>
        <X size={13} />
      </button>
    </div>
  );
}

export function Toasts() {
  const toasts = useGraphStore((s) => s.toasts);
  return (
    <div className="pointer-events-none fixed right-4 top-12 z-50 flex flex-col gap-2">
      {toasts.map((t) => (
        <div key={t.id} className="pointer-events-auto">
          <ToastItem toast={t} />
        </div>
      ))}
    </div>
  );
}
