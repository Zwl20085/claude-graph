/** Chat with a node's Claude session in the shown run. */
import { useEffect, useRef, useState } from 'react';
import { Send } from 'lucide-react';
import type { NodeMessage } from '../types/contract';
import { api } from '../lib/api';
import {
  composerState,
  createMessageHistory,
  displayMessages,
  pruneLocal,
  type LocalMessage,
  type MessageHistory,
} from '../lib/messages';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { btnCls, inputCls } from './ui';

/**
 * Changes whenever the node's run state changes. The backend emits `result`
 * from the stream, before it writes the assistant reply to messages.jsonl, and
 * only then emits run:state; refetching on this key picks the reply up.
 */
function useNodeStateKey(nodeId: string): string {
  return useGraphStore((s) => {
    const n = s.runState?.nodes[nodeId];
    return n ? `${n.status}|${n.finished_at ?? ''}|${n.num_turns ?? ''}|${n.cost_usd ?? ''}` : '';
  });
}

/**
 * History from list_messages merged with live user_message events (M5);
 * refetched on result events and on every change of the node's run state.
 */
function useHistory(dir: string | null, runId: string | null, nodeId: string) {
  const [history, setHistory] = useState<NodeMessage[]>([]);
  const lines = useGraphStore((s) => (s.eventsRunId === runId ? s.events[nodeId] : undefined));
  const controller = useRef<MessageHistory | null>(null);
  const seen = useRef(0);
  const stateKey = useNodeStateKey(nodeId);

  useEffect(() => {
    setHistory([]);
    seen.current = 0;
    if (!dir || !runId) {
      controller.current = null;
      return;
    }
    const h = createMessageHistory(
      () => api.listMessages(dir, runId, nodeId),
      setHistory,
      (err) => console.warn('list_messages failed', err),
    );
    controller.current = h;
    void h.refresh();
    return () => h.dispose();
  }, [dir, runId, nodeId]);

  useEffect(() => {
    const fresh = (lines ?? []).filter((l) => l.seq > seen.current);
    if (fresh.length === 0) return;
    seen.current = fresh[fresh.length - 1]?.seq ?? seen.current;
    const h = controller.current;
    if (!h) return;
    h.addLive(fresh.filter((l) => l.kind === 'user_message').map((l): NodeMessage => ({ role: 'user', text: l.text, ts: l.ts })));
    if (fresh.some((l) => l.kind === 'result')) void h.refresh();
  }, [lines]);

  // Skip the first render: the setup effect above has just fetched.
  const lastKey = useRef(stateKey);
  useEffect(() => {
    if (lastKey.current === stateKey) return;
    lastKey.current = stateKey;
    void controller.current?.refresh();
  }, [stateKey]);

  return history;
}

export function MessagesTab({ nodeId }: { nodeId: string }) {
  const dir = useGraphStore((s) => s.dir);
  const runId = useGraphStore((s) => s.runState?.run_id ?? null);
  const status = useGraphStore((s) => s.runState?.nodes[nodeId]?.status ?? null);
  const pushToast = useGraphStore((s) => s.pushToast);
  const ensureTrusted = useGraphStore((s) => s.ensureTrusted);
  const history = useHistory(dir, runId, nodeId);
  const [local, setLocal] = useState<LocalMessage[]>([]);
  const [draft, setDraft] = useState('');
  const scroller = useRef<HTMLDivElement>(null);
  const composer = composerState(status);

  useEffect(() => setLocal((l) => pruneLocal(history, l)), [history]);
  const shown = displayMessages(history, local);
  useEffect(() => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [shown.length]);

  const send = async (): Promise<void> => {
    const text = draft.trim();
    if (!text || !dir || !runId || !composer.enabled) return;
    // A follow-up resumes Claude with the graph's permissions: same trust gate as a run.
    if (!(await ensureTrusted(dir))) return;
    const entry: LocalMessage = { text, ts: new Date().toISOString() };
    setLocal((l) => [...l, entry]);
    setDraft('');
    try {
      await api.sendMessage(dir, runId, nodeId, text);
    } catch (err) {
      setLocal((l) => l.filter((x) => x !== entry));
      setDraft(text);
      pushToast('error', `Message not sent: ${errorMessage(err)}`);
    }
  };

  return (
    <div className="flex h-full flex-col gap-2">
      <div ref={scroller} className="min-h-0 flex-1 space-y-2 overflow-auto">
        {shown.length === 0 && <p className="text-[12px] text-[#777]">No messages in this run yet.</p>}
        {shown.map((m) => (
          <div key={m.key} className={`flex ${m.role === 'user' ? 'justify-end' : 'justify-start'}`}>
            <div
              className={`max-w-[90%] whitespace-pre-wrap break-words rounded px-2 py-1.5 text-[12px] ${
                m.role === 'user' ? 'bg-[#1e3a5f] text-[#e5e5e5]' : 'bg-[#1e1e1e] text-[#d4d4d4]'
              }`}
            >
              {m.text}
              {m.queued && status === 'running' && (
                <span className="ml-2 rounded bg-[#f59e0b]/20 px-1 text-[10px] text-[#f59e0b]">queued</span>
              )}
            </div>
          </div>
        ))}
      </div>
      {composer.hint && <p className="text-[11px] text-[#9a9a9a]">{composer.hint}</p>}
      <div className="flex gap-1">
        <textarea
          aria-label="message to this node"
          className={`${inputCls} resize-none`}
          rows={3}
          disabled={!composer.enabled}
          value={draft}
          placeholder={composer.enabled ? 'Message this node (Ctrl+Enter to send)' : (composer.hint ?? '')}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button className={`${btnCls} self-end bg-[#3b82f6] text-white`} disabled={!composer.enabled || draft.trim() === ''} onClick={() => void send()}>
          <Send size={13} />Send
        </button>
      </div>
    </div>
  );
}
