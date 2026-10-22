// Syntax highlighting for the diff with Shiki, without WebAssembly (the CSP blocks it) and
// without {@html}: tokens are rendered as text spans whose colour is a CSS variable.
//
// - The JavaScript regex engine replaces Oniguruma.
// - The CSS-variables theme makes every colour `var(--shiki-...)`, so the page theme decides.
// - Grammars load on first use, one chunk per language.

import { createCssVariablesTheme, createHighlighterCore } from 'shiki/core';
import type { HighlighterCore, LanguageRegistration, ThemedToken } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import type { DiffLine, Hunk } from './types';

export type { ThemedToken };

export type LangId =
  | 'typescript'
  | 'tsx'
  | 'javascript'
  | 'jsx'
  | 'json'
  | 'python'
  | 'rust'
  | 'go'
  | 'sql'
  | 'yaml'
  | 'toml'
  | 'markdown'
  | 'html'
  | 'css'
  | 'shellscript'
  | 'svelte';

type Loader = () => Promise<{ default: LanguageRegistration[] }>;

const LOADERS: Record<LangId, Loader> = {
  typescript: () => import('shiki/langs/typescript.mjs'),
  tsx: () => import('shiki/langs/tsx.mjs'),
  javascript: () => import('shiki/langs/javascript.mjs'),
  jsx: () => import('shiki/langs/jsx.mjs'),
  json: () => import('shiki/langs/json.mjs'),
  python: () => import('shiki/langs/python.mjs'),
  rust: () => import('shiki/langs/rust.mjs'),
  go: () => import('shiki/langs/go.mjs'),
  sql: () => import('shiki/langs/sql.mjs'),
  yaml: () => import('shiki/langs/yaml.mjs'),
  toml: () => import('shiki/langs/toml.mjs'),
  markdown: () => import('shiki/langs/markdown.mjs'),
  html: () => import('shiki/langs/html.mjs'),
  css: () => import('shiki/langs/css.mjs'),
  shellscript: () => import('shiki/langs/shellscript.mjs'),
  svelte: () => import('shiki/langs/svelte.mjs'),
};

export const LANG_LABEL: Record<LangId, string> = {
  typescript: 'TypeScript',
  tsx: 'TSX',
  javascript: 'JavaScript',
  jsx: 'JSX',
  json: 'JSON',
  python: 'Python',
  rust: 'Rust',
  go: 'Go',
  sql: 'SQL',
  yaml: 'YAML',
  toml: 'TOML',
  markdown: 'Markdown',
  html: 'HTML',
  css: 'CSS',
  shellscript: 'Shell',
  svelte: 'Svelte',
};

// A Map, so names such as "constructor" or "__proto__" can never match an Object property.
const BY_EXTENSION = new Map<string, LangId>([
  ['ts', 'typescript'],
  ['mts', 'typescript'],
  ['cts', 'typescript'],
  ['tsx', 'tsx'],
  ['js', 'javascript'],
  ['mjs', 'javascript'],
  ['cjs', 'javascript'],
  ['jsx', 'jsx'],
  ['json', 'json'],
  ['jsonc', 'json'],
  ['geojson', 'json'],
  ['py', 'python'],
  ['pyi', 'python'],
  ['rs', 'rust'],
  ['go', 'go'],
  ['sql', 'sql'],
  ['yaml', 'yaml'],
  ['yml', 'yaml'],
  ['toml', 'toml'],
  ['md', 'markdown'],
  ['markdown', 'markdown'],
  ['html', 'html'],
  ['htm', 'html'],
  ['css', 'css'],
  ['sh', 'shellscript'],
  ['bash', 'shellscript'],
  ['zsh', 'shellscript'],
  ['bashrc', 'shellscript'],
  ['zshrc', 'shellscript'],
  ['svelte', 'svelte'],
]);

/** The language for a file path, from its extension. Unknown extensions get null (plain text). */
export function languageForPath(path: string | null | undefined): LangId | null {
  if (!path) return null;
  const base = path.slice(path.lastIndexOf('/') + 1);
  const dot = base.lastIndexOf('.');
  if (dot < 0 || dot === base.length - 1) return null;
  return BY_EXTENSION.get(base.slice(dot + 1).toLowerCase()) ?? null;
}

export function languageLabel(lang: LangId | null): string | null {
  return lang === null ? null : LANG_LABEL[lang];
}

const THEME_NAME = 'bn-css-variables';

