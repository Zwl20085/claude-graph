/**
 * Run summary strip under the top bar: the shown run's status, its non-zero
 * status counts, total cost, and "Rerun failed" once it ended with a node
 * that did not succeed.
 */
import { RotateCcw } from 'lucide-react';
import { countStatuses, formatCost, isRunActive, RERUN_FAILED_TITLE, runHasFailures, STATUS_COLORS, statusLabel } from '../lib/status';
import { runCost } from '../store/runWatch';
import { useGraphStore } from '../store/graphStore';
import { btnCls } from './ui';

export function RunSummaryStrip() {
  const run = useGraphStore((s) => s.runState);
  const startRun = useGraphStore((s) => s.startRun);
  const starting = useGraphStore((s) => s.startingRun);
  if (run === null) return null;
  const counts = Object.entries(countStatuses(run)).filter(([, n]) => n > 0);
  return (
    <div role="status" aria-label="run summary" className="flex h-7 shrink-0 items-center gap-3 border-b border-[#3c3c3c] bg-[#1f1f1f] px-2 text-[11px]">
      <span className="font-mono text-[#8a8a8a]">{run.run_id}</span>
      <span className="font-semibold">{isRunActive(run) ? 'running' : run.status}</span>
      {counts.map(([s, n]) => (
        <span key={s} style={{ color: STATUS_COLORS[s as keyof typeof STATUS_COLORS] }}>
          {n} {statusLabel(s as keyof typeof STATUS_COLORS)}
        </span>
      ))}
      <span className="text-[#9a9a9a]">{formatCost(runCost(run))}</span>
      {runHasFailures(run) && (
        <button
          className={`${btnCls} ml-auto border border-[#3c3c3c] py-0`}
          title={RERUN_FAILED_TITLE}
          disabled={starting}
          onClick={() => void startRun({ failed_of: run.run_id })}
        >
          <RotateCcw size={12} />Rerun failed
        </button>
      )}
    </div>
  );
}
