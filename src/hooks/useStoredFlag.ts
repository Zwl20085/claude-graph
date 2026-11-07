import { useCallback, useState } from 'react';

function read(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : v === '1';
  } catch {
    return fallback;
  }
}

/** A boolean UI preference remembered in localStorage (falls back silently when storage is blocked). */
export function useStoredFlag(key: string, fallback: boolean): [boolean, (next: boolean) => void] {
  const [value, setValue] = useState(() => read(key, fallback));
  const set = useCallback(
    (next: boolean) => {
      setValue(next);
      try {
        localStorage.setItem(key, next ? '1' : '0');
      } catch (err) {
        console.warn('could not persist preference', key, err);
      }
    },
    [key],
  );
  return [value, set];
}
