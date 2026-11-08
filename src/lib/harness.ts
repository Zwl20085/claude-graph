/**
 * Harness profiles (HARNESS_CONTRACT): the four built-ins, label lookup, and
 * the text <-> profile mapping of the edit form (args / markers one per line,
 * env `KEY=value` one per line). Pure; no store, no IPC.
 */
import { type HarnessKind, type HarnessProfile, type StopMode } from '../types/contract';
import { envKeyError } from './validate';

export const DEFAULT_QUIET_MS = 8000;

export const HARNESS_KIND_LABELS: Record<HarnessKind, string> = {
  claude: 'Claude Code',
  codex: 'Codex CLI',
  custom: 'custom',
  'deepseek-harness': 'DeepSeek Harness (dsh)',
};

/** Session chip / window text of a dsh session: headless, one process per message. */
export const DSH_MODE_LABEL = 'dsh (headless, chained)';
export const DSH_NOTE =
  'dsh has no interactive terminal: each message runs `dsh --profile <p> --json` once, and later messages continue the same dsh session (--session-id). The app passes no model or permission flags; set them in ~/.dsh.';

export const STOP_MODE_LABELS: Record<StopMode, string> = {
  quiet: 'output goes quiet',
  claude_hooks: 'Claude Code hooks',
  codex_notify: 'Codex notify',
};

export const SECRETS_NOTE =
  'The app stores no secrets. Write ${VAR} in a value and it is read from the environment when the session starts.';

const DEEPSEEK_MODEL = 'deepseek-flash';

const base = (p: Pick<HarnessProfile, 'id' | 'label' | 'kind'> & Partial<HarnessProfile>): HarnessProfile => ({
  command: null,
  args: [],
  env: {},
  model: null,
  builtin: true,
  idle_marker: null,
  dialog_markers: [],
  stop_mode: 'quiet',
  quiet_ms: null,
  option_pattern: null,
  dialog_hints: [],
  dsh_profile: null,
  ...p,
});

/** Same as the backend's `harness::builtins()`. */
export const BUILTIN_HARNESSES: readonly HarnessProfile[] = [
  base({ id: 'claude', label: 'Claude Code', kind: 'claude' }),
  base({
    id: 'claude-deepseek',
    label: 'Claude Code via DeepSeek API',
    kind: 'claude',
    env: {
      ANTHROPIC_BASE_URL: 'https://api.deepseek.com/anthropic',
      ANTHROPIC_AUTH_TOKEN: '${DEEPSEEK_API_KEY}',
      ANTHROPIC_MODEL: `${DEEPSEEK_MODEL}[1m]`,
      ANTHROPIC_DEFAULT_OPUS_MODEL: `${DEEPSEEK_MODEL}[1m]`,
      ANTHROPIC_DEFAULT_SONNET_MODEL: `${DEEPSEEK_MODEL}[1m]`,
      ANTHROPIC_DEFAULT_HAIKU_MODEL: DEEPSEEK_MODEL,
      CLAUDE_CODE_SUBAGENT_MODEL: DEEPSEEK_MODEL,
    },
  }),
  base({ id: 'codex', label: 'Codex CLI', kind: 'codex', args: ['--no-alt-screen'] }),
  base({ id: 'deepseek-harness', label: 'DeepSeek Harness (dsh)', kind: 'deepseek-harness' }),
];

/** The harness `id` runs dsh (chained headless). */
export function isDshHarness(profiles: readonly HarnessProfile[], id: string): boolean {
  return findHarness(profiles, id)?.kind === 'deepseek-harness';
}

export function findHarness(profiles: readonly HarnessProfile[], id: string): HarnessProfile | null {
  return profiles.find((p) => p.id === id) ?? null;
}

/** Label of a harness id (built-in labels as fallback); an id not defined anywhere shows as itself. */
export function harnessLabel(profiles: readonly HarnessProfile[], id: string): string {
  return findHarness(profiles, id)?.label ?? findHarness(BUILTIN_HARNESSES, id)?.label ?? id;
}

/**
 * Whether node-level Claude model names apply. Unknown ids count as claude
 * (the run fails at start anyway, and greying the select would hide the value).
 */
export function isClaudeKind(profiles: readonly HarnessProfile[], id: string): boolean {
  return (findHarness(profiles, id)?.kind ?? 'claude') === 'claude';
}

