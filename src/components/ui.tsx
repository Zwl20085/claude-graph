/** Small shared form pieces (dark theme). */
import { useState, type ReactNode } from 'react';
import { X } from 'lucide-react';

export const inputCls =
  'w-full rounded border border-[#3c3c3c] bg-[#1e1e1e] px-2 py-1 text-[13px] text-[#d4d4d4] outline-none focus:border-[#3b82f6] disabled:opacity-60';
export const btnCls =
  'inline-flex items-center gap-1 rounded px-2 py-1 text-[12px] text-[#d4d4d4] hover:bg-[#37373d] disabled:cursor-not-allowed disabled:opacity-40';

interface FieldProps {
  label: string;
  hint?: string;
  children: ReactNode;
}

export function Field({ label, hint, children }: FieldProps) {
  return (
    <label className="block">
      <span className="mb-1 block text-[11px] uppercase tracking-wide text-[#9a9a9a]">{label}</span>
      {children}
      {hint && <span className="mt-0.5 block text-[11px] text-[#8a8a8a]">{hint}</span>}
    </label>
  );
}

interface SelectProps<T extends string> {
  value: T;
  options: readonly T[];
  onChange: (value: T) => void;
  title?: string;
}

export function Select<T extends string>({ value, options, onChange, title }: SelectProps<T>) {
  return (
    <select className={inputCls} value={value} title={title} onChange={(e) => onChange(e.target.value as T)}>
      {options.map((o) => (
        <option key={o} value={o}>
          {o}
        </option>
      ))}
    </select>
  );
}

export const RESOURCE_SUGGESTIONS = ['cpu', 'memory', 'gpu', 'exclusive'] as const;

interface ChipsProps {
  values: readonly string[];
  onChange: (values: string[]) => void;
  suggestions?: readonly string[];
}

/** Tag chips with free-text add (Enter or comma) and one-click suggestions. */
export function ChipsInput({ values, onChange, suggestions = RESOURCE_SUGGESTIONS }: ChipsProps) {
  const [draft, setDraft] = useState('');
  const add = (raw: string): void => {
    const tag = raw.trim();
    if (tag !== '' && !values.includes(tag)) onChange([...values, tag]);
    setDraft('');
  };
  return (
    <div className="flex flex-wrap items-center gap-1 rounded border border-[#3c3c3c] bg-[#1e1e1e] p-1">
      {values.map((v) => (
        <span key={v} className="inline-flex items-center gap-0.5 rounded-full bg-[#37373d] px-2 py-0.5 text-[11px]">
          {v}
          <button type="button" aria-label={`remove ${v}`} onClick={() => onChange(values.filter((x) => x !== v))}>
            <X size={10} />
          </button>
        </span>
      ))}
      <input
        className="min-w-[60px] flex-1 bg-transparent px-1 text-[12px] outline-none"
        value={draft}
        placeholder="add tag"
        onChange={(e) => (e.target.value.endsWith(',') ? add(e.target.value.slice(0, -1)) : setDraft(e.target.value))}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            add(draft);
          } else if (e.key === 'Backspace' && draft === '' && values.length > 0) {
            onChange(values.slice(0, -1));
          }
        }}
      />
      {suggestions
        .filter((s) => !values.includes(s))
        .map((s) => (
          <button key={s} type="button" className="rounded-full border border-dashed border-[#555] px-1.5 text-[10px] text-[#9a9a9a] hover:text-[#d4d4d4]" onClick={() => add(s)}>
            +{s}
          </button>
        ))}
    </div>
  );
}

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
}

export function Modal({ title, onClose, children }: ModalProps) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onMouseDown={onClose}>
      <div
        className="flex max-h-[85vh] w-[min(900px,92vw)] flex-col rounded border border-[#3c3c3c] bg-[#252526] shadow-xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center border-b border-[#3c3c3c] px-3 py-2 text-[13px]">
          <span className="font-semibold">{title}</span>
          <button className={`${btnCls} ml-auto`} onClick={onClose} aria-label="close">
            <X size={14} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3">{children}</div>
      </div>
    </div>
  );
}
