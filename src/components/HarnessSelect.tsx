/** Harness pickers: graph default (Settings) and per node (Inherit + labels). */
import type { HarnessProfile } from '../types/contract';
import { findHarness } from '../lib/harness';
import { useGraphStore } from '../store/graphStore';
import { inputCls, useFieldA11y } from './ui';

const INHERIT = '';

interface HarnessSelectProps {
  /** null = inherit (only when `inheritLabel` is given). */
  value: string | null;
  onChange: (value: string | null) => void;
  /** Label of the inherited harness; shows the "Inherit" option. */
  inheritLabel?: string;
}

function UnknownOption({ profiles, value }: { profiles: readonly HarnessProfile[]; value: string | null }) {
  if (value === null || findHarness(profiles, value)) return null;
  return <option value={value}>{value} (not on this machine)</option>;
}

export function HarnessSelect({ value, onChange, inheritLabel }: HarnessSelectProps) {
  const profiles = useGraphStore((s) => s.harnesses);
  const a11y = useFieldA11y();
  return (
    <select
      className={inputCls}
      value={value ?? INHERIT}
      {...a11y}
      onChange={(e) => onChange(e.target.value === INHERIT ? null : e.target.value)}
    >
      {inheritLabel !== undefined && <option value={INHERIT}>Inherit ({inheritLabel})</option>}
      <UnknownOption profiles={profiles} value={value} />
      {profiles.map((p) => (
        <option key={p.id} value={p.id}>
          {p.label}
        </option>
      ))}
    </select>
  );
}

