/** Small shared form pieces (dark theme). */
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type InputHTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
  type TextareaHTMLAttributes,
} from 'react';
import { X } from 'lucide-react';

/** Input look without a width: combine with an explicit width class (inputCls carries w-full, which would win). */
export const inputBoxCls =
  'rounded border border-[#3c3c3c] bg-[#1e1e1e] px-2 py-1 text-[13px] text-[#d4d4d4] outline-none focus:border-[#3b82f6] disabled:opacity-60';
export const inputCls = `w-full ${inputBoxCls}`;
export const btnCls =
  'inline-flex items-center gap-1 rounded px-2 py-1 text-[12px] text-[#d4d4d4] hover:bg-[#37373d] disabled:cursor-not-allowed disabled:opacity-40';
export const errorCls = 'mt-0.5 block text-[11px] text-[#ef4444]';

interface FieldIds {
  labelId: string;
  describedBy: string | undefined;
}
const FieldContext = createContext<FieldIds | null>(null);

/** aria-labelledby / aria-describedby of the enclosing Field, for its control. */
export function useFieldA11y(): { 'aria-labelledby'?: string; 'aria-describedby'?: string } {
  const ids = useContext(FieldContext);
  return ids ? { 'aria-labelledby': ids.labelId, 'aria-describedby': ids.describedBy } : {};
}

interface FieldProps {
  label: string;
  hint?: string;
  /** Inline validation message (red). */
  error?: string | null;
  /** Several controls (lists, chips): rendered as fieldset + legend. */
  composite?: boolean;
  children: ReactNode;
}

/**
 * A labelled form row. Not a <label> (it may contain buttons): the caption is
 * a span whose id the control references via `useFieldA11y`.
 */
export function Field({ label, hint, error, composite = false, children }: FieldProps) {
  const id = useId();
  const labelId = `${id}-label`;
  const hintId = hint ? `${id}-hint` : undefined;
  const errorId = error ? `${id}-error` : undefined;
  const describedBy = [hintId, errorId].filter(Boolean).join(' ') || undefined;
  const captionCls = 'mb-1 block text-[11px] uppercase tracking-wide text-[#9a9a9a]';
  const tail = (
    <>
      {hint && <span id={hintId} className="mt-0.5 block text-[11px] text-[#8a8a8a]">{hint}</span>}
      {error && <span id={errorId} role="alert" className={errorCls}>{error}</span>}
    </>
  );
  const body = <FieldContext.Provider value={{ labelId, describedBy }}>{children}</FieldContext.Provider>;
  if (composite) {
    return (
      <fieldset className="block min-w-0 border-0 p-0" aria-describedby={describedBy}>
        <legend id={labelId} className={captionCls}>{label}</legend>
        {body}
        {tail}
      </fieldset>
    );
  }
  return (
    <div className="block" role="group" aria-labelledby={labelId}>
      <span id={labelId} className={captionCls}>{label}</span>
      {body}
      {tail}
    </div>
  );
}

/** <input> wired to the enclosing Field's label. */
export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={className ?? inputCls} {...useFieldA11y()} {...rest} />;
}

/** <textarea> wired to the enclosing Field's label. */
export function TextArea({ className, ...rest }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea className={className ?? `${inputCls} resize-y`} {...useFieldA11y()} {...rest} />;
}

interface NumberInputProps {
  value: number | null;
  min: number;
  /** Called with a parsed integer >= min, or null when emptied and `nullable`. */
  onCommit: (value: number | null) => void;
  nullable?: boolean;
  placeholder?: string;
  className?: string;
  'aria-label'?: string;
}

/** Integer input that may be empty while typing; only valid values are committed. */
export function NumberInput({ value, min, onCommit, nullable = false, placeholder, className, ...aria }: NumberInputProps) {
  const [draft, setDraft] = useState(value === null ? '' : String(value));
  useEffect(() => {
    setDraft((d) => {
      const shown = d.trim() === '' ? null : Number.parseInt(d, 10);
      return shown === value ? d : value === null ? '' : String(value);
    });
  }, [value]);
  const a11y = useFieldA11y();
  return (
    <input
      type="number"
      min={min}
      className={className ?? inputCls}
      value={draft}
      placeholder={placeholder}
      {...a11y}
      {...aria}
      onChange={(e) => {
        const raw = e.target.value;
        setDraft(raw);
        if (raw.trim() === '') {
          if (nullable) onCommit(null);
          return;
        }
        const n = Number.parseInt(raw, 10);
        if (Number.isInteger(n) && n >= min) onCommit(n);
      }}
      onBlur={() => setDraft(value === null ? '' : String(value))}
    />
  );
}

