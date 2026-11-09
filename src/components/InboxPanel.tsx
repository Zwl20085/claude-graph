/**
 * The Inbox (docs/INBOX.md): settings of this machine, Scan now, last / next
 * scan, and the agent tips of the Vindictive folder with their protocol
 * state, what this app does with them, and buttons to the session window,
 * the generated graph and the results folder. Unreadable files are listed
 * with the reason.
 */
import { useState } from 'react';
import { ExternalLink, FolderOpen, Network, RefreshCw } from 'lucide-react';
import { INBOX_LIMITS, type InboxPhase, type InboxTip, type InboxTipState } from '../types/contract';
import { inboxSettingsOf, useInboxStore } from '../store/inboxStore';
import { graphStore } from '../store/graphStore';
import { btnCls, errorCls, Field, Modal, NumberInput } from './ui';

export const STATE_BADGES: Record<InboxTipState, { icon: string; label: string }> = {
  queued: { icon: '🕓', label: 'queued' },
  running: { icon: '⚙', label: 'running' },
  done: { icon: '⭐', label: 'done' },
  question: { icon: '❓', label: 'question' },
  failed: { icon: '⚠', label: 'failed' },
  none: { icon: '', label: 'newer protocol' },
};

export const PHASE_LABELS: Record<InboxPhase, string> = {
  waiting: 'waiting for a slot',
  working: 'session working',
  question: 'session waiting for you',
  finished: 'finished',
};

