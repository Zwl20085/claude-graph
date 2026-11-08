/**
 * Global keyboard shortcuts of the main window and the `?` sheet that lists
 * them: Ctrl+S save, Ctrl+Enter Run all, Esc cancel the run (after a
 * confirm), Ctrl+N add a node, Ctrl+J toggle the log, `?` this sheet.
 * Keys typed into a text field are left alone (except Ctrl+S).
 */
import { useEffect } from 'react';
import { X } from 'lucide-react';
import { isRunActive } from '../lib/status';
import { graphStore } from '../store/graphStore';
import { btnCls } from './ui';

export const SHORTCUTS: ReadonlyArray<[string, string]> = [
  ['Ctrl+S', 'Save'],
  ['Ctrl+Enter', 'Run all'],
  ['Esc', 'Cancel the run (asks first)'],
  ['Ctrl+N', 'Add a node'],
  ['Ctrl+J', 'Show or hide the log'],
  ['Delete', 'Delete the selected node or wire'],
  ['Ctrl+Enter (in a composer)', 'Send the message'],
  ['?', 'This list'],
];

export const CANCEL_CONFIRM = 'Cancel the running graph? Running sessions are stopped.';

function inField(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  return el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName) || el.closest('.xterm') !== null;
}

export interface ShortcutHandlers {
  addNode: () => void;
  toggleLog: () => void;
  showSheet: () => void;
  /** Asks the user; true = go ahead. */
  confirm?: (text: string) => boolean;
}

export function handleShortcut(e: KeyboardEvent, h: ShortcutHandlers): boolean {
  const store = graphStore().getState();
  const ctrl = e.ctrlKey || e.metaKey;
  const key = e.key.toLowerCase();
  if (ctrl && key === 's') {
    void store.save();
    return true;
  }
  if (inField(e.target) || store.dir === null) return false;
  if (ctrl && e.key === 'Enter') {
    if (!isRunActive(store.runState) && !store.startingRun) void store.startRun('all');
    return true;
  }
  if (ctrl && key === 'n') {
    h.addNode();
    return true;
  }
  if (ctrl && key === 'j') {
    h.toggleLog();
    return true;
  }
  if (!ctrl && e.key === '?') {
    h.showSheet();
    return true;
  }
  if (!ctrl && e.key === 'Escape' && isRunActive(store.runState)) {
    const ask = h.confirm ?? ((t: string) => window.confirm(t));
    if (ask(CANCEL_CONFIRM)) void store.cancelRun();
    return true;
  }
  return false;
}

export function useShortcuts(h: ShortcutHandlers): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (handleShortcut(e, h)) e.preventDefault();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [h]);
}

export function ShortcutSheet({ onClose }: { onClose: () => void }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={onClose}>
      <div
        role="dialog"
        aria-label="Keyboard shortcuts"
        className="w-[380px] rounded border border-[#3c3c3c] bg-[#252526] p-3 text-[12px] shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-2 flex items-center">
          <h2 className="flex-1 text-[13px] font-semibold">Keyboard shortcuts</h2>
          <button className={btnCls} aria-label="close shortcuts" onClick={onClose}>
            <X size={13} />
          </button>
        </div>
        <table className="w-full">
          <tbody>
            {SHORTCUTS.map(([keys, what]) => (
              <tr key={keys}>
                <td className="py-0.5 pr-3 font-mono text-[#d4d4d4]">{keys}</td>
                <td className="py-0.5 text-[#9a9a9a]">{what}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
