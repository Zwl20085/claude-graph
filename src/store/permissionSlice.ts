/**
 * "Ask me in the app" slice of the graph store (ASKAPP_CONTRACT): the queue
 * of pending permission prompts, oldest first. Filled by
 * `list_permission_prompts` on app start and kept current by the
 * `permission_prompt` / `permission_answered` events. Every update builds a
 * new array. Failures become toasts.
 */
import {
  isAppError,
  isPermissionPrompt,
  validPermissionPrompts,
  type PermissionAnswer,
  type PermissionPrompt,
  type RunState,
} from '../types/contract';
import type { Api } from '../lib/api';
import { errorMessage } from './runHelpers';

/** How many resolved ids are remembered so a late list reply cannot resurrect them. */
export const RESOLVED_MEMORY = 256;

export interface PermissionState {
  /** Pending prompts, oldest first. */
  permissionPrompts: readonly PermissionPrompt[];
  /** The user hid the dialog ("Decide later"); a new prompt or the top-bar badge shows it again. */
  permissionDialogHidden: boolean;
  /** Id of the prompt whose answer is in flight (buttons disabled); null when none. */
  permissionAnswering: string | null;
}

export interface PermissionActions {
  loadPermissionPrompts: () => Promise<void>;
  /** A `permission_prompt` event (validated here). */
  addPermissionPrompt: (prompt: unknown) => void;
  /** A `permission_answered` event, or a local answer that went through. */
  removePermissionPrompt: (id: string) => void;
  /** Returns true when the backend took the answer (or the prompt was already gone). */
  answerPermission: (answer: PermissionAnswer) => Promise<boolean>;
  showPermissionDialog: () => void;
  hidePermissionDialog: () => void;
}

export const INITIAL_PERMISSIONS: PermissionState = {
  permissionPrompts: [],
  permissionDialogHidden: false,
  permissionAnswering: null,
};

type Toast = (kind: 'error' | 'info', text: string) => void;
type Get = () => PermissionState;
type SetPatch = (patch: Partial<PermissionState>) => void;

/** `list` merged with what arrived meanwhile: list order first, then newer event-only prompts. */
export function mergePrompts(
  current: readonly PermissionPrompt[],
  listed: readonly PermissionPrompt[],
  resolved: ReadonlySet<string>,
): PermissionPrompt[] {
  const fromList = listed.filter((p) => !resolved.has(p.id));
  const ids = new Set(fromList.map((p) => p.id));
  return [...fromList, ...current.filter((p) => !ids.has(p.id) && !resolved.has(p.id))];
}

/** Node ids of `run` with a pending prompt (the "waiting for you" card footer). */
export function waitingNodeIds(prompts: readonly PermissionPrompt[], run: RunState | null): ReadonlySet<string> {
  if (run === null) return new Set();
  return new Set(prompts.filter((p) => p.run_id === run.run_id).map((p) => p.node_id));
}

export function approvalsText(n: number): string {
  return `${n} approval${n === 1 ? '' : 's'} needed`;
}

export function createPermissionActions(api: Api, set: SetPatch, get: Get, toast: Toast): PermissionActions {
  let resolved: readonly string[] = [];
  const remember = (id: string): void => {
    resolved = [...resolved.filter((x) => x !== id), id].slice(-RESOLVED_MEMORY);
  };

  const remove = (id: string): void => {
    remember(id);
    const prompts = get().permissionPrompts;
    if (!prompts.some((p) => p.id === id)) return;
    set({ permissionPrompts: prompts.filter((p) => p.id !== id) });
  };

  return {
    loadPermissionPrompts: async () => {
      try {
        const listed = validPermissionPrompts(await api.listPermissionPrompts());
        set({ permissionPrompts: mergePrompts(get().permissionPrompts, listed, new Set(resolved)) });
      } catch (err) {
        toast('error', `Could not load pending approvals: ${errorMessage(err)}`);
      }
    },
    addPermissionPrompt: (prompt) => {
      if (!isPermissionPrompt(prompt)) {
        console.warn('ignored a malformed permission_prompt payload', prompt);
        return;
      }
      const prompts = get().permissionPrompts;
      if (resolved.includes(prompt.id) || prompts.some((p) => p.id === prompt.id)) return;
      const clean: PermissionPrompt = { ...prompt, suggestions: Array.isArray(prompt.suggestions) ? [...prompt.suggestions] : [] };
      set({ permissionPrompts: [...prompts, clean], permissionDialogHidden: false });
    },
    removePermissionPrompt: remove,
    answerPermission: async (answer) => {
      if (get().permissionAnswering !== null) return false;
      set({ permissionAnswering: answer.id });
      try {
        await api.answerPermission(answer);
        remove(answer.id);
        return true;
      } catch (err) {
        if (isAppError(err) && err.code === 'not_found') {
          remove(answer.id);
          toast('info', 'That approval was already answered.');
          return true;
        }
        toast('error', `Could not send your answer: ${errorMessage(err)}`);
        return false;
      } finally {
        set({ permissionAnswering: null });
      }
    },
    showPermissionDialog: () => set({ permissionDialogHidden: false }),
    hidePermissionDialog: () => set({ permissionDialogHidden: true }),
  };
}
