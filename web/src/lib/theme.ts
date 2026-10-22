// Applies theme variables from GET /api/theme and the SSE `theme` event.
// Each variable is set through the CSSOM, which the service's CSP allows.

const VAR_NAME = /^--[A-Za-z0-9_-]{1,64}$/;
const MAX_VALUE_LENGTH = 512;

interface StyleTarget {
  style: { setProperty(name: string, value: string): void; removeProperty(name: string): string };
  dataset: Record<string, string | undefined>;
}

let applied = new Set<string>();

/** Keeps only well-formed `--name: value` pairs. */
export function cleanThemeVars(vars: unknown): Map<string, string> {
  const out = new Map<string, string>();
  if (vars === null || typeof vars !== 'object') return out;
  for (const [name, value] of Object.entries(vars as Record<string, unknown>)) {
    if (!VAR_NAME.test(name) || typeof value !== 'string') continue;
    if (value.length > MAX_VALUE_LENGTH) continue;
    out.set(name, value);
  }
  return out;
}

export function applyTheme(vars: unknown, root: StyleTarget = document.documentElement): void {
  const clean = cleanThemeVars(vars);
  for (const name of applied) {
    if (!clean.has(name)) root.style.removeProperty(name);
  }
  for (const [name, value] of clean) root.style.setProperty(name, value);
  applied = new Set(clean.keys());
  const bg = clean.get('--background');
  if (bg !== undefined) {
    const scheme = schemeForColor(bg);
    if (scheme) root.dataset.scheme = scheme;
  }
}

/** "light" or "dark" for a #rgb / #rrggbb colour, so native controls and scrollbars match. */
export function schemeForColor(color: string): 'light' | 'dark' | null {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})([0-9a-f]{2})?$/i.exec(color.trim());
  if (!m) return null;
  let hex = m[1];
  if (hex.length === 3) hex = hex.split('').map((c) => c + c).join('');
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  const luminance = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  return luminance > 0.4 ? 'light' : 'dark';
}
