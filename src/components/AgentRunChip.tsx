/**
 * Top-bar chip while the shown run is driven from a CLI (agent-called mode):
 * "Driven by <client> in your CLI", the agent's latest note as tooltip, and
 * Stop (marks the run cancelled so a new one can start). The app never types
 * into the CLI.
 */
import { useState } from 'react';
import { SquareTerminal, Square } from 'lucide-react';
import { agentRunOf, useAgentStore } from '../store/agentStore';
import { useGraphStore } from '../store/graphStore';
import { btnCls } from './ui';

export function drivenText(client: string, active: boolean): string {
  return active ? `Driven by ${client} in your CLI` : `Driven by ${client} (finished)`;
}

export function AgentRunChip() {
  const runId = useGraphStore((s) => s.runState?.run_id);
  const pushToast = useGraphStore((s) => s.pushToast);
  const info = useAgentStore((s) => agentRunOf(s.info, runId));
  const stopping = useAgentStore((s) => s.stopping);
  const stop = useAgentStore((s) => s.stop);
  const [confirming, setConfirming] = useState(false);
  if (info === null) return null;
  const title = info.last_note ? `Latest note: ${info.last_note}` : `Run ${info.run_id}`;

  const onStop = async (): Promise<void> => {
    setConfirming(false);
    const error = await stop();
    if (error) pushToast('error', `Could not stop the agent run: ${error}`);
    else pushToast('info', `Agent run stopped. ${info.client} will be told on its next call.`);
  };

  return (
    <span className="ml-1 inline-flex shrink-0 items-center gap-1" role="status">
      <span
        className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] ${
          info.active ? 'bg-[#8b5cf6]/20 text-[#c4b5fd]' : 'bg-[#3c3c3c] text-[#9a9a9a]'
        }`}
        title={title}
      >
        <SquareTerminal size={12} />
        {drivenText(info.client, info.active)}
      </span>
      {info.active && !confirming && (
        <button className={`${btnCls} text-[#ef4444]`} disabled={stopping} onClick={() => setConfirming(true)} aria-label="stop the agent run">
          <Square size={12} />
          Stop
        </button>
      )}
      {info.active && confirming && (
        <span className="inline-flex items-center gap-1 text-[11px]">
          Stop this run?
          <button className={`${btnCls} text-[#ef4444]`} onClick={() => void onStop()} aria-label="confirm stop">
            Yes, stop
          </button>
          <button className={btnCls} onClick={() => setConfirming(false)} aria-label="keep running">
            No
          </button>
        </span>
      )}
    </span>
  );
}
