/**
 * Local rows of a list editor with stable keys. Only rows that pass `keep`
 * are committed, so a half-typed or invalid row never reaches the graph (and
 * never blocks autosave). The rows re-sync when the committed list changes
 * from outside (another node, a reload).
 */
import { useEffect, useRef, useState } from 'react';

export interface Row<T> {
  key: string;
  value: T;
}

let keySeq = 0;
export const nextRowKey = (): string => {
  keySeq += 1;
  return `row-${keySeq}`;
};

const same = (a: unknown, b: unknown): boolean => JSON.stringify(a) === JSON.stringify(b);
const toRows = <T,>(items: readonly T[]): Row<T>[] => items.map((value) => ({ key: nextRowKey(), value }));

export function useDraftRows<T>(
  items: readonly T[],
  onChange: (items: T[]) => void,
  keep: (value: T) => boolean,
): [Row<T>[], (rows: Row<T>[]) => void] {
  const [rows, setRows] = useState<Row<T>[]>(() => toRows(items));
  const committed = useRef<readonly T[]>(items);

  useEffect(() => {
    if (same(items, committed.current)) return;
    committed.current = items;
    setRows(toRows(items));
  }, [items]);

  const update = (next: Row<T>[]): void => {
    setRows(next);
    const values = next.filter((r) => keep(r.value)).map((r) => r.value);
    if (same(values, committed.current)) return;
    committed.current = values;
    onChange(values);
  };
  return [rows, update];
}