interface SelectProps<T extends string> {
  value: T;
  options: readonly T[];
  onChange: (value: T) => void;
  title?: string;
  /** Display text of an option (default: the value). */
  label?: (value: T) => string;
  disabled?: boolean;
}

export function Select<T extends string>({ value, options, onChange, title, label, disabled }: SelectProps<T>) {
  return (
    <select className={inputCls} value={value} title={title} disabled={disabled} {...useFieldA11y()} onChange={(e) => onChange(e.target.value as T)}>
      {options.map((o) => (
        <option key={o} value={o}>
          {label ? label(o) : o}
        </option>
      ))}
    </select>
  );
}

export const RESOURCE_SUGGESTIONS = ['cpu', 'memory', 'gpu', 'exclusive'] as const;

/** Resource tag rule: trimmed, non-empty, no whitespace inside. Returns an error or null. */
export function tagError(tag: string): string | null {
  if (tag === '') return 'Tag is empty.';
  return /\s/.test(tag) ? 'Tags cannot contain spaces.' : null;
}

interface ChipsProps {
  values: readonly string[];
  onChange: (values: string[]) => void;
  suggestions?: readonly string[];
}

/** Tag chips with free-text add (Enter or comma) and one-click suggestions. */
export function ChipsInput({ values, onChange, suggestions = RESOURCE_SUGGESTIONS }: ChipsProps) {
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const a11y = useFieldA11y();
  const add = (raw: string): void => {
    const tag = raw.trim();
    if (tag === '') return setDraft('');
    const err = tagError(tag);
    setError(err);
    if (err) return setDraft(raw);
    if (!values.includes(tag)) onChange([...values, tag]);
    setDraft('');
  };
  return (
    <div>
      <div className="flex flex-wrap items-center gap-1 rounded border border-[#3c3c3c] bg-[#1e1e1e] p-1">
        {values.map((v) => (
          <span key={v} className="inline-flex items-center gap-0.5 rounded-full bg-[#37373d] px-2 py-0.5 text-[11px]">
            {v}
            <button type="button" aria-label={`remove tag ${v}`} onClick={() => onChange(values.filter((x) => x !== v))}>
              <X size={10} />
            </button>
          </span>
        ))}
        <input
          className="min-w-[60px] flex-1 bg-transparent px-1 text-[12px] outline-none"
          value={draft}
          placeholder="add tag"
          {...a11y}
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
            <button
              key={s}
              type="button"
              aria-label={`add tag ${s}`}
              className="rounded-full border border-dashed border-[#555] px-1.5 text-[10px] text-[#9a9a9a] hover:text-[#d4d4d4]"
              onClick={() => add(s)}
            >
              +{s}
            </button>
          ))}
      </div>
      {error && <span className={errorCls}>{error}</span>}
    </div>
  );
}

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Keep Tab / Shift+Tab inside `root`. */
function trapTab(e: KeyboardEvent<HTMLElement>, root: HTMLElement | null): void {
  if (e.key !== 'Tab' || !root) return;
  const items = [...root.querySelectorAll<HTMLElement>(FOCUSABLE)];
  const first = items[0];
  const last = items[items.length - 1];
  if (!first || !last) return;
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
}

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** Narrow dialog (confirmations). */
  small?: boolean;
}

/** Modal dialog: role=dialog, Escape closes, focus moves in, Tab stays in, focus is restored on close. */
export function Modal({ title, onClose, children, small = false }: ModalProps) {
  const titleId = useId();
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const first = box.current?.querySelector<HTMLElement>('[data-autofocus]') ?? box.current?.querySelector<HTMLElement>(FOCUSABLE);
    first?.focus();
    return () => previous?.focus?.();
  }, []);
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onMouseDown={onClose}>
      <div
        ref={box}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className={`flex max-h-[85vh] flex-col rounded border border-[#3c3c3c] bg-[#252526] shadow-xl ${small ? 'w-[min(560px,92vw)]' : 'w-[min(900px,92vw)]'}`}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.stopPropagation();
            onClose();
          } else {
            trapTab(e, box.current);
          }
        }}
      >
        <div className="flex items-center border-b border-[#3c3c3c] px-3 py-2 text-[13px]">
          <span id={titleId} className="font-semibold">{title}</span>
          <button className={`${btnCls} ml-auto`} onClick={onClose} aria-label="close dialog">
            <X size={14} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3">{children}</div>
      </div>
    </div>
  );
}
