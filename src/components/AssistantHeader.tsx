/** Header row of the Assistant tab: status pill, turn, model, Cancel / Reset session / Open folder. */
import { useState } from 'react';
import { ExternalLink, RotateCcw, Square } from 'lucide-react';
import type { AssistantSession, AssistantStatus } from '../types/contract';
import { STATUS_LOOK } from '../lib/assistantView';
import { btnCls } from './ui';

export function StatusPill({ status }: { status: AssistantStatus }) {
  const look = STATUS_LOOK[status];
  return (
    <span
      data-status={status}
      className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px]"
      style={{ color: look.color, backgroundColor: `${look.color}26` }}
    >
      <span className={`h-1.5 w-1.5 rounded-full ${look.pulse ? 'animate-pulse' : ''}`} style={{ backgroundColor: look.color }} />
      {look.label}
    </span>
  );
}

interface ResetControlProps {
  busy: boolean;
  onReset: () => void;
}

/** "Reset session" with an inline confirm. */
function ResetControl({ busy, onReset }: ResetControlProps) {
  const [confirming, setConfirming] = useState(false);
  if (!confirming) {
    return (
      <button className={btnCls} disabled={busy} onClick={() => setConfirming(true)} aria-label="reset the scheduler session">
        <RotateCcw size={13} />Reset session
      </button>
    );
  }
  return (
    <span className="inline-flex items-center gap-1 text-[#f59e0b]">
      Start a fresh session?
      <button
        className={`${btnCls} text-[#f59e0b]`}
        disabled={busy}
        aria-label="confirm reset"
        onClick={() => {
          setConfirming(false);
          onReset();
        }}
      >
        Reset
      </button>
      <button className={btnCls} onClick={() => setConfirming(false)} aria-label="keep the session">
        Keep
      </button>
    </span>
  );
}

interface AssistantHeaderProps {
  session: AssistantSession;
  busy: boolean;
  onCancel: () => void;
  onReset: () => void;
  onOpenFolder: () => void;
}

export function AssistantHeader({ session, busy, onCancel, onReset, onOpenFolder }: AssistantHeaderProps) {
  const working = session.status === 'working';
  return (
    <div className="space-y-1 border-b border-[#3c3c3c] p-2 text-[12px]">
      <div className="flex flex-wrap items-center gap-2">
        <StatusPill status={session.status} />
        <span className="text-[#9a9a9a]">turn {session.turns}</span>
        {session.model !== null && (
          <span className="font-mono text-[11px] text-[#8a8a8a]" title="model reported by the CLI">
            {session.model}
          </span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-1">
        {working ? (
          <button className={`${btnCls} text-[#ef4444]`} disabled={busy} onClick={onCancel} aria-label="cancel the scheduler turn">
            <Square size={13} />Cancel
          </button>
        ) : (
          <ResetControl busy={busy} onReset={onReset} />
        )}
        <button className={`${btnCls} ml-auto`} onClick={onOpenFolder} aria-label="open the graph folder" title="Show the graph folder in the file manager">
          <ExternalLink size={13} />Open folder
        </button>
      </div>
      {session.status === 'failed' && session.last_error !== null && (
        <p role="alert" className="whitespace-pre-wrap text-[11px] text-[#ef4444]">
          {session.last_error}
        </p>
      )}
    </div>
  );
}