export const NOT_CLAUDE_MODEL_NOTE = 'model is set in the harness profile';

/** A fresh custom profile for the "New" button. */
export function newCustomHarness(id: string): HarnessProfile {
  return { ...base({ id, label: 'My harness', kind: 'custom' }), builtin: false, command: '', quiet_ms: DEFAULT_QUIET_MS };
}

/** First `custom-N` id not taken. */
export function nextCustomId(profiles: readonly HarnessProfile[]): string {
  const taken = new Set(profiles.map((p) => p.id));
  let n = 1;
  while (taken.has(`custom-${n}`)) n += 1;
  return `custom-${n}`;
}

// ---------------------------------------------------------------------------
// Edit form text <-> profile
// ---------------------------------------------------------------------------

/** Editable text of a profile (one string per input). */
export interface HarnessDraft {
  id: string;
  label: string;
  kind: HarnessKind;
  command: string;
  args: string;
  env: string;
  model: string;
  idle_marker: string;
  dialog_markers: string;
  stop_mode: StopMode;
  quiet_ms: string;
  option_pattern: string;
  dialog_hints: string;
  dsh_profile: string;
}

export function toDraft(p: HarnessProfile): HarnessDraft {
  return {
    id: p.id,
    label: p.label,
    kind: p.kind,
    command: p.command ?? '',
    args: p.args.join('\n'),
    env: Object.entries(p.env)
      .map(([k, v]) => `${k}=${v}`)
      .join('\n'),
    model: p.model ?? '',
    idle_marker: p.idle_marker ?? '',
    dialog_markers: p.dialog_markers.join('\n'),
    stop_mode: p.stop_mode,
    quiet_ms: p.quiet_ms === null ? '' : String(p.quiet_ms),
    option_pattern: p.option_pattern ?? '',
    dialog_hints: (p.dialog_hints ?? []).join('\n'),
    dsh_profile: p.dsh_profile ?? '',
  };
}

/** Non-empty lines, each trimmed. */
export function lines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l !== '');
}

const orNull = (text: string): string | null => (text.trim() === '' ? null : text.trim());

/** `KEY=value` lines -> env map. The value keeps everything after the first '=' (it may contain '='). */
export function parseEnv(text: string): { env: Record<string, string>; error: string | null } {
  const entries: Array<[string, string]> = [];
  for (const [i, line] of lines(text).entries()) {
    const eq = line.indexOf('=');
    if (eq <= 0) return { env: {}, error: `line ${i + 1}: write KEY=value.` };
    const key = line.slice(0, eq).trim();
    const keyErr = envKeyError(key);
    if (keyErr) return { env: {}, error: `line ${i + 1}: ${keyErr}` };
    if (entries.some(([k]) => k === key)) return { env: {}, error: `line ${i + 1}: ${key} is set twice.` };
    entries.push([key, line.slice(eq + 1)]);
  }
  return { env: Object.fromEntries(entries), error: null };
}

/** Draft -> profile, or the first form error (field-prefixed). `builtin` comes from the original. */
export function fromDraft(d: HarnessDraft, builtin: boolean): { profile: HarnessProfile | null; error: string | null } {
  const { env, error } = parseEnv(d.env);
  if (error) return { profile: null, error: `env: ${error}` };
  const quiet = d.quiet_ms.trim();
  const quietMs = quiet === '' ? null : Number(quiet);
  if (quietMs !== null && !(Number.isInteger(quietMs) && quietMs > 0)) {
    return { profile: null, error: 'quiet_ms: Must be a positive whole number of ms.' };
  }
  const custom = d.kind === 'custom';
  const profile: HarnessProfile = {
    id: d.id.trim(),
    label: d.label.trim(),
    kind: d.kind,
    command: orNull(d.command),
    args: lines(d.args),
    env,
    model: orNull(d.model),
    builtin,
    idle_marker: custom ? orNull(d.idle_marker) : null,
    dialog_markers: custom ? lines(d.dialog_markers) : [],
    stop_mode: custom ? d.stop_mode : 'quiet',
    quiet_ms: custom ? quietMs : null,
    option_pattern: custom ? orNull(d.option_pattern) : null,
    dialog_hints: custom ? lines(d.dialog_hints) : [],
    dsh_profile: d.kind === 'deepseek-harness' ? orNull(d.dsh_profile) : null,
  };
  return { profile, error: null };
}
