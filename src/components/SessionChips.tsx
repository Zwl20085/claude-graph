/**
 * Top bar chips of a terminal-mode run: one per Claude Code session (title,
 * status colour); a click opens or focuses that session's window. The backend
 * opens each window itself when its session starts.
 */
import { useRef } from 'react';
import { SquareTerminal } from 'lucide-react';
import { DEFAULT_HARNESS, runStateMode, sessionHarness, sessionsOf } from '../types/contract';
import { api } from '../lib/api';
import { DSH_MODE_LABEL, harnessLabel, isDshHarness } from '../lib/harness';
import { SESSION_COLORS } from '../lib/sessions';
import { STATUS_COLORS, WAITING_LABEL } from '../lib/status';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';

/** Chip badge of a session that waits on a question or dialog. */
export const NEEDS_YOU = WAITING_LABEL;

export function openSessionWindow(runId: string, sid: string, toast: (text: string) => void): void {
  api.openSessionWindow(runId, sid).catch((err: unknown) => toast(`Could not open the Claude Code window: ${errorMessage(err)}`));
}

export function SessionChips() {
  const runState = useGraphStore((s) => s.runState);
  const pushToast = useGraphStore((s) => s.pushToast);
  const harnesses = useGraphStore((s) => s.harnesses);
  const terminal = runStateMode(runState) === 'terminal';
  const sessions = terminal ? sessionsOf(runState) : [];
  const runId = terminal ? (runState?.run_id ?? null) : null;
  const toast = useRef((text: string) => pushToast('error', text)).current;
  if (runId === null || sessions.length === 0) return null;

  return (
    <div className="flex min-w-0 shrink items-center gap-1 overflow-hidden text-ellipsis whitespace-nowrap" role="group" aria-label="Claude Code sessions">
      {sessions.map((s) => {
        const color = SESSION_COLORS[s.status];
        const harness = sessionHarness(s);
        const label = isDshHarness(harnesses, harness) ? DSH_MODE_LABEL : harnessLabel(harnesses, harness);
        const other = harness !== DEFAULT_HARNESS;
        // The backend appends " (<label>)" to a non-claude session title; the badge shows it instead.
        const suffix = ` (${harnessLabel(harnesses, harness)})`;
        const shown = other && s.title.endsWith(suffix) ? s.title.slice(0, -suffix.length) : s.title;
        const needsYou = typeof s.pending_prompt === 'string';
        const chipColor = needsYou ? STATUS_COLORS.waiting : color;
        return (
          <button
            key={s.sid}
            className="flex min-w-[2.5rem] max-w-[180px] shrink items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] hover:bg-[#37373d]"
            style={{ borderColor: chipColor, color: chipColor }}
            title={`${s.title} (${needsYou ? NEEDS_YOU : s.status}, ${label}) — open the session window`}
            aria-label={`open Claude Code window ${s.title}, ${needsYou ? NEEDS_YOU : s.status}, harness ${label}`}
            onClick={() => openSessionWindow(runId, s.sid, toast)}
          >
            <SquareTerminal size={11} className="shrink-0" />
            <span className="truncate">{shown || s.sid}</span>
            {needsYou && (
              <span className="shrink-0 rounded bg-[#38bdf8] px-1 text-[10px] font-semibold text-black" aria-hidden="true">
                {NEEDS_YOU}
              </span>
            )}
            {other && (
              <span className="shrink-0 rounded bg-black/30 px-1 text-[10px]" aria-hidden="true">
                {label}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
