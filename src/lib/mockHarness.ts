/**
 * Harness profiles behind the mock IPC: the three built-ins plus saved
 * overrides and custom profiles, held in memory. Mirrors the backend rules of
 * HARNESS_CONTRACT: built-ins keep id, label and kind and cannot be deleted;
 * save validates like `save_harness`; check simulates `<command> --version`.
 */
import type { AppError, Graph, HarnessCheck, HarnessProfile } from '../types/contract';
import { effectiveHarness } from '../types/contract';
import { BUILTIN_HARNESSES, findHarness } from './harness';
import { harnessProfileError } from './validate';

const appError = (code: string, message: string): AppError => ({ code, message });
const copy = (p: HarnessProfile): HarnessProfile => ({ ...p, args: [...p.args], env: { ...p.env }, dialog_markers: [...p.dialog_markers] });

/** A command containing this text fails the mock check (tests, browser dev). */
export const MOCK_HARNESS_FAIL = 'FAKE_MISSING';

export interface MockHarnesses {
  list: () => HarnessProfile[];
  save: (profile: HarnessProfile) => HarnessProfile[];
  remove: (id: string) => HarnessProfile[];
  check: (id: string) => HarnessCheck;
  /** Label of an id, for session titles (unknown ids show as themselves). */
  label: (id: string) => string;
  /** Throws invalid_input naming the first node whose harness is not defined. */
  assertKnown: (graph: Graph, ids: readonly string[]) => void;
}

export function createMockHarnesses(): MockHarnesses {
  /** Overrides of built-ins (by id) and custom profiles, in save order. */
  let saved: HarnessProfile[] = [];

  const list = (): HarnessProfile[] => {
    const builtins = BUILTIN_HARNESSES.map((b) => {
      const o = saved.find((p) => p.id === b.id);
      return o ? { ...b, command: o.command, args: o.args, env: o.env, model: o.model, dsh_profile: o.dsh_profile ?? null } : b;
    });
    const customs = saved.filter((p) => !BUILTIN_HARNESSES.some((b) => b.id === p.id));
    return [...builtins, ...customs].map(copy);
  };

  const label = (id: string): string => findHarness(list(), id)?.label ?? id;

  return {
    list,
    label,
    save: (profile) => {
      const builtin = BUILTIN_HARNESSES.find((b) => b.id === profile.id);
      const next: HarnessProfile = builtin
        ? { ...copy(profile), label: builtin.label, kind: builtin.kind, builtin: true }
        : { ...copy(profile), builtin: false };
      const err = harnessProfileError(next);
      if (err) throw appError('invalid_input', err);
      saved = saved.some((p) => p.id === next.id) ? saved.map((p) => (p.id === next.id ? next : p)) : [...saved, next];
      return list();
    },
    remove: (id) => {
      if (BUILTIN_HARNESSES.some((b) => b.id === id)) throw appError('invalid_input', `${id} is built in and cannot be deleted`);
      if (!saved.some((p) => p.id === id)) throw appError('not_found', `harness ${id} not found`);
      saved = saved.filter((p) => p.id !== id);
      return list();
    },
    check: (id) => {
      const p = findHarness(list(), id);
      if (!p) throw appError('not_found', `harness ${id} not found`);
      const program = p.command ?? p.kind;
      return program.includes(MOCK_HARNESS_FAIL)
        ? { id, ok: false, version: null, message: `${program} was not found (mock)` }
        : { id, ok: true, version: `${program} 1.0.0 (mock)`, message: null };
    },
    assertKnown: (graph, ids) => {
      const known = new Set(list().map((p) => p.id));
      const bad = graph.nodes.find((n) => ids.includes(n.id) && !known.has(effectiveHarness(n, graph.settings)));
      if (!bad) return;
      const id = effectiveHarness(bad, graph.settings);
      throw appError(
        'invalid_input',
        `node ${bad.id}: harness ${id} is not defined on this machine; add it under Settings → Harnesses`,
      );
    },
  };
}
