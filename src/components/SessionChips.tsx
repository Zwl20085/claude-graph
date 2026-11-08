/**
 * Top bar chips of a terminal-mode run: one per agent session (title, status
 * colour, harness); a click opens or focuses that session window. The
 * backend opens each window itself when its session starts. A session that
 * no longer takes messages says "process exited".
 */
import { useRef } from 'react';
import { SquareTerminal } from 'lucide-react';
import { DEFAULT_HARNESS, runStateMode, sessionHarness, sessionsOf } from '../types/contract';
import { api } from '../lib/api';
import { DSH_MODE_LABEL, harnessLabel, isDshHarness } from '../lib/harness';
import { isSessionAlive, SESSION_COLORS } from '../lib/sessions';
import { STATUS_COLORS, WAITING_LABEL } from '../lib/status';
import { useGraphStore } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';

/** Chip badge of a session that waits on a question or dialog. */
export const NEEDS_YOU = WAITING_LABEL;
/** Title suffix of a session whose process is gone. */
export const PROCESS_EXITED = 'process exited';

export function openSessionWindow(runId: string, sid: string, toast: (text: string) => void): void {
  api.openSessionWindow(runId, sid).catch((err: unknown) => toast(`Could not open the session window: ${errorMessage(err)}`));
}

export function SessionChips({ vertical = false }: { vertical?: boolean }) {
  const runState = useGraphStore((s) => s.runState);
  const pushToast = useGraphStore((s) => s.pushToast);
  const harnesses = useGraphStore((s) => s.harnesses);
  const terminal = runStateMode(runState) === 'terminal';
  const sessions = terminal ? sessionsOf(runState) : [];
  const runId = terminal ? (runState?.run_id ?? null) : null;
  const toast = useRef((text: string) => pushToast('error', text)).current;
  if (runId === null || sessions.length === 0) return null;

  return (
    <div
      className={vertical ? 'flex flex-col items-start gap-1' : 'flex min-w-0 shrink items-center gap-1 overflow-hidden text-ellipsis whitespace-nowrap'}
      role="group"
      aria-label="sessions"
    >
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
        const exited = s.status !== 'starting' && s.status !== 'running' && !isSessionAlive(s);
        const state = `${needsYou ? NEEDS_YOU : s.status}${exited ? `, ${PROCESS_EXITED}` : ''}`;
        return (
          <button
            key={s.sid}
            className="flex min-w-[2.5rem] max-w-[180px] shrink items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] hover:bg-[#37373d]"
            style={{ borderColor: chipColor, color: chipColor }}
            title={`${s.title} (${state}, ${label}) — open the session window`}
            aria-label={`open session window ${s.title}, ${state}, harness ${label}`}
            onClick={() => openSessionWindow(runId, s.sid, toast)}
          >
            <SquareTerminal size={11} className="shrink-0" />
            <span className="truncate">{shown || s.sid}</span>
            {needsYou && (
              <span className="shrink-0 rounded px-1 text-[10px] font-semibold text-black" style={{ background: STATUS_COLORS.waiting }} aria-hidden="true">
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
