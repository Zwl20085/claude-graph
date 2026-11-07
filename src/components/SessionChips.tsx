/**
 * Top bar chips of a terminal-mode run: one per Claude Code session (title,
 * status colour); a click opens or focuses that session's window. The backend
 * opens each window itself when its session starts.
 */
import { useRef } from 'react';
import { SquareTerminal } from 'lucide-react';
import { runStateMode, sessionsOf } from '../types/contract';
import { api } from '../lib/api';
import { SESSION_COLORS } from '../lib/sessions';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';

export function openSessionWindow(runId: string, sid: string, toast: (text: string) => void): void {
  api.openSessionWindow(runId, sid).catch((err: unknown) => toast(`Could not open the Claude Code window: ${errorMessage(err)}`));
}

export function SessionChips() {
  const runState = useGraphStore((s) => s.runState);
  const pushToast = useGraphStore((s) => s.pushToast);
  const terminal = runStateMode(runState) === 'terminal';
  const sessions = terminal ? sessionsOf(runState) : [];
  const runId = terminal ? (runState?.run_id ?? null) : null;
  const toast = useRef((text: string) => pushToast('error', text)).current;
  if (runId === null || sessions.length === 0) return null;

  return (
    <div className="flex min-w-0 shrink items-center gap-1 overflow-hidden text-ellipsis whitespace-nowrap" role="group" aria-label="Claude Code sessions">
      {sessions.map((s) => {
        const color = SESSION_COLORS[s.status];
        return (
          <button
            key={s.sid}
            className="flex min-w-[2.5rem] max-w-[180px] shrink items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] hover:bg-[#37373d]"
            style={{ borderColor: color, color }}
            title={`${s.title} (${s.status}) — open the Claude Code window`}
            aria-label={`open Claude Code window ${s.title}, ${s.status}`}
            onClick={() => openSessionWindow(runId, s.sid, toast)}
          >
            <SquareTerminal size={11} className="shrink-0" />
            <span className="truncate">{s.title || s.sid}</span>
          </button>
        );
      })}
    </div>
  );
}
