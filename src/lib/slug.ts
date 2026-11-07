/** Node id slugs: lowercase ASCII, digits and single dashes; safe as a folder name on every OS. */

export const SLUG_MAX_LENGTH = 40;
const FALLBACK = 'node';
const WINDOWS_RESERVED = new Set([
  'con', 'prn', 'aux', 'nul',
  'com1', 'com2', 'com3', 'com4', 'com5', 'com6', 'com7', 'com8', 'com9',
  'lpt1', 'lpt2', 'lpt3', 'lpt4', 'lpt5', 'lpt6', 'lpt7', 'lpt8', 'lpt9',
]);

const SLUG_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** Turn free text into a slug. Never returns an empty or reserved string. */
export function slugify(text: string): string {
  const base = text
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, SLUG_MAX_LENGTH)
    .replace(/-+$/g, '');
  if (base === '') return FALLBACK;
  return WINDOWS_RESERVED.has(base) ? `${base}-node` : base;
}

/** True if `id` is a valid slug (what the id field accepts). */
export function isValidSlug(id: string): boolean {
  return id.length <= SLUG_MAX_LENGTH && SLUG_RE.test(id) && !WINDOWS_RESERVED.has(id);
}

/** `slugify(text)`, suffixed with -2, -3, ... until it is not in `existing`. */
export function uniqueSlug(text: string, existing: Iterable<string>): string {
  const taken = new Set(existing);
  const base = slugify(text);
  if (!taken.has(base)) return base;
  for (let i = 2; ; i += 1) {
    const candidate = `${base}-${i}`;
    if (!taken.has(candidate)) return candidate;
  }
}
