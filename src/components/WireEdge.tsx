/** Bezier wire coloured by the status of its source node. */
import { memo } from 'react';
import { BaseEdge, getBezierPath, type EdgeProps } from '@xyflow/react';
import type { WireFlowEdge } from '../lib/flow';
import { edgeVisual } from '../lib/status';

function WireEdgeView(props: EdgeProps<WireFlowEdge>) {
  const { sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition, data, selected, markerEnd } = props;
  const [path] = getBezierPath({ sourceX, sourceY, targetX, targetY, sourcePosition, targetPosition });
  const look = edgeVisual(data?.sourceStatus ?? null);
  return (
    <BaseEdge
      path={path}
      markerEnd={markerEnd}
      className={look.animated ? 'cg-wire-run' : undefined}
      interactionWidth={16}
      style={{
        stroke: selected ? '#d4d4d4' : look.stroke,
        strokeWidth: selected ? 3 : 2,
        strokeDasharray: look.animated ? '8 6' : undefined,
        filter: look.glow ? `drop-shadow(0 0 4px ${look.glow})` : 'drop-shadow(0 0 2px rgba(138,138,138,0.35))',
      }}
    />
  );
}

export const WireEdge = memo(WireEdgeView);
