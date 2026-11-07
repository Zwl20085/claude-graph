/** Graph settings (concurrency, resources, defaults) and per-machine app settings (claude path). */
import { useEffect, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { MODELS, PERMISSION_MODES, runModeOf, type RunMode, type Settings } from '../types/contract';
import { permissionLabel } from '../lib/status';
import { IGNORED_CLAUDE_BIN_TEXT, useGraphStore } from '../store/graphStore';
import { btnCls, errorCls, Field, Input, inputBoxCls, inputCls, NumberInput, Select, tagError } from './ui';

function ResourcesTable({ resources, onChange }: { resources: Settings['resources']; onChange: (r: Settings['resources']) => void }) {
  const [tag, setTag] = useState('');
  const [error, setError] = useState<string | null>(null);
  const entries = Object.entries(resources);
  const add = (): void => {
    const t = tag.trim();
    const err = t in resources ? `"${t}" already exists.` : tagError(t);
    setError(err);
    if (err) return;
    onChange({ ...resources, [t]: 1 });
    setTag('');
  };
  return (
    <div className="space-y-1">
      {entries.map(([k, cap]) => (
        <div key={k} className="flex items-center gap-1">
          <span className="min-w-[3rem] flex-1 truncate font-mono text-[12px]" title={k}>
            {k}
          </span>
          <NumberInput
            className={`${inputBoxCls} w-20 shrink-0`}
            value={cap}
            min={0}
            aria-label={`capacity of ${k}`}
            onCommit={(n) => n !== null && onChange({ ...resources, [k]: n })}
          />
          <button
            className={btnCls}
            aria-label={`remove resource ${k}`}
            onClick={() => onChange(Object.fromEntries(entries.filter(([x]) => x !== k)))}
          >
            <Trash2 size={13} />
          </button>
        </div>
      ))}
      <div className="flex items-center gap-1">
        <input
          className={inputCls}
          placeholder="new tag"
          aria-label="new resource tag"
          value={tag}
          onChange={(e) => setTag(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && add()}
        />
        <button className={btnCls} onClick={add} aria-label="add resource">
          <Plus size={13} />
        </button>
      </div>
      {error && <span className={errorCls}>{error}</span>}
    </div>
  );
}

/** Per-machine claude path: get_app_settings / set_app_settings, backend errors inline. */
function AppSettingsSection() {
  const appSettings = useGraphStore((s) => s.appSettings);
  const saveAppSettings = useGraphStore((s) => s.saveAppSettings);
  const loadAppSettings = useGraphStore((s) => s.loadAppSettings);
  const [draft, setDraft] = useState(appSettings?.claude_bin ?? '');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (appSettings === null) void loadAppSettings();
  }, [appSettings, loadAppSettings]);
  useEffect(() => setDraft(appSettings?.claude_bin ?? ''), [appSettings]);

  const apply = async (): Promise<void> => {
    const next = draft.trim() === '' ? null : draft.trim();
    if (next === (appSettings?.claude_bin ?? null)) return setError(null);
    setBusy(true);
    setError(await saveAppSettings({ claude_bin: next }));
    setBusy(false);
  };

  return (
    <section className="space-y-2 border-t border-[#3c3c3c] pt-3" aria-label="App settings (this machine)">
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[#d4d4d4]">App settings (this machine)</h3>
      <Field label="claude path" hint="Empty = CLAUDE_GRAPH_CLAUDE_BIN, then claude on PATH. Applied on Enter or when the field loses focus." error={error}>
        <Input
          className={`${inputCls} font-mono ${error ? 'border-[#ef4444]' : ''}`}
          value={draft}
          disabled={busy}
          placeholder="claude on PATH"
          aria-invalid={error !== null}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => void apply()}
          onKeyDown={(e) => e.key === 'Enter' && void apply()}
        />
      </Field>
    </section>
  );
}

const RUN_MODE_CHOICES: ReadonlyArray<{ mode: RunMode; label: string }> = [
  { mode: 'terminal', label: 'Claude Code windows (default)' },
  { mode: 'headless', label: 'headless engine (advanced)' },
];

/** Run in: Claude Code windows (terminal) or the headless engine. Missing = terminal. */
export function RunModeRadio({ value, onChange }: { value: RunMode; onChange: (mode: RunMode) => void }) {
  return (
    <fieldset className="space-y-1" aria-label="Run in">
      <legend className="mb-1 text-[11px] text-[#9a9a9a]">Run in</legend>
      {RUN_MODE_CHOICES.map(({ mode, label }) => (
        <label key={mode} className="flex items-center gap-2 text-[13px]">
          <input type="radio" name="run_mode" value={mode} checked={value === mode} onChange={() => onChange(mode)} />
          {label}
        </label>
      ))}
    </fieldset>
  );
}

export function SettingsPopover() {
  const settings = useGraphStore((s) => s.graph?.settings);
  const update = useGraphStore((s) => s.updateSettings);
  if (!settings) return null;
  return (
    <div
      role="dialog"
      aria-label="Settings"
      className="absolute right-0 top-9 z-40 max-h-[80vh] w-[340px] space-y-3 overflow-auto rounded border border-[#3c3c3c] bg-[#252526] p-3 shadow-xl"
    >
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[#d4d4d4]">Graph settings</h3>
      <RunModeRadio value={runModeOf(settings)} onChange={(run_mode) => update({ run_mode })} />
      <Field label="max_parallel">
        <NumberInput value={settings.max_parallel} min={1} onCommit={(n) => n !== null && update({ max_parallel: n })} />
      </Field>
      <Field label="Resource capacities" composite>
        <ResourcesTable resources={settings.resources} onChange={(resources) => update({ resources })} />
      </Field>
      <Field label="default_model">
        <Select value={settings.default_model} options={MODELS} onChange={(default_model) => update({ default_model })} />
      </Field>
      <Field label="default_permission_mode">
        <Select
          value={settings.default_permission_mode}
          options={PERMISSION_MODES.filter((m) => m !== 'inherit')}
          label={permissionLabel}
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
      {settings.claude_bin !== null && (
        <p className="text-[11px] text-[#f59e0b]" title={settings.claude_bin}>
          graph.json sets claude_bin: {IGNORED_CLAUDE_BIN_TEXT}.
        </p>
      )}
      <AppSettingsSection />
    </div>
  );
}