let highlighter: Promise<HighlighterCore> | null = null;
const languages = new Map<LangId, Promise<boolean>>();

function getHighlighter(): Promise<HighlighterCore> {
  highlighter ??= createHighlighterCore({
    themes: [createCssVariablesTheme({ name: THEME_NAME, variablePrefix: '--shiki-', fontStyle: true })],
    langs: [],
    engine: createJavaScriptRegexEngine({ forgiving: true }),
  });
  return highlighter;
}

async function ensureLanguage(hl: HighlighterCore, lang: LangId): Promise<boolean> {
  let loading = languages.get(lang);
  if (!loading) {
    loading = LOADERS[lang]()
      .then(async (mod) => {
        await hl.loadLanguage(...mod.default);
        return true;
      })
      .catch(() => false);
    languages.set(lang, loading);
  }
  return loading;
}

/** Lines longer than this are left plain, so one minified line can't stall the page. */
const MAX_LINE_LENGTH = 2000;
/** Hunks are tokenised in pieces of this many lines, yielding between pieces. */
const CHUNK_LINES = 400;

/**
 * Tokenises consecutive lines of one language. Returns null when highlighting isn't
 * possible, in which case the lines are shown as plain text.
 */
export async function tokenizeLines(lines: string[], lang: LangId): Promise<ThemedToken[][] | null> {
  if (lines.length === 0) return [];
  try {
    const hl = await getHighlighter();
    if (!(await ensureLanguage(hl, lang))) return null;
    const out: ThemedToken[][] = [];
    for (let start = 0; start < lines.length; start += CHUNK_LINES) {
      const chunk = lines.slice(start, start + CHUNK_LINES);
      const result = hl.codeToTokens(chunk.join('\n'), {
        lang,
        theme: THEME_NAME,
        tokenizeMaxLineLength: MAX_LINE_LENGTH,
        tokenizeTimeLimit: 200,
      });
      if (result.tokens.length !== chunk.length) return null;
      out.push(...result.tokens);
      if (start + CHUNK_LINES < lines.length) await yieldToBrowser();
    }
    return out;
  } catch {
    return null;
  }
}

function yieldToBrowser(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

export interface HunkSides {
  oldText: string[];
  newText: string[];
  /** For each hunk line: which side it is highlighted from, and its index there. */
  map: { side: 'old' | 'new'; index: number }[];
}

/**
 * Splits a hunk into its old side (context and removed lines) and new side (context and
 * added lines), so each side is tokenised as continuous code. Context lines use the new side.
 */
export function hunkSides(lines: DiffLine[]): HunkSides {
  const oldText: string[] = [];
  const newText: string[] = [];
  const map: HunkSides['map'] = [];
  for (const line of lines) {
    if (line.kind === '-') {
      map.push({ side: 'old', index: oldText.length });
      oldText.push(line.text);
    } else if (line.kind === '+') {
      map.push({ side: 'new', index: newText.length });
      newText.push(line.text);
    } else {
      oldText.push(line.text);
      map.push({ side: 'new', index: newText.length });
      newText.push(line.text);
    }
  }
  return { oldText, newText, map };
}

/**
 * Tokens for each line of a hunk. A line's entry is null when it couldn't be highlighted,
 * and is then shown as plain text.
 */
export async function highlightHunk(hunk: Hunk, lang: LangId): Promise<(ThemedToken[] | null)[]> {
  const sides = hunkSides(hunk.lines);
  const hasRemoved = hunk.lines.some((l) => l.kind === '-');
  const newTokens = await tokenizeLines(sides.newText, lang);
  const oldTokens = hasRemoved ? await tokenizeLines(sides.oldText, lang) : null;
  return sides.map.map(({ side, index }) => {
    const source = side === 'old' ? oldTokens : newTokens;
    return source?.[index] ?? null;
  });
}

const SAFE_COLOR = /^(var\(--shiki-[a-z0-9-]+\)|#[0-9a-f]{3,8})$/i;

/** A token's colour, only when it is one of the theme's CSS variables or a hex colour. */
export function tokenColor(token: Pick<ThemedToken, 'color'>): string | undefined {
  return token.color !== undefined && SAFE_COLOR.test(token.color) ? token.color : undefined;
}

/** Shiki's font style bit flags. */
export const FONT_ITALIC = 1;
export const FONT_BOLD = 2;
export const FONT_UNDERLINE = 4;
