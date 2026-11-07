/** Asks the user to trust a graph's elevated settings before a run or a follow-up message. */
import { ShieldAlert } from 'lucide-react';
import { trustItemText } from '../lib/trust';
import { useGraphStore } from '../store/graphStore';
import { btnCls, Modal } from './ui';

export function TrustDialog() {
  const request = useGraphStore((s) => s.trustRequest);
  const resolve = useGraphStore((s) => s.resolveTrust);
  if (!request) return null;
  return (
    <Modal title="Trust this graph?" onClose={() => resolve(false)} small>
      <div className="space-y-3 text-[13px]">
        <p className="flex items-start gap-2 text-[#d4d4d4]">
          <ShieldAlert size={16} className="mt-0.5 shrink-0 text-[#f59e0b]" />
          This graph asks for elevated permissions. Run it only if you trust where graph.json came from.
        </p>
        <ul className="list-disc space-y-1 pl-6 text-[12px] text-[#d4d4d4]">
          {request.items.map((item, i) => (
            <li key={`${item.node_id ?? ''}-${item.kind}-${i}`}>{trustItemText(item)}</li>
          ))}
        </ul>
        <p className="text-[11px] text-[#8a8a8a]">
          Trust is remembered for this graph until one of these settings changes.
        </p>
        <div className="flex justify-end gap-2">
          <button className={`${btnCls} border border-[#3c3c3c]`} onClick={() => resolve(false)} data-autofocus>
            Cancel
          </button>
          <button className={`${btnCls} bg-[#b45309] text-white hover:bg-[#92400e]`} onClick={() => resolve(true)}>
            Trust this graph and run
          </button>
        </div>
      </div>
    </Modal>
  );
}
