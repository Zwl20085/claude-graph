/** List editors for do_nots (strings) and deliverables (path + description). */
import { Plus, Trash2 } from 'lucide-react';
import type { Deliverable } from '../types/contract';
import { btnCls, inputCls } from './ui';

const replaceAt = <T,>(items: readonly T[], i: number, value: T): T[] => items.map((x, j) => (j === i ? value : x));
const removeAt = <T,>(items: readonly T[], i: number): T[] => items.filter((_, j) => j !== i);

interface StringListProps {
  items: readonly string[];
  onChange: (items: string[]) => void;
  placeholder?: string;
}

export function StringListEditor({ items, onChange, placeholder }: StringListProps) {
  return (
    <div className="space-y-1">
      {items.map((item, i) => (
        <div key={i} className="flex gap-1">
          <input className={inputCls} value={item} placeholder={placeholder} onChange={(e) => onChange(replaceAt(items, i, e.target.value))} />
          <button className={btnCls} aria-label="remove" onClick={() => onChange(removeAt(items, i))}>
            <Trash2 size={13} />
          </button>
        </div>
      ))}
      <button className={btnCls} onClick={() => onChange([...items, ''])}>
        <Plus size={13} />Add
      </button>
    </div>
  );
}

interface DeliverablesProps {
  items: readonly Deliverable[];
  onChange: (items: Deliverable[]) => void;
}

export function DeliverablesEditor({ items, onChange }: DeliverablesProps) {
  return (
    <div className="space-y-1">
      {items.map((d, i) => (
        <div key={i} className="flex gap-1">
          <input
            className={`${inputCls} w-[38%] font-mono`}
            value={d.path}
            placeholder="path"
            onChange={(e) => onChange(replaceAt(items, i, { ...d, path: e.target.value }))}
          />
          <input
            className={inputCls}
            value={d.description}
            placeholder="description"
            onChange={(e) => onChange(replaceAt(items, i, { ...d, description: e.target.value }))}
          />
          <button className={btnCls} aria-label="remove" onClick={() => onChange(removeAt(items, i))}>
            <Trash2 size={13} />
          </button>
        </div>
      ))}
      <button className={btnCls} onClick={() => onChange([...items, { path: '', description: '' }])}>
        <Plus size={13} />Add deliverable
      </button>
    </div>
  );
}
