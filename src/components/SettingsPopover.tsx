/** Graph settings: concurrency, resource capacities, defaults, claude path. */
import { useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { MODELS, PERMISSION_MODES, type Settings } from '../types/contract';
import { useGraphStore } from '../store/graphStore';
import { btnCls, Field, inputCls, Select } from './ui';

const toInt = (raw: string, min: number): number | null => {
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) && n >= min ? n : null;
};

function ResourcesTable({ resources, onChange }: { resources: Settings['resources']; onChange: (r: Settings['resources']) => void }) {
  const [tag, setTag] = useState('');
  const entries = Object.entries(resources);
  const add = (): void => {
    const t = tag.trim();
    if (t === '' || t in resources) return;
    onChange({ ...resources, [t]: 1 });
    setTag('');
  };
  return (
    <div className="space-y-1">
      {entries.map(([k, cap]) => (
        <div key={k} className="flex items-center gap-1">
          <span className="flex-1 truncate font-mono text-[12px]">{k}</span>
          <input
            type="number"
            min={0}
            className={`${inputCls} w-20`}
            value={cap}
            onChange={(e) => {
              const n = toInt(e.target.value, 0);
              if (n !== null) onChange({ ...resources, [k]: n });
            }}
          />
          <button
            className={btnCls}
            aria-label={`remove ${k}`}
            onClick={() => onChange(Object.fromEntries(entries.filter(([x]) => x !== k)))}
          >
            <Trash2 size={13} />
          </button>
        </div>
      ))}
      <div className="flex items-center gap-1">
        <input className={inputCls} placeholder="new tag" value={tag} onChange={(e) => setTag(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && add()} />
        <button className={btnCls} onClick={add} aria-label="add resource">
          <Plus size={13} />
        </button>
      </div>
    </div>
  );
}

export function SettingsPopover() {
  const settings = useGraphStore((s) => s.graph?.settings);
  const update = useGraphStore((s) => s.updateSettings);
  const [binDraft, setBinDraft] = useState<string>(settings?.claude_bin ?? '');
  if (!settings) return null;
  return (
    <div className="absolute right-0 top-9 z-40 w-[340px] space-y-3 rounded border border-[#3c3c3c] bg-[#252526] p-3 shadow-xl">
      <Field label="max_parallel">
        <input
          type="number"
          min={1}
          className={inputCls}
          value={settings.max_parallel}
          onChange={(e) => {
            const n = toInt(e.target.value, 1);
            if (n !== null) update({ max_parallel: n });
          }}
        />
      </Field>
      <Field label="Resource capacities">
        <ResourcesTable resources={settings.resources} onChange={(resources) => update({ resources })} />
      </Field>
      <Field label="default_model">
        <Select value={settings.default_model} options={MODELS} onChange={(default_model) => update({ default_model })} />
      </Field>
      <Field label="default_permission_mode">
        <Select
          value={settings.default_permission_mode}
          options={PERMISSION_MODES.filter((m) => m !== 'inherit')}
          onChange={(default_permission_mode) => update({ default_permission_mode })}
        />
        {settings.default_permission_mode === 'bypassPermissions' && (
          <span className="mt-1 block text-[11px] text-[#ef4444]">
            bypassPermissions lets every node run any tool without asking. Use only in a sandbox.
          </span>
        )}
      </Field>
      <label className="flex items-center gap-2 text-[13px]">
        <input type="checkbox" checked={settings.stop_on_failure} onChange={(e) => update({ stop_on_failure: e.target.checked })} />
        stop_on_failure
      </label>
      <Field label="claude_bin" hint="Empty = find claude on PATH. Applied when the field loses focus.">
        <input
          className={inputCls}
          value={binDraft}
          placeholder="claude on PATH"
          onChange={(e) => setBinDraft(e.target.value)}
          onBlur={() => {
            const next = binDraft.trim() === '' ? null : binDraft.trim();
            if (next !== settings.claude_bin) update({ claude_bin: next });
          }}
        />
      </Field>
    </div>
  );
}
