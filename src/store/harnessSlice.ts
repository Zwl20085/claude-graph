/**
 * Harness profiles slice of the graph store: the machine's profile list,
 * loaded on app start and replaced by the list every save / delete returns.
 * Failures become error toasts; save also returns its message for inline use.
 */
import { HARNESS_KINDS, type HarnessCheck, type HarnessProfile } from '../types/contract';
import type { Api } from '../lib/api';
import { BUILTIN_HARNESSES } from '../lib/harness';
import { errorMessage } from './runHelpers';

export interface HarnessState {
  /** Built-ins until `list_harnesses` answers, so labels work from the first paint. */
  harnesses: readonly HarnessProfile[];
}

export interface HarnessActions {
  loadHarnesses: () => Promise<void>;
  /** Returns null on success, else the backend's message (also toasted). */
  saveHarness: (profile: HarnessProfile) => Promise<string | null>;
  deleteHarness: (id: string) => Promise<boolean>;
  /** null when the check itself could not run (toasted). */
  checkHarness: (id: string) => Promise<HarnessCheck | null>;
}

type Toast = (kind: 'error' | 'success', text: string) => void;

/** Shape check of one profile from the backend. */
export function isHarnessProfile(v: unknown): v is HarnessProfile {
  if (typeof v !== 'object' || v === null) return false;
  const p = v as Record<string, unknown>;
  return (
    typeof p.id === 'string' &&
    typeof p.label === 'string' &&
    (HARNESS_KINDS as readonly unknown[]).includes(p.kind) &&
    Array.isArray(p.args) &&
    typeof p.env === 'object' &&
    p.env !== null
  );
}

/** The well-formed profiles of a backend list (others dropped with a warning). */
export function validProfiles(list: unknown): HarnessProfile[] {
  if (!Array.isArray(list)) {
    console.warn('ignored a malformed harness list', list);
    return [];
  }
  const ok = list.filter(isHarnessProfile);
  if (ok.length !== list.length) console.warn('ignored malformed harness profiles', list.filter((p) => !isHarnessProfile(p)));
  return ok;
}

export const INITIAL_HARNESSES: HarnessState = { harnesses: BUILTIN_HARNESSES };

export function createHarnessActions(api: Api, set: (patch: Partial<HarnessState>) => void, toast: Toast): HarnessActions {
  return {
    loadHarnesses: async () => {
      try {
        const list = validProfiles(await api.listHarnesses());
        if (list.length > 0) set({ harnesses: list });
      } catch (err) {
        toast('error', `Could not load harness profiles: ${errorMessage(err)}`);
      }
    },
    saveHarness: async (profile) => {
      try {
        const list = validProfiles(await api.saveHarness(profile));
        if (list.length > 0) set({ harnesses: list });
        toast('success', `Saved harness ${profile.label}.`);
        return null;
      } catch (err) {
        const message = errorMessage(err);
        toast('error', `Could not save harness ${profile.id}: ${message}`);
        return message;
      }
    },
    deleteHarness: async (id) => {
      try {
        const list = validProfiles(await api.deleteHarness(id));
        if (list.length > 0) set({ harnesses: list });
        return true;
      } catch (err) {
        toast('error', `Could not delete harness ${id}: ${errorMessage(err)}`);
        return false;
      }
    },
    checkHarness: async (id) => {
      try {
        return await api.checkHarness(id);
      } catch (err) {
        toast('error', `Could not check harness ${id}: ${errorMessage(err)}`);
        return null;
      }
    },
  };
}
