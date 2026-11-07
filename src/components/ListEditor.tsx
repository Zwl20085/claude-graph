/** List editors for do_nots (strings) and deliverables (path + description). */
import { Plus, Trash2 } from 'lucide-react';
import type { Deliverable } from '../types/contract';
import { deliverablePathError } from '../lib/validate';
import { nextRowKey, useDraftRows, type Row } from '../hooks/useDraftRows';
import { btnCls, errorCls, inputCls } from './ui';

const replaceRow = <T,>(rows: readonly Row<T>[], key: string, value: T): Row<T>[] =>
  rows.map((r) => (r.key === key ? { ...r, value } : r));
const removeRow = <T,>(rows: readonly Row<T>[], key: string): Row<T>[] => rows.filter((r) => r.key !== key);

interface StringListProps {
  items: readonly string[];
  onChange: (items: string[]) => void;
  placeholder?: string;
  /** Accessible name of each row's input. */
  itemLabel?: string;
}

/** Empty rows stay local until they have text. */
export function StringListEditor({ items, onChange, placeholder, itemLabel = 'item' }: StringListProps) {
  const [rows, update] = useDraftRows(items, onChange, (v) => v.trim() !== '');
  return (
    <div className="space-y-1">
      {rows.map((row, i) => (
        <div key={row.key} className="flex gap-1">
          <input
            className={inputCls}
            value={row.value}
            placeholder={placeholder}
            aria-label={`${itemLabel} ${i + 1}`}
            onChange={(e) => update(replaceRow(rows, row.key, e.target.value))}
          />
          <button className={btnCls} aria-label={`remove ${itemLabel} ${i + 1}`} onClick={() => update(removeRow(rows, row.key))}>
            <Trash2 size={13} />
          </button>
        </div>
      ))}
      <button className={btnCls} onClick={() => update([...rows, { key: nextRowKey(), value: '' }])}>
        <Plus size={13} />Add
      </button>
    </div>
  );
}

interface DeliverablesProps {
  items: readonly Deliverable[];
  onChange: (items: Deliverable[]) => void;
}

/** Rows with an invalid or empty path show a hint and are not committed. */
export function DeliverablesEditor({ items, onChange }: DeliverablesProps) {
  const [rows, update] = useDraftRows(items, onChange, (d) => deliverablePathError(d.path) === null);
  return (
    <div className="space-y-1">
      {rows.map((row, i) => {
        const d = row.value;
        const error = d.path === '' ? null : deliverablePathError(d.path);
        return (
          <div key={row.key}>
            <div className="flex gap-1">
              <input
                className={`${inputCls} w-[38%] font-mono ${error ? 'border-[#ef4444]' : ''}`}
                value={d.path}
                placeholder="path"
                aria-label={`deliverable ${i + 1} path`}
                aria-invalid={error !== null}
                onChange={(e) => update(replaceRow(rows, row.key, { ...d, path: e.target.value }))}
              />
              <input
                className={inputCls}
                value={d.description}
                placeholder="description"
                aria-label={`deliverable ${i + 1} description`}
                onChange={(e) => update(replaceRow(rows, row.key, { ...d, description: e.target.value }))}
              />
              <button className={btnCls} aria-label={`remove deliverable ${i + 1}`} onClick={() => update(removeRow(rows, row.key))}>
                <Trash2 size={13} />
              </button>
            </div>
            {error && <span className={errorCls}>{error} Not saved until fixed.</span>}
          </div>
        );
      })}
      <button className={btnCls} onClick={() => update([...rows, { key: nextRowKey(), value: { path: '', description: '' } }])}>
        <Plus size={13} />Add deliverable
      </button>
    </div>
  );
}
