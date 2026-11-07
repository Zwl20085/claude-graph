/**
 * Live stream of the selected node in the shown run ("watch Claude work"),
 * or of the assistant's scheduler turn while the Assistant tab is open.
 */
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Copy, Eraser } from 'lucide-react';
import type { NodeEventKind } from '../types/contract';
import { api } from '../lib/api';
import { isRunActive } from '../lib/status';
import { useGraphStore } from '../store/graphStore';
import { useAssistantStore } from '../store/assistantStore';
import { ASSISTANT_NODE_ID } from '../types/contract';
import { errorMessage, type LogLine } from '../store/runHelpers';
import { btnCls } from './ui';

const KIND_COLORS: Record<NodeEventKind | 'raw', string> = {
  text: '#d4d4d4',
  tool_use: '#3b82f6',
  tool_result: '#8b5cf6',
  system: '#8a8a8a',
  stderr: '#f59e0b',
  result: '#22c55e',
  parse_error: '#ef4444',
  user_message: '#60a5fa',
  permission_denied: '#f59e0b',
  raw: '#6b7280',
};
/** Column label of a line kind (default: the kind itself). */
export function kindLabel(kind: NodeEventKind): string {
  return kind === 'permission_denied' ? 'DENIED' : kind;
}
const STICK_PX = 24;
const EMPTY: readonly LogLine[] = [];

/** events.jsonl tail of a finished run, when no live lines were captured in this session. */
function useHistoricTail(nodeId: string | null, live: number): { tail: string[]; error: string | null } {
  const dir = useGraphStore((s) => s.dir);
  const runState = useGraphStore((s) => s.runState);
  const [tail, setTail] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const runId = runState?.run_id ?? null;
  const finished = runState !== null && !isRunActive(runState);
  useEffect(() => {
    setTail([]);
    setError(null);
    if (!dir || !runId || !nodeId || !finished || live > 0) return;
    let on = true;
    api
      .readNodeResult(dir, runId, nodeId)
      .then((r) => on && setTail(r.events_tail))
      .catch((err) => on && setError(errorMessage(err)));
    return () => {
      on = false;
    };
  }, [dir, runId, nodeId, finished, live]);
  return { tail, error };
}

interface LogPanelProps {
  /** Show the assistant's scheduler stream instead of the selected node. */
  scheduler?: boolean;
}

export function LogPanel({ scheduler = false }: LogPanelProps) {
  const selected = useGraphStore((s) => s.selectedNodeId);
  const nodeLines = useGraphStore((s) =>
    selected !== null && s.eventsRunId !== null && s.eventsRunId === s.runState?.run_id ? (s.events[selected] ?? EMPTY) : EMPTY,
  );
  const schedulerLines = useAssistantStore((s) => s.log);
  const clearNode = useGraphStore((s) => s.clearEvents);
  const clearScheduler = useAssistantStore((s) => s.clearLog);
  const pushToast = useGraphStore((s) => s.pushToast);
  const nodeId = scheduler ? ASSISTANT_NODE_ID : selected;
  const lines = scheduler ? schedulerLines : nodeLines;
  const clear = (id: string): void => (scheduler ? clearScheduler() : clearNode(id));
  const { tail, error: tailError } = useHistoricTail(scheduler ? null : selected, lines.length);
  const box = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useLayoutEffect(() => {
    const el = box.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines, tail]);

  const copy = (): void => {
    const text = lines.length > 0 ? lines.map((l) => `[${l.kind}] ${l.text}`).join('\n') : tail.join('\n');
    navigator.clipboard
      .writeText(text)
      .then(() => pushToast('success', 'Log copied'))
      .catch((err) => pushToast('error', `Copy failed: ${String(err)}`));
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-7 shrink-0 items-center gap-2 border-b border-[#3c3c3c] px-2 text-[12px]">
        <span className="font-semibold">Log</span>
        <span className="font-mono text-[#9a9a9a]">{nodeId ?? 'no node selected'}</span>
        <span className="text-[#777]">{lines.length > 0 ? `${lines.length} lines` : tail.length > 0 ? 'events.jsonl tail' : ''}</span>
        <button className={`${btnCls} ml-auto`} onClick={() => nodeId && clear(nodeId)} disabled={lines.length === 0}>
          <Eraser size={13} />Clear
        </button>
        <button className={btnCls} onClick={copy} disabled={lines.length === 0 && tail.length === 0}>
          <Copy size={13} />Copy
        </button>
      </div>
      <div
        ref={box}
        className="min-h-0 flex-1 overflow-auto px-2 py-1 font-mono text-[12px] leading-5"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < STICK_PX;
        }}
      >
        {lines.map((l) => (
          <div key={l.seq} className="flex gap-2">
            <span className="w-[84px] shrink-0 text-right text-[10px] uppercase" style={{ color: KIND_COLORS[l.kind] }}>
              {kindLabel(l.kind)}
            </span>
            <span
              className="whitespace-pre-wrap break-all"
              style={l.kind === 'permission_denied' ? { color: KIND_COLORS.permission_denied } : undefined}
            >
              {l.text}
            </span>
          </div>
        ))}
        {lines.length === 0 &&
          tail.map((t, i) => (
            <div key={i} className="whitespace-pre-wrap break-all" style={{ color: KIND_COLORS.raw }}>
              {t}
            </div>
          ))}
        {lines.length === 0 && tailError !== null && (
          <p className="text-[#ef4444]">could not read events tail: {tailError}</p>
        )}
        {lines.length === 0 && tail.length === 0 && tailError === null && <p className="text-[#777]">{scheduler ? 'No scheduler output in this session yet.' : 'No output for this node in the shown run.'}</p>}
      </div>
    </div>
  );
}
