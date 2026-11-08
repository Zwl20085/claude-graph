/**
 * Settings, in three groups: Run (how Run all runs), Defaults (what nodes
 * inherit; headless-only ones tagged), This machine (claude path, CLI add-on).
 * Choosing bypassPermissions asks first.
 */
import { useEffect, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { harnessOf, MODELS, PERMISSION_MODES, runModeOf, type RunMode, type Settings } from '../types/contract';
import { ASK_APP_HINT, permissionLabel } from '../lib/status';
import { IGNORED_CLAUDE_BIN_TEXT, useGraphStore } from '../store/graphStore';
import { CliAddonPanel } from './CliAddonPanel';
import { HarnessDialog } from './HarnessDialog';
import { HarnessSelect } from './HarnessSelect';
import { btnCls, errorCls, Field, Input, inputBoxCls, inputCls, NumberInput, Select, tagError } from './ui';
import { HEADLESS_ONLY } from './PromptTab';

export const BYPASS_CONFIRM =
  'Use bypassPermissions as the default? Every node may then run any tool, including shell commands, without asking. Use it only in a sandbox.';

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
    <section className="space-y-2" aria-label="This machine">
      <Field label="Claude Code program" hint="Empty: CLAUDE_GRAPH_CLAUDE_BIN, then claude on PATH. Applied on Enter or when the field loses focus." error={error}>
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
  { mode: 'terminal', label: 'Interactive windows (default)' },
  { mode: 'headless', label: 'Headless engine (advanced)' },
];

/** Run in: interactive session windows (terminal) or the headless engine. Missing = terminal. */
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

/** Graph default harness plus "Manage…" (the machine's profiles). */
function HarnessField({ value, onChange }: { value: string; onChange: (harness: string) => void }) {
  const [managing, setManaging] = useState(false);
  return (
    <Field label="Default harness" hint="The agent CLI nodes use unless they pick their own.">
      <div className="flex gap-1">
        <HarnessSelect value={value} onChange={(h) => h !== null && onChange(h)} />
        <button className={`${btnCls} shrink-0 border border-[#3c3c3c]`} onClick={() => setManaging(true)} aria-label="manage harnesses">
          Manage…
        </button>
      </div>
      {managing && <HarnessDialog onClose={() => setManaging(false)} />}
    </Field>
  );
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-3 border-t border-[#3c3c3c] pt-3 first:border-t-0 first:pt-0" aria-label={title}>
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-[#d4d4d4]">{title}</h3>
      {children}
    </section>
  );
}

export function SettingsPopover({ confirm = (t: string) => window.confirm(t) }: { confirm?: (text: string) => boolean }) {
  const settings = useGraphStore((s) => s.graph?.settings);
  const update = useGraphStore((s) => s.updateSettings);
  if (!settings) return null;
  const terminal = runModeOf(settings) === 'terminal';
  const headless = terminal ? ` (${HEADLESS_ONLY})` : '';
  return (
    <div
      role="dialog"
      aria-label="Settings"
      className="absolute right-0 top-9 z-40 max-h-[80vh] w-[360px] space-y-3 overflow-auto rounded border border-[#3c3c3c] bg-[#252526] p-3 shadow-xl"
    >
      <Group title="Run">
        <RunModeRadio value={runModeOf(settings)} onChange={(run_mode) => update({ run_mode })} />
        <HarnessField value={harnessOf(settings)} onChange={(harness) => update({ harness })} />
        <Field label="Max parallel" hint="How many nodes (or session windows) may run at the same time.">
          <NumberInput value={settings.max_parallel} min={1} onCommit={(n) => n !== null && update({ max_parallel: n })} />
        </Field>
        <label className="flex items-start gap-2 text-[13px]">
          <input type="checkbox" className="mt-1" checked={settings.stop_on_failure} onChange={(e) => update({ stop_on_failure: e.target.checked })} />
          <span>
            Stop when a node fails
            <span className="block text-[11px] text-[#8a8a8a]">The nodes after it are skipped instead of run.</span>
          </span>
        </label>
      </Group>
      <Group title="Defaults">
        <Field label={`Default model${headless}`} hint="Nodes set to inherit use this model.">
          <Select value={settings.default_model} options={MODELS} disabled={terminal} onChange={(default_model) => update({ default_model })} />
        </Field>
        <Field label={`Default permission mode${headless}`} hint="What a node may do without asking, unless it sets its own.">
          <Select
            value={settings.default_permission_mode}
            options={PERMISSION_MODES.filter((m) => m !== 'inherit')}
            label={permissionLabel}
            disabled={terminal}
            onChange={(default_permission_mode) => {
              if (default_permission_mode === 'bypassPermissions' && !confirm(BYPASS_CONFIRM)) return;
              update({ default_permission_mode });
            }}
          />
          {settings.default_permission_mode === 'ask_app' && (
            <span className="mt-1 block text-[11px] text-[#f59e0b]">{ASK_APP_HINT}</span>
          )}
          {settings.default_permission_mode === 'bypassPermissions' && (
            <span className="mt-1 block text-[11px] text-[#ef4444]">
              bypassPermissions lets every node run any tool without asking. Use only in a sandbox.
            </span>
          )}
        </Field>
        <Field label={`Resource capacities${headless}`} hint="How many nodes holding each tag may run at once." composite>
          <ResourcesTable resources={settings.resources} onChange={(resources) => update({ resources })} />
        </Field>
        {settings.claude_bin !== null && (
          <p className="text-[11px] text-[#f59e0b]" title={settings.claude_bin}>
            graph.json sets claude_bin: {IGNORED_CLAUDE_BIN_TEXT}.
          </p>
        )}
      </Group>
      <Group title="This machine">
        <AppSettingsSection />
        <CliAddonPanel />
      </Group>
    </div>
  );
}
