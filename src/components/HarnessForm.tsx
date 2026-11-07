/** Edit form of one harness profile: save, check (`check_harness`), delete (customs only). */
import { useState } from 'react';
import { HARNESS_KINDS, STOP_MODES, type HarnessCheck, type HarnessProfile } from '../types/contract';
import { fromDraft, HARNESS_KIND_LABELS, SECRETS_NOTE, STOP_MODE_LABELS, toDraft, type HarnessDraft } from '../lib/harness';
import { harnessProfileError } from '../lib/validate';
import { useGraphStore } from '../store/graphStore';
import { btnCls, errorCls, Field, Input, inputCls, Select, TextArea } from './ui';

export interface HarnessFormProps {
  /** The profile as last saved, or a fresh draft when `isNew`. */
  profile: HarnessProfile;
  isNew: boolean;
  /** Ids already taken (a new profile must not reuse one). */
  takenIds: readonly string[];
  onSaved: (id: string) => void;
  onDeleted: () => void;
}

/** Client-side checks before save_harness; the backend repeats them. */
export function draftError(draft: HarnessDraft, builtin: boolean, isNew: boolean, takenIds: readonly string[]): string | null {
  const { profile, error } = fromDraft(draft, builtin);
  if (error || !profile) return error;
  if (isNew && takenIds.includes(profile.id)) return `id: "${profile.id}" is already used.`;
  return harnessProfileError(profile);
}

function CheckResult({ check }: { check: HarnessCheck }) {
  const color = check.ok ? '#22c55e' : '#ef4444';
  return (
    <span role="status" aria-label="harness check result" className="text-[12px]" style={{ color }}>
      {check.ok ? (check.version ?? 'found') : (check.message ?? 'not found')}
    </span>
  );
}

function CustomFields({ draft, set }: { draft: HarnessDraft; set: (patch: Partial<HarnessDraft>) => void }) {
  return (
    <>
      <Field label="Idle marker" hint="Text on screen when the CLI waits for input. Empty = wait for 3 s of quiet.">
        <Input value={draft.idle_marker} onChange={(e) => set({ idle_marker: e.target.value })} />
      </Field>
      <Field label="Dialog markers" hint="One per line: texts that mean a dialog is open.">
        <TextArea rows={2} className={`${inputCls} resize-y font-mono`} value={draft.dialog_markers} onChange={(e) => set({ dialog_markers: e.target.value })} />
      </Field>
      <fieldset className="space-y-1" aria-label="Turn end">
        <legend className="mb-1 text-[11px] uppercase tracking-wide text-[#9a9a9a]">Turn end</legend>
        {STOP_MODES.map((mode) => (
          <label key={mode} className="flex items-center gap-2 text-[13px]">
            <input type="radio" name="stop_mode" value={mode} checked={draft.stop_mode === mode} onChange={() => set({ stop_mode: mode })} />
            {STOP_MODE_LABELS[mode]}
          </label>
        ))}
      </fieldset>
      {draft.stop_mode === 'quiet' && (
        <Field label="Quiet ms" hint="No output for this long after the message = turn over. Empty = 8000.">
          <Input inputMode="numeric" value={draft.quiet_ms} placeholder="8000" onChange={(e) => set({ quiet_ms: e.target.value })} />
        </Field>
      )}
    </>
  );
}

