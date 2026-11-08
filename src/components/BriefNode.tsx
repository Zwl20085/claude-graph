/** Blueprint-style node card: model-coloured header, status ring, two pins. */
import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import type { BriefFlowNode } from '../lib/flow';
import { isEmptyNode } from '../lib/graphOps';
import { MODEL_COLORS, ringStyle, statusLabel } from '../lib/status';

export const NODE_WIDTH = 260;

const pinStyle = (color: string): React.CSSProperties => ({
  width: 12,
  height: 12,
  background: color,
  border: '2px solid #1e1e1e',
  boxShadow: `0 0 0 1px ${color}`,
});

/** Goal if set, else the first two lines of the prompt. */
export function cardSummary(goal: string, prompt: string): string {
  if (goal.trim() !== '') return goal.trim();
  return prompt.trim().split(/\r?\n/).slice(0, 2).join('\n');
}

function BriefNodeView({ data, selected }: NodeProps<BriefFlowNode>) {
  const { node, status, reason, denied, harnessLabel, waiting } = data;
  const ring = ringStyle(status);
  const color = MODEL_COLORS[node.model];
  const empty = isEmptyNode(node);
  return (
    <div
      className={`overflow-hidden rounded-md bg-[#252526] text-[#d4d4d4] ${ring.className}`}
      style={{
        width: NODE_WIDTH,
        borderWidth: 2,
        borderStyle: ring.borderStyle,
        borderColor: ring.borderColor,
        outline: selected ? '1px solid #3b82f6' : 'none',
        outlineOffset: 2,
      }}
      title={reason ?? undefined}
    >
      <div className="flex items-center gap-2 px-2 py-1" style={{ background: color }}>
        <span className="flex-1 truncate text-[13px] font-semibold text-white">{node.title || node.id}</span>
        {status === 'waiting' && (
          <span
            className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-[#38bdf8] text-[11px] font-bold text-black"
            title="This node asked you something: answer it in the Messages tab."
            role="img"
            aria-label="needs you"
          >
            ?
          </span>
        )}
        {harnessLabel !== null && (
          <span
            className="max-w-[110px] truncate rounded bg-black/40 px-1.5 text-[10px] text-white"
            title={`Harness: ${harnessLabel}`}
            aria-label={`harness ${harnessLabel}`}
          >
            {harnessLabel}
          </span>
        )}
        <span className="rounded bg-black/30 px-1.5 text-[10px] uppercase tracking-wide text-white">{node.model}</span>
      </div>
      <div className="space-y-1.5 px-2 py-2 text-[12px]">
        <div className="flex items-center gap-1 text-[10px] text-[#8a8a8a]">
          <span className="font-mono">{node.id}</span>
          {status && <span className="ml-auto" style={{ color: ring.borderColor }}>{statusLabel(status)}</span>}
          {empty && <span className="ml-auto rounded bg-[#f59e0b] px-1 font-semibold text-black">empty</span>}
        </div>
        <p className="line-clamp-2 whitespace-pre-line text-[#c8c8c8]">
          {empty ? <span className="italic text-[#777]">No prompt yet</span> : cardSummary(node.goal, node.prompt)}
        </p>
        <div className="flex flex-wrap items-center gap-1">
          {node.resources.map((r) => (
            <span key={r} className="rounded-full bg-[#37373d] px-1.5 text-[10px]">
              {r}
            </span>
          ))}
          {waiting && (
            <span
              className="ml-auto rounded bg-[#f59e0b] px-1 text-[10px] font-semibold text-black"
              title="A tool call of this node waits for your approval."
              aria-label="waiting for you"
            >
              waiting for you
            </span>
          )}
          {denied && (
            <span
              className={`${waiting ? '' : 'ml-auto '}rounded bg-[#f59e0b]/20 px-1 text-[10px] text-[#f59e0b]`}
              title="A tool call of this node was denied in the shown run. See the Results tab."
              aria-label="tool call denied"
            >
              denied
            </span>
          )}
          {node.deliverables.length > 0 && (
            <span className={`${denied || waiting ? '' : 'ml-auto '}text-[10px] text-[#9a9a9a]`}>
              {node.deliverables.length} deliverable{node.deliverables.length === 1 ? '' : 's'}
            </span>
          )}
        </div>
      </div>
      <Handle type="target" position={Position.Left} style={pinStyle('#8a8a8a')} />
      <Handle type="source" position={Position.Right} style={pinStyle(color)} />
    </div>
  );
}

export const BriefNode = memo(BriefNodeView);
