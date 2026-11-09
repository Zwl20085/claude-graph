/** The backend's clamping of the inbox numbers (src-tauri/src/inbox/settings.rs). No imports of the IPC layer. */
import { INBOX_LIMITS, type InboxSettings } from '../types/contract';

const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, Math.round(n)));

export function clampInbox(s: InboxSettings): InboxSettings {
  return {
    enabled: s.enabled,
    folder: s.folder.trim(),
    interval_hours: clamp(s.interval_hours, INBOX_LIMITS.interval_hours.min, INBOX_LIMITS.interval_hours.max),
    max_parallel: clamp(s.max_parallel, INBOX_LIMITS.max_parallel.min, INBOX_LIMITS.max_parallel.max),
    timeout_minutes: clamp(s.timeout_minutes, INBOX_LIMITS.timeout_minutes.min, INBOX_LIMITS.timeout_minutes.max),
  };
}