export function HarnessForm({ profile, isNew, takenIds, onSaved, onDeleted }: HarnessFormProps) {
  const saveHarness = useGraphStore((s) => s.saveHarness);
  const deleteHarness = useGraphStore((s) => s.deleteHarness);
  const checkHarness = useGraphStore((s) => s.checkHarness);
  const [draft, setDraft] = useState<HarnessDraft>(() => toDraft(profile));
  const [error, setError] = useState<string | null>(null);
  const [check, setCheck] = useState<HarnessCheck | null>(null);
  const [busy, setBusy] = useState(false);
  const set = (patch: Partial<HarnessDraft>): void => {
    setDraft((d) => ({ ...d, ...patch }));
    setCheck(null);
  };
  const builtin = profile.builtin;
  const dirty = isNew || JSON.stringify(draft) !== JSON.stringify(toDraft(profile));

  const save = async (): Promise<void> => {
    const err = draftError(draft, builtin, isNew, takenIds);
    setError(err);
    const { profile: next } = fromDraft(draft, builtin);
    if (err || !next) return;
    setBusy(true);
    const backendErr = await saveHarness(next);
    setBusy(false);
    setError(backendErr);
    if (backendErr === null) onSaved(next.id);
  };
  const runCheck = async (): Promise<void> => {
    setBusy(true);
    setCheck(await checkHarness(profile.id));
    setBusy(false);
  };
  const remove = async (): Promise<void> => {
    setBusy(true);
    const ok = await deleteHarness(profile.id);
    setBusy(false);
    if (ok) onDeleted();
  };

  return (
    <form
      className="space-y-3"
      aria-label={`Edit harness ${profile.id}`}
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <div className="grid grid-cols-2 gap-2">
        <Field label="Id" hint={isNew ? 'a-z, 0-9 and -, max 32' : undefined}>
          <Input className={`${inputCls} font-mono`} value={draft.id} readOnly={!isNew} onChange={(e) => set({ id: e.target.value })} />
        </Field>
        <Field label="Kind">
          {builtin ? (
            <Input value={HARNESS_KIND_LABELS[draft.kind]} readOnly />
          ) : (
            <Select value={draft.kind} options={HARNESS_KINDS} label={(k) => HARNESS_KIND_LABELS[k]} onChange={(kind) => set({ kind })} />
          )}
        </Field>
      </div>
      <Field label="Label">
        <Input value={draft.label} readOnly={builtin} onChange={(e) => set({ label: e.target.value })} />
      </Field>
      <Field label="Command" hint={draft.kind === 'custom' ? 'Program path or name (required).' : 'Empty = find it like the app does today.'}>
        <Input className={`${inputCls} font-mono`} value={draft.command} placeholder={draft.kind === 'custom' ? '' : draft.kind} onChange={(e) => set({ command: e.target.value })} />
      </Field>
      <Field label="Args" hint="One per line, appended when the session starts.">
        <TextArea rows={2} className={`${inputCls} resize-y font-mono`} value={draft.args} onChange={(e) => set({ args: e.target.value })} />
      </Field>
      <Field label="Environment" hint="KEY=value, one per line.">
        <TextArea rows={4} className={`${inputCls} resize-y font-mono`} value={draft.env} onChange={(e) => set({ env: e.target.value })} />
      </Field>
      <p role="note" aria-label="secrets note" className="rounded border border-[#3c3c3c] bg-[#1e1e1e] px-2 py-1.5 text-[11px] text-[#9a9a9a]">
        {SECRETS_NOTE}
      </p>
      {draft.kind !== 'custom' && (
        <Field label="Model" hint={draft.kind === 'codex' ? 'Passed as -m at start. Empty = Codex default.' : 'Passed as --model at start. Empty = per node.'}>
          <Input className={`${inputCls} font-mono`} value={draft.model} onChange={(e) => set({ model: e.target.value })} />
        </Field>
      )}
      {draft.kind === 'custom' && <CustomFields draft={draft} set={set} />}
      {error && <span role="alert" className={errorCls}>{error}</span>}
      <div className="flex flex-wrap items-center gap-2">
        <button type="submit" className={`${btnCls} bg-[#3b82f6] text-white hover:bg-[#2563eb]`} disabled={busy || !dirty}>
          Save
        </button>
        <button
          type="button"
          className={`${btnCls} border border-[#3c3c3c]`}
          disabled={busy || isNew}
          title={dirty && !isNew ? 'Checks the saved profile; save first to check your edits' : undefined}
          onClick={() => void runCheck()}
        >
          Check
        </button>
        {!builtin && !isNew && (
          <button type="button" className={`${btnCls} border border-[#7f1d1d] text-[#ef4444]`} disabled={busy} onClick={() => void remove()} aria-label={`delete harness ${profile.id}`}>
            Delete
          </button>
        )}
        {check && <CheckResult check={check} />}
      </div>
    </form>
  );
}
