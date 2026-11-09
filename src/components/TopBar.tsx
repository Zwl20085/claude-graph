/** Top bar: graph name, File menu, Add node, Run split-button and run chip, sessions, needs-you / approvals badges, harness status, bell, log, settings. */
import { useEffect, useRef, useState, type Dispatch, type RefObject, type SetStateAction } from 'react';
import { useReactFlow } from '@xyflow/react';
import { type LucideIcon, PanelBottom, Plus, Settings as SettingsIcon, ShieldQuestion, Square } from 'lucide-react';
import { runStateMode, sessionsOf } from '../types/contract';
import { MOCK_MODE } from '../lib/api';
import { isRunActive } from '../lib/status';
import { graphStore, useGraphStore } from '../store/graphStore';
import { FileMenu, HarnessChip, NeedsYouBadge, NotificationBell, RunChip, RunSplitButton, SessionsDropdown } from './TopBarParts';
import { GRID } from './Canvas';
import { SessionChips } from './SessionChips';
import { AgentRunChip } from './AgentRunChip';
import { SettingsPopover } from './SettingsPopover';
import { InboxButton } from './InboxPanel';
import { StatusPill } from './AssistantHeader';
import { showsPill } from '../lib/assistantView';
import { useAssistantStore } from '../store/assistantStore';
import { approvalsText } from '../store/permissionSlice';
import { btnCls } from './ui';

/** Breakpoint below which toolbar buttons show only their icon (the bar stays one line). */
export const LABEL_CLS = 'hidden min-[1200px]:inline';
const barBtnCls = `${btnCls} shrink-0 whitespace-nowrap`;

interface BarButtonProps {
  icon: LucideIcon;
  label: string;
  title?: string;
  disabled?: boolean;
  className?: string;
  onClick: () => void;
}

/** Toolbar button: icon plus a label that hides below 1200 px (name kept in aria-label / title). */
function BarButton({ icon: Icon, label, title, disabled, className = '', onClick }: BarButtonProps) {
  return (
    <button className={`${barBtnCls} ${className}`} disabled={disabled} onClick={onClick} title={title ?? label} aria-label={label}>
      <Icon size={14} className="shrink-0" />
      <span className={LABEL_CLS}>{label}</span>
    </button>
  );
}

/** Popover open state that closes on Escape and on a mouse-down outside `ref`. */
function useDismissable(): [boolean, Dispatch<SetStateAction<boolean>>, RefObject<HTMLDivElement>] {
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
  return [open, setOpen, ref];
}

/** Assistant status while it works or waits for an answer; a click opens the Assistant tab. */
function AssistantChip({ onOpen }: { onOpen: () => void }) {
  const status = useAssistantStore((s) => s.state?.session.status ?? null);
  if (status === null || !showsPill(status)) return null;
  return (
    <button onClick={onOpen} title="Open the Assistant tab" aria-label="open the assistant tab">
      <StatusPill status={status} />
    </button>
  );
}

/** Amber "N approval(s) needed" while permission prompts are pending; a click shows the dialog. */
export function ApprovalsBadge() {
  const count = useGraphStore((s) => s.permissionPrompts.length);
  const show = useGraphStore((s) => s.showPermissionDialog);
  if (count === 0) return null;
  const text = approvalsText(count);
  return (
    <button
      className="inline-flex shrink-0 items-center gap-1 rounded-full bg-[#f59e0b] px-2 py-0.5 text-[11px] font-semibold text-black hover:bg-[#fbbf24]"
      onClick={show}
      title="Show the approval dialog"
      aria-label={text}
    >
      <ShieldQuestion size={12} />
      {text}
    </button>
  );
}

interface TopBarProps {
  logVisible: boolean;
  onToggleLog: () => void;
  onOpenAssistant: () => void;
}

export function TopBar({ logVisible, onToggleLog, onOpenAssistant }: TopBarProps) {
  const name = useGraphStore((s) => s.graph?.name ?? '');
  const dirty = useGraphStore((s) => s.dirty);
  const runState = useGraphStore((s) => s.runState);
  const sessionCount = useGraphStore((s) => (runStateMode(s.runState) === 'terminal' ? sessionsOf(s.runState).length : 0));
  const [settingsOpen, setSettingsOpen, settingsRef] = useDismissable();
  const { screenToFlowPosition } = useReactFlow();
  const store = graphStore().getState;

  const addNode = (): void => {
    const p = screenToFlowPosition({ x: window.innerWidth * 0.4, y: window.innerHeight * 0.4 });
    store().addNodeAt({ x: Math.round(p.x / GRID) * GRID, y: Math.round(p.y / GRID) * GRID });
  };

  return (
    <header className="flex h-10 shrink-0 flex-nowrap items-center gap-1 overflow-visible whitespace-nowrap border-b border-[#3c3c3c] bg-[#252526] px-2 text-[13px]">
      <span className="mr-1 hidden shrink-0 font-semibold text-[#3b82f6] min-[1000px]:inline">Claude Graph</span>
      <input
        className="w-40 min-w-[5rem] shrink rounded bg-transparent px-1 py-0.5 outline-none hover:bg-[#1e1e1e] focus:bg-[#1e1e1e]"
        value={name}
        aria-label="graph name"
        onChange={(e) => store().setName(e.target.value)}
      />
      <span title={dirty ? 'unsaved changes' : 'saved'} className={`h-2 w-2 shrink-0 rounded-full ${dirty ? 'bg-[#f59e0b]' : 'bg-transparent'}`} />
      <FileMenu />
      <BarButton icon={Plus} label="Add node" title="Add node (Ctrl+N, or double-click the canvas)" onClick={addNode} />
      <div className="mx-1 h-5 w-px shrink-0 bg-[#3c3c3c]" />
      <RunSplitButton />
      {isRunActive(runState) && (
        <BarButton icon={Square} label="Cancel" title="Cancel the run (Esc)" className="text-[#ef4444]" onClick={() => void store().cancelRun()} />
      )}
      <RunChip onOpenSettings={() => setSettingsOpen(true)} />
      <div className="hidden min-w-0 shrink min-[1100px]:flex">
        <SessionChips />
      </div>
      <SessionsDropdown count={sessionCount}>
        <SessionChips vertical />
      </SessionsDropdown>
      <AgentRunChip />
      <div className="ml-auto flex shrink-0 items-center gap-2">
        {MOCK_MODE && <span className="rounded bg-[#f59e0b]/15 px-1.5 text-[11px] text-[#f59e0b]" title="No Tauri backend: in-memory mock">mock</span>}
        <NeedsYouBadge />
        <ApprovalsBadge />
        <AssistantChip onOpen={onOpenAssistant} />
        <HarnessChip />
        <InboxButton />
        <NotificationBell />
        <button className={`${btnCls} ${logVisible ? 'bg-[#37373d]' : ''}`} onClick={onToggleLog} title="Show/hide the log panel (Ctrl+J)" aria-label="show or hide the log panel" aria-pressed={logVisible}>
          <PanelBottom size={14} />
        </button>
        <div className="relative" ref={settingsRef}>
          <button
            className={`${btnCls} ${settingsOpen ? 'bg-[#37373d]' : ''}`}
            onClick={() => setSettingsOpen((o) => !o)}
            title="Settings"
            aria-label="settings"
            aria-expanded={settingsOpen}
          >
            <SettingsIcon size={14} />
          </button>
          {settingsOpen && <SettingsPopover />}
        </div>
      </div>
    </header>
  );
}
