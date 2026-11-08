/**
 * Top bar pieces (UX pass, v0.6): the persistent "N need you" badge, the
 * read-only run chip, the File menu, the Run split-button, the default
 * harness chip, the notification bell, and the session dropdown used below
 * 1100 px. Each is a small component over the graph store.
 */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Bell, ChevronDown, CircleHelp, ExternalLink, FilePlus, FileText, FolderOpen, Play, PlayCircle, RotateCcw, Save } from 'lucide-react';
import { harnessOf, runModeOf, type HarnessCheck, type HarnessProfile, type RunState, type SessionPrompt, type Settings } from '../types/contract';
import { api } from '../lib/api';
import { findHarness, harnessLabel, isDshHarness } from '../lib/harness';
import { isRunActive, RERUN_FAILED_TITLE, runHasFailures, STATUS_COLORS } from '../lib/status';
import { graphStore, useGraphStore, type ClaudeState } from '../store/graphStore';
import { errorMessage } from '../store/runHelpers';
import { newGraphFlow, openFolderFlow } from './StartScreen';
import { btnCls } from './ui';

/** A menu that closes on Escape, on a click outside, and after an item ran. */
export function useMenu(): { open: boolean; setOpen: (o: boolean) => void; ref: React.RefObject<HTMLDivElement> } {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent): void => {
      if (ref.current && !ref.current.contains(e.target as globalThis.Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);
  return { open, setOpen, ref };
}

const menuCls = 'absolute left-0 top-8 z-40 min-w-[220px] rounded border border-[#3c3c3c] bg-[#252526] py-1 shadow-xl';
const itemCls = 'flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] hover:bg-[#37373d] disabled:opacity-40';

interface MenuItemProps {
  icon: typeof Play;
  label: string;
  keys?: string;
  disabled?: boolean;
  title?: string;
  onClick: () => void;
}

function MenuItem({ icon: Icon, label, keys, disabled, title, onClick }: MenuItemProps) {
  return (
    <button role="menuitem" className={itemCls} disabled={disabled} title={title} onClick={onClick}>
      <Icon size={13} className="shrink-0" />
      <span className="flex-1">{label}</span>
      {keys && <kbd className="text-[10px] text-[#8a8a8a]">{keys}</kbd>}
    </button>
  );
}

function Menu({ label, icon, children, open, setOpen }: { label: string; icon: ReactNode; children: ReactNode; open: boolean; setOpen: (o: boolean) => void }) {
  return (
    <>
      <button
        className={`${btnCls} shrink-0 whitespace-nowrap ${open ? 'bg-[#37373d]' : ''}`}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={label}
        title={label}
        onClick={() => setOpen(!open)}
      >
        {icon}
        <span className="hidden min-[900px]:inline">{label}</span>
        <ChevronDown size={11} />
      </button>
      {open && (
        <div role="menu" aria-label={label} className={menuCls}>
          {children}
        </div>
      )}
    </>
  );
}

/** New / Open graph… / Show in file manager / Save. */
export function FileMenu() {
  const { open, setOpen, ref } = useMenu();
  const dir = useGraphStore((s) => s.dir);
  const store = graphStore().getState;
  const run = (f: () => void) => () => {
    setOpen(false);
    f();
  };
  const reveal = (): void => {
    if (!dir) return;
    api.revealGraphDir(dir).catch((err) => store().pushToast('error', `Could not show the folder: ${errorMessage(err)}`));
  };
  return (
    <div className="relative" ref={ref}>
      <Menu label="File" icon={<FileText size={14} />} open={open} setOpen={setOpen}>
        <MenuItem icon={FilePlus} label="New graph…" onClick={run(() => void newGraphFlow())} />
        <MenuItem icon={FolderOpen} label="Open graph…" onClick={run(() => void openFolderFlow())} />
        <MenuItem icon={ExternalLink} label="Show in file manager" onClick={run(reveal)} />
        <MenuItem icon={Save} label="Save" keys="Ctrl+S" onClick={run(() => void store().save())} />
      </Menu>
    </div>
  );
}

/** Run all, with Run from selected and Rerun failed in its dropdown. */
export function RunSplitButton() {
  const { open, setOpen, ref } = useMenu();
  const runState = useGraphStore((s) => s.runState);
  const selected = useGraphStore((s) => s.selectedNodeId);
  const starting = useGraphStore((s) => s.startingRun);
  const store = graphStore().getState;
  const active = isRunActive(runState) || starting;
  const run = (f: () => void) => () => {
    setOpen(false);
    f();
  };
  return (
    <div className="relative flex shrink-0" ref={ref}>
      <button
        className={`${btnCls} shrink-0 whitespace-nowrap rounded-r-none bg-[#3b82f6]/20`}
        disabled={active}
        title="Run all (Ctrl+Enter)"
        aria-label="Run all"
        onClick={() => void store().startRun('all')}
      >
        <Play size={14} />
        <span className="hidden min-[900px]:inline">Run all</span>
      </button>
      <button
        className={`${btnCls} rounded-l-none border-l border-[#3c3c3c] bg-[#3b82f6]/20 px-1`}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="more run options"
        title="More run options"
        onClick={() => setOpen(!open)}
      >
        <ChevronDown size={11} />
      </button>
      {open && (
        <div role="menu" aria-label="Run" className={menuCls}>
          <MenuItem
            icon={PlayCircle}
            label="Run from selected"
            disabled={active || selected === null}
            onClick={run(() => selected && void store().startRun({ from: selected }))}
          />
          <MenuItem
            icon={RotateCcw}
            label="Rerun failed"
            title={RERUN_FAILED_TITLE}
            disabled={active || !runHasFailures(runState)}
            onClick={run(() => runState && void store().startRun({ failed_of: runState.run_id }))}
          />
        </div>
      )}
    </div>
  );
}

/** Prompts of the shown run (all known ones when no run is shown), oldest first. */
export function waitingPrompts(prompts: readonly SessionPrompt[], run: RunState | null): SessionPrompt[] {
  return run === null ? [...prompts] : prompts.filter((p) => p.run_id === run.run_id);
}

export function needYouText(n: number): string {
  return `${n} need${n === 1 ? 's' : ''} you`;
}

/** "N need you": a click reveals the oldest waiting node on its Chat tab. */
export function NeedsYouBadge() {
  const prompts = useGraphStore((s) => s.sessionPrompts);
  const runState = useGraphStore((s) => s.runState);
  const reveal = useGraphStore((s) => s.revealNode);
  const waiting = waitingPrompts(prompts, runState);
  const oldest = waiting[0];
  if (oldest === undefined) return null;
  const text = needYouText(waiting.length);
  return (
    <button
      className="inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-semibold text-black"
      style={{ background: STATUS_COLORS.waiting }}
      onClick={() => reveal(oldest.node_id, 'chat')}
      title="Show the oldest waiting node"
      aria-label={text}
    >
      <CircleHelp size={12} />
      {text}
    </button>
  );
}

/** "Windows · Claude Code", "Windows · dsh (headless)", "Headless · claude". */
export function runChipText(settings: Settings | null | undefined, profiles: readonly HarnessProfile[]): string {
  if (runModeOf(settings) === 'headless') return 'Headless · claude';
  const id = harnessOf(settings);
  if (isDshHarness(profiles, id)) return 'Windows · dsh (headless)';
  return `Windows · ${harnessLabel(profiles, id)}`;
}

/** Read-only summary of how Run all runs; a click opens Settings. */
export function RunChip({ onOpenSettings }: { onOpenSettings: () => void }) {
  const settings = useGraphStore((s) => s.graph?.settings);
  const harnesses = useGraphStore((s) => s.harnesses);
  const text = runChipText(settings, harnesses);
  return (
    <button
      className="shrink-0 rounded border border-[#3c3c3c] px-1.5 py-0.5 text-[11px] text-[#9a9a9a] hover:text-white"
      onClick={onOpenSettings}
      title="How Run all runs — open Settings to change it"
      aria-label={`run mode ${text}, open settings`}
    >
      {text}
    </button>
  );
}

type Probe = { kind: 'checking' } | { kind: 'done'; check: HarnessCheck } | { kind: 'error'; message: string };

/** Text of the default-harness chip. */
export function harnessChipText(profile: HarnessProfile | null, id: string, claude: ClaudeState, probe: Probe | null): { ok: boolean; text: string } {
  const label = profile?.label || id;
  if (profile === null || profile.kind === 'claude') {
    if (claude.kind === 'checking') return { ok: true, text: 'checking claude…' };
    const ok = claude.kind === 'ok' && claude.check.found;
    const version = claude.kind === 'ok' ? (claude.check.version ?? '') : '';
    return ok ? { ok, text: `${label} ${version}`.trim() } : { ok, text: 'claude not found — install Claude Code and log in' };
  }
  if (probe === null || probe.kind === 'checking') return { ok: true, text: `checking ${label}…` };
  if (probe.kind === 'error' || !probe.check.ok) return { ok: false, text: `${label} not found` };
  return { ok: true, text: `${label} ${probe.check.version ?? ''}`.trim() };
}

/** Status of the graph's default harness (claude kinds: the claude check; others: check_harness). */
export function HarnessChip() {
  const claude = useGraphStore((s) => s.claude);
  const harnesses = useGraphStore((s) => s.harnesses);
  const id = useGraphStore((s) => harnessOf(s.graph?.settings));
  const check = useGraphStore((s) => s.checkHarness);
  const profile = findHarness(harnesses, id);
  const needsProbe = profile !== null && profile.kind !== 'claude';
  const [probe, setProbe] = useState<Probe | null>(null);
  useEffect(() => {
    if (!needsProbe) return setProbe(null);
    let live = true;
    setProbe({ kind: 'checking' });
    check(id)
      .then((c) => live && setProbe(c === null ? { kind: 'error', message: 'check failed' } : { kind: 'done', check: c }))
      .catch((err: unknown) => live && setProbe({ kind: 'error', message: errorMessage(err) }));
    return () => {
      live = false;
    };
  }, [id, needsProbe, check]);
  const { ok, text } = harnessChipText(profile, id, claude, probe);
  return (
    <span
      title={claude.kind === 'ok' && !needsProbe ? (claude.check.path ?? undefined) : undefined}
      className={`shrink-0 rounded-full px-2 py-0.5 text-[11px] ${ok ? 'bg-[#22c55e]/15 text-[#22c55e]' : 'bg-[#ef4444]/15 text-[#ef4444]'}`}
    >
      {text}
    </span>
  );
}

/** The last notifications; "Show" reveals the node of one. */
export function NotificationBell() {
  const { open, setOpen, ref } = useMenu();
  const history = useGraphStore((s) => s.toastHistory);
  const reveal = useGraphStore((s) => s.revealNode);
  return (
    <div className="relative" ref={ref}>
      <button
        className={`${btnCls} ${open ? 'bg-[#37373d]' : ''}`}
        aria-label="notifications"
        aria-expanded={open}
        title="Last notifications"
        onClick={() => setOpen(!open)}
      >
        <Bell size={14} />
        {history.length > 0 && <span className="text-[10px] text-[#9a9a9a]">{history.length}</span>}
      </button>
      {open && (
        <div role="dialog" aria-label="Notifications" className="absolute right-0 top-9 z-40 max-h-[60vh] w-[360px] overflow-auto rounded border border-[#3c3c3c] bg-[#252526] p-2 shadow-xl">
          {history.length === 0 && <p className="text-[12px] text-[#777]">No notifications yet.</p>}
          <ul className="space-y-1">
            {[...history].reverse().map((t) => (
              <li key={t.id} className="flex items-start gap-2 rounded border border-[#3c3c3c] px-2 py-1 text-[12px]">
                <span className="flex-1 whitespace-pre-wrap">{t.text}</span>
                {t.nodeId !== undefined && (
                  <button className="shrink-0 rounded border border-[#3c3c3c] px-1.5 hover:bg-[#37373d]" onClick={() => {
                    setOpen(false);
                    reveal(t.nodeId ?? '');
                  }}>
                    Show
                  </button>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

/** Below 1100 px the session chips collapse to "N sessions" with the chips in a dropdown. */
export function SessionsDropdown({ count, children }: { count: number; children: ReactNode }) {
  const { open, setOpen, ref } = useMenu();
  if (count === 0) return null;
  return (
    <div className="relative shrink-0 min-[1100px]:hidden" ref={ref}>
      <button className={`${btnCls} border border-[#3c3c3c] text-[11px]`} aria-expanded={open} aria-label={`${count} sessions`} onClick={() => setOpen(!open)}>
        {count} session{count === 1 ? '' : 's'}
        <ChevronDown size={11} />
      </button>
      {open && (
        <div role="dialog" aria-label="Sessions" className="absolute left-0 top-8 z-40 flex flex-col gap-1 rounded border border-[#3c3c3c] bg-[#252526] p-2 shadow-xl">
          {children}
        </div>
      )}
    </div>
  );
}

