/**
 * Harness profiles slice of the graph store: the machine's profile list,
 * loaded on app start and replaced by the list every save / delete returns.
 * Failures become error toasts; save also returns its message for inline use.
 */
import type { HarnessCheck, HarnessProfile } from '../types/contract';
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

export const INITIAL_HARNESSES: HarnessState = { harnesses: BUILTIN_HARNESSES };

export function createHarnessActions(api: Api, set: (patch: Partial<HarnessState>) => void, toast: Toast): HarnessActions {
  return {
    loadHarnesses: async () => {
      try {
        set({ harnesses: await api.listHarnesses() });
      } catch (err) {
        toast('error', `Could not load harness profiles: ${errorMessage(err)}`);
      }
    },
    saveHarness: async (profile) => {
      try {
        set({ harnesses: await api.saveHarness(profile) });
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
        set({ harnesses: await api.deleteHarness(id) });
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