/** Local date and time, or a dash. */
export function when(iso: string | null): string {
  if (iso === null) return '—';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

function TipRow({ tip, onError }: { tip: InboxTip; onError: (e: string | null) => void }) {
  const openSession = useInboxStore((s) => s.openSession);
  const reveal = useInboxStore((s) => s.revealResults);
  const setOpen = useInboxStore((s) => s.setOpen);
  const badge = STATE_BADGES[tip.state];
  const run = tip.run;
  const openGraph = async (): Promise<void> => {
    if (!run?.graph_dir) return;
    setOpen(false);
    await graphStore().getState().openGraph(run.graph_dir);
  };
  return (
    <li className="space-y-1 rounded border border-[#3c3c3c] px-2 py-1.5" aria-label={`agent tip ${tip.id}`}>
      <div className="flex items-center gap-2">
        <span className="w-5 shrink-0 text-center" role="img" aria-label={`state ${badge.label}`} title={badge.label}>
          {badge.icon}
        </span>
        <span className="min-w-0 flex-1 truncate text-[13px]" title={tip.id}>
          {tip.title || tip.id}
        </span>
        <button
          className={btnCls}
          disabled={!run?.sid}
          aria-label={`open session window of ${tip.id}`}
          title="Open the session window"
          onClick={async () => onError(await openSession(tip.id))}
        >
          <ExternalLink size={13} />
        </button>
        <button
          className={btnCls}
          disabled={!run?.graph_dir}
          aria-label={`open graph of ${tip.id}`}
          title="Open the generated graph in the main view"
          onClick={() => void openGraph()}
        >
          <Network size={13} />
        </button>
        <button
          className={btnCls}
          aria-label={`reveal results of ${tip.id}`}
          title="Reveal the results folder"
          onClick={async () => onError(await reveal(tip.id))}
        >
          <FolderOpen size={13} />
        </button>
      </div>
      <div className="pl-7 text-[11px] text-[#8a8a8a]">
        {run ? <span aria-label="run state">{PHASE_LABELS[run.phase]}</span> : <span>{tip.decision}</span>}
        {run?.note && <span> · {run.note}</span>}
        {tip.host && <span> · last by {tip.host}{tip.at ? ` at ${tip.at}` : ''}</span>}
      </div>
    </li>
  );
}

function SettingsSection() {
  const app = useInboxStore((s) => s.app);
  const status = useInboxStore((s) => s.status);
  const busy = useInboxStore((s) => s.busy);
  const saveInbox = useInboxStore((s) => s.saveInbox);
  const saveAutostart = useInboxStore((s) => s.saveAutostart);
  const pickFolder = useInboxStore((s) => s.pickFolder);
  const [error, setError] = useState<string | null>(null);
  const inbox = inboxSettingsOf(app);
  const suggested = inbox.folder === '' ? status?.suggested_folder ?? null : null;
  const run = async (p: Promise<string | null>): Promise<void> => setError(await p);
  return (
    <section className="space-y-2" aria-label="Inbox settings">
      <label className="flex items-start gap-2 text-[13px]">
        <input
          type="checkbox"
          className="mt-1"
          checked={inbox.enabled}
          disabled={busy || app === null}
          aria-label="enable the inbox"
          onChange={(e) => void run(saveInbox({ enabled: e.target.checked }))}
        />
        <span>
          Run agent tips from the board on this machine
          <span className="block text-[11px] text-[#8a8a8a]">Enable it on one machine only.</span>
        </span>
      </label>
      <Field label="Tips folder" hint="The Vindictive folder (synced by OneDrive).">
        <div className="flex items-center gap-1">
          <span className="min-w-0 flex-1 truncate font-mono text-[12px]" aria-label="tips folder" title={inbox.folder}>
            {inbox.folder || 'not chosen'}
          </span>
          <button className={`${btnCls} border border-[#3c3c3c]`} disabled={busy} onClick={() => void run(pickFolder())}>
            Choose…
          </button>
        </div>
        {suggested && (
          <div className="mt-1 flex items-center gap-1 text-[11px] text-[#9a9a9a]">
            <span className="min-w-0 flex-1 truncate" title={suggested}>
              Suggested: {suggested}
            </span>
            <button className={btnCls} disabled={busy} onClick={() => void run(saveInbox({ folder: suggested }))}>
              Use it
            </button>
          </div>
        )}
      </Field>
      <div className="grid grid-cols-3 gap-2">
        <Field label="Every (hours)">
          <NumberInput
            value={inbox.interval_hours}
            min={INBOX_LIMITS.interval_hours.min}
            aria-label="scan interval in hours"
            onCommit={(n) => n !== null && void run(saveInbox({ interval_hours: n }))}
          />
        </Field>
        <Field label="At once">
          <NumberInput
            value={inbox.max_parallel}
            min={INBOX_LIMITS.max_parallel.min}
            aria-label="sessions at once"
            onCommit={(n) => n !== null && void run(saveInbox({ max_parallel: n }))}
          />
        </Field>
        <Field label="Timeout (min)">
          <NumberInput
            value={inbox.timeout_minutes}
            min={INBOX_LIMITS.timeout_minutes.min}
            aria-label="timeout in minutes"
            onCommit={(n) => n !== null && void run(saveInbox({ timeout_minutes: n }))}
          />
        </Field>
      </div>
      <label className="flex items-center gap-2 text-[13px]">
        <input
          type="checkbox"
          checked={app?.autostart === true}
          disabled={busy || app === null}
          aria-label="start with Windows"
          onChange={(e) => void run(saveAutostart(e.target.checked))}
        />
        Start Claude Graph with Windows
      </label>
      {error && <span className={errorCls}>{error}</span>}
    </section>
  );
}

export function InboxPanel() {
  const status = useInboxStore((s) => s.status);
  const scanNow = useInboxStore((s) => s.scanNow);
  const [error, setError] = useState<string | null>(null);
  const tips = status?.tips ?? [];
  return (
    <div className="flex min-h-[420px] gap-4" aria-label="Inbox">
      <div className="w-[300px] shrink-0">
        <SettingsSection />
      </div>
      <div className="min-w-0 flex-1 space-y-2">
        <div className="flex items-center gap-2 text-[12px]">
          <button
            className={`${btnCls} border border-[#3c3c3c]`}
            disabled={!status?.enabled || status.scanning}
            onClick={async () => setError(await scanNow())}
          >
            <RefreshCw size={13} />
            {status?.scanning ? 'Scanning…' : 'Scan now'}
          </button>
          <span className="text-[#9a9a9a]" aria-label="scan times">
            Last: {when(status?.last_scan ?? null)} · Next: {when(status?.next_scan ?? null)}
          </span>
        </div>
        {status?.host && <p className="text-[11px] text-[#8a8a8a]">This machine: {status.host}</p>}
        {(error ?? status?.last_error) && <span className={errorCls}>{error ?? status?.last_error}</span>}
        {tips.length === 0 ? (
          <p className="text-[12px] text-[#777]">
            {status?.last_scan ? 'No agent tips in the folder.' : 'Not scanned yet.'}
          </p>
        ) : (
          <ul className="max-h-[50vh] space-y-1 overflow-auto" aria-label="agent tips">
            {tips.map((t) => (
              <TipRow key={t.id} tip={t} onError={setError} />
            ))}
          </ul>
        )}
        {status && status.other_tips > 0 && (
          <p className="text-[11px] text-[#8a8a8a]">{status.other_tips} other tip(s) left alone.</p>
        )}
        {status && status.skipped.length > 0 && (
          <section aria-label="unreadable files" className="space-y-0.5">
            <h4 className="text-[11px] font-semibold text-[#f59e0b]">Skipped (never written)</h4>
            <ul className="space-y-0.5 text-[11px] text-[#9a9a9a]">
              {status.skipped.map((s) => (
                <li key={s.file}>
                  <span className="font-mono">{s.file}</span>: {s.reason}
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </div>
  );
}

/** The panel as a dialog of the main window (open from the top bar or the start screen). */
export function InboxDialog() {
  const open = useInboxStore((s) => s.open);
  const setOpen = useInboxStore((s) => s.setOpen);
  if (!open) return null;
  return (
    <Modal title="Inbox" onClose={() => setOpen(false)}>
      <InboxPanel />
    </Modal>
  );
}

/** Top bar / start screen button; shows how many tips wait for an answer. */
export function InboxButton() {
  const setOpen = useInboxStore((s) => s.setOpen);
  const questions = useInboxStore((s) => s.status?.tips.filter((t) => t.state === 'question').length ?? 0);
  return (
    <button className={btnCls} onClick={() => setOpen(true)} aria-label="open the inbox" title="Inbox: agent tips from Vindictive">
      Inbox{questions > 0 ? ` (${questions} ❓)` : ''}
    </button>
  );
}
