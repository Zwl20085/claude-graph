/**
 * Window routing on `location.hash`. The main window has no hash; a session
 * window is opened at `#/terminal/<run>/<sid>` (optionally `?dir=<graph dir>`,
 * used for list_sessions when present).
 */
import { useEffect, useState } from 'react';

export type HashRoute = { kind: 'main' } | { kind: 'terminal'; runId: string; sid: string; dir: string | null };

const SEGMENT_RE = /^[A-Za-z0-9._-]{1,128}$/;
const TERMINAL_RE = /^#\/terminal\/([^/?]+)\/([^/?]+)\/?(?:\?(.*))?$/;

function decode(segment: string): string | null {
  try {
    const text = decodeURIComponent(segment);
    return SEGMENT_RE.test(text) ? text : null;
  } catch {
    return null;
  }
}

function dirOf(query: string | undefined): string | null {
  if (!query) return null;
  try {
    const dir = new URLSearchParams(query).get('dir');
    return dir === null || dir.trim() === '' ? null : dir;
  } catch {
    return null;
  }
}

/** Route of a hash; anything that is not a well-formed terminal route is the main window. */
export function parseHashRoute(hash: string): HashRoute {
  const m = TERMINAL_RE.exec(hash);
  if (!m) return { kind: 'main' };
  const runId = decode(m[1] ?? '');
  const sid = decode(m[2] ?? '');
  if (runId === null || sid === null) return { kind: 'main' };
  return { kind: 'terminal', runId, sid, dir: dirOf(m[3]) };
}

const currentHash = (): string => (typeof window === 'undefined' ? '' : window.location.hash);

/** The route of this window, updated on `hashchange`. */
export function useHashRoute(): HashRoute {
  const [hash, setHash] = useState(currentHash);
  useEffect(() => {
    const onChange = (): void => setHash(currentHash());
    window.addEventListener('hashchange', onChange);
    return () => window.removeEventListener('hashchange', onChange);
  }, []);
  return parseHashRoute(hash);
}
