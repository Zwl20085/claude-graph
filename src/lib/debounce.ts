/** Trailing-edge debouncer with explicit flush/cancel (used by autosave). */

export interface Timers {
  setTimeout: (fn: () => void, ms: number) => unknown;
  clearTimeout: (handle: unknown) => void;
}

export const defaultTimers: Timers = {
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface Debouncer {
  /** (Re)start the timer; `fn` runs `ms` after the last call. */
  schedule: () => void;
  /** Run `fn` now if a call is pending. Returns whether it ran. */
  flush: () => boolean;
  /** Drop a pending call. */
  cancel: () => void;
  pending: () => boolean;
}

export function createDebouncer(fn: () => void, ms: number, timers: Timers = defaultTimers): Debouncer {
  let handle: unknown = null;

  const cancel = (): void => {
    if (handle !== null) timers.clearTimeout(handle);
    handle = null;
  };

  const fire = (): void => {
    handle = null;
    fn();
  };

  return {
    schedule: () => {
      cancel();
      handle = timers.setTimeout(fire, ms);
    },
    flush: () => {
      if (handle === null) return false;
      cancel();
      fn();
      return true;
    },
    cancel,
    pending: () => handle !== null,
  };
}
