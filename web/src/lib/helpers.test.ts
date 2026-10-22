import { afterEach, describe, expect, it, vi } from 'vitest';
import { apiPath, toActionResult } from './api';
import { blockFromSearch, parseNext, parsePage, readBoot } from './boot';
import { backoffDelay } from './connection.svelte';
import { relativeTime, shortDuration, worktreeParts } from './format';
import { hunkSides, languageForPath } from './highlight';
import { hostOf, safeHttpUrl } from './links';
import { worktreeNameProblem } from './names';
import { createRefresher } from './refresher';
import { applyTheme, cleanThemeVars, schemeForColor } from './theme';

describe('safeHttpUrl', () => {
  it('accepts http and https', () => {
    expect(safeHttpUrl('https://github.com/acme/shop/pull/42')).toBe('https://github.com/acme/shop/pull/42');
    expect(safeHttpUrl(' http://example.atlassian.net/browse/ABC-123 ')).toBe('http://example.atlassian.net/browse/ABC-123');
  });

  it.each(['javascript:alert(1)', 'JaVaScRiPt:alert(1)', 'file:///etc/hosts', 'smb://host/share', 'vscode://file/x', 'data:,x', 'not a url', ''])(
    'refuses %s',
    (url) => {
      expect(safeHttpUrl(url)).toBeNull();
      expect(hostOf(url)).toBeNull();
    },
  );

  it('reports the host the link really goes to', () => {
    expect(hostOf('https://example.atlassian.net@evil.example/x')).toBe('evil.example');
  });
});

describe('worktreeNameProblem', () => {
  it('accepts names the bridge accepts', () => {
    expect(worktreeNameProblem('retry-backoff')).toBeNull();
    expect(worktreeNameProblem('ABC-123_v2.1')).toBeNull();
    expect(worktreeNameProblem('a'.repeat(64))).toBeNull();
    expect(worktreeNameProblem('9lives')).toBeNull();
  });

  it.each(['', 'a'.repeat(65), 'has space', 'semi;colon', 'new\nline', 'trailing\n', 'slash/name', '$(cmd)', '.', '..', '-rf', '--help', '.hidden', '_x'])(
    'refuses %j',
    (name) => {
      expect(worktreeNameProblem(name)).not.toBeNull();
    },
  );
});

describe('readBoot', () => {
  function fakeDoc(metas: Record<string, string>) {
    const removed: string[] = [];
    return {
      removed,
      querySelector(selector: string) {
        const name = /name="([^"]+)"/.exec(selector)?.[1] ?? '';
        if (!(name in metas)) return null;
        return { getAttribute: () => metas[name], remove: () => removed.push(name) };
      },
    };
  }

  it('reads the page and the key, and removes the key from the DOM', () => {
    const doc = fakeDoc({ 'bn-page': 'panel', 'bn-key': 'k-123' });
    expect(readBoot(doc, { pathname: '/', search: '' })).toEqual({ page: 'panel', key: 'k-123', next: null });
    expect(doc.removed).toEqual(['bn-key']);
  });

  it('falls back to the path when the page meta is missing or unknown', () => {
    expect(parsePage(null, '/changes')).toBe('changes');
    expect(parsePage('<img>', '/panel')).toBe('panel');
    expect(parsePage(null, '/')).toBe('board');
  });

  it('accepts only the service\'s own page paths as the clean path', () => {
    expect(parseNext('/')).toBe('/');
    expect(parseNext('/panel')).toBe('/panel');
    expect(parseNext('/changes?block=12')).toBe('/changes?block=12');
    for (const bad of ['//evil.example/', 'https://evil.example/', '/changes?block=1&x=2', '/api/board', null]) {
      expect(parseNext(bad)).toBeNull();
    }
  });

  it('reads a positive integer block ID only', () => {
    expect(blockFromSearch('?block=3')).toBe(3);
    expect(blockFromSearch('?block=0')).toBeNull();
    expect(blockFromSearch('?block=3x')).toBeNull();
    expect(blockFromSearch('?block=-1')).toBeNull();
    expect(blockFromSearch('')).toBeNull();
  });
});

describe('apiPath', () => {
  it('encodes IDs so they can never change the path', () => {
    expect(apiPath('chats', '../board', 'open')).toBe('/api/chats/..%2Fboard/open');
    expect(apiPath('blocks', 3, 'diff')).toBe('/api/blocks/3/diff');
  });
});

describe('toActionResult', () => {
  it('passes through the service’s error text', () => {
    expect(toActionResult({ ok: false, error: 'iTerm2 not connected' }, 200)).toEqual({ ok: false, error: 'iTerm2 not connected' });
    expect(toActionResult({ ok: true }, 200)).toEqual({ ok: true });
  });

  it('describes replies that aren’t JSON', () => {
    expect(toActionResult(null, 403)).toEqual({ ok: false, error: 'bletherneuk refused this (HTTP 403).' });
    expect(toActionResult({ ok: false }, 500)).toEqual({ ok: false, error: 'Something went wrong.' });
  });
});

describe('theme', () => {
  it('keeps only well-formed custom properties with string values', () => {
    const clean = cleanThemeVars({
      '--background': '#24273a',
      '--shiki-token-keyword': '#c6a0f6',
      color: 'red',
      '--bad name': 'x',
      '--number': 3,
      '--long': 'x'.repeat(600),
    });
    expect([...clean.keys()]).toEqual(['--background', '--shiki-token-keyword']);
  });

  it('sets each variable through the CSSOM and removes ones no longer sent', () => {
    const set = new Map<string, string>();
    const root = {
      style: {
        setProperty: (n: string, v: string) => void set.set(n, v),
        removeProperty: (n: string) => {
          set.delete(n);
          return '';
        },
      },
      dataset: {} as Record<string, string | undefined>,
    };
    applyTheme({ '--background': '#eff1f5', '--accent': '#1e66f5' }, root);
    expect(Object.fromEntries(set)).toEqual({ '--background': '#eff1f5', '--accent': '#1e66f5' });
    expect(root.dataset.scheme).toBe('light');
    applyTheme({ '--background': '#24273a' }, root);
    expect(Object.fromEntries(set)).toEqual({ '--background': '#24273a' });
    expect(root.dataset.scheme).toBe('dark');
  });

  it('tells light from dark backgrounds', () => {
    expect(schemeForColor('#fff')).toBe('light');
    expect(schemeForColor('#24273a')).toBe('dark');
    expect(schemeForColor('rgb(0,0,0)')).toBeNull();
  });
});

describe('languageForPath', () => {
  it('picks a language from the extension and leaves unknown ones plain', () => {
    expect(languageForPath('src/main.rs')).toBe('rust');
    expect(languageForPath('web/src/App.SVELTE')).toBe('svelte');
    expect(languageForPath('~/.zshrc')).toBe('shellscript');
    expect(languageForPath('Makefile')).toBeNull();
    expect(languageForPath('data.parquet')).toBeNull();
    expect(languageForPath('trailing.')).toBeNull();
    expect(languageForPath(null)).toBeNull();
  });
});

describe('hunkSides', () => {
  it('splits a hunk into old and new code, mapping each line back', () => {
    const sides = hunkSides([
      { kind: ' ', old: 1, new: 1, text: 'a' },
      { kind: '-', old: 2, new: null, text: 'b' },
      { kind: '+', old: null, new: 2, text: 'c' },
      { kind: ' ', old: 3, new: 3, text: 'd' },
    ]);
    expect(sides.oldText).toEqual(['a', 'b', 'd']);
    expect(sides.newText).toEqual(['a', 'c', 'd']);
    expect(sides.map).toEqual([
      { side: 'new', index: 0 },
      { side: 'old', index: 1 },
      { side: 'new', index: 1 },
      { side: 'new', index: 2 },
    ]);
  });
});

describe('format', () => {
  const now = new Date(2026, 9, 22, 12, 0).getTime();

  it('shows recent times relative to now', () => {
    expect(relativeTime(now - 10_000, now)).toBe('just now');
    expect(relativeTime(now - 5 * 60_000, now)).toBe('5m ago');
    expect(relativeTime(now - 3 * 3_600_000, now)).toBe('3h ago');
    expect(relativeTime(now - 2 * 86_400_000, now)).toBe('2d ago');
    expect(relativeTime(null, now)).toBe('');
    expect(shortDuration(now - 12 * 60_000, now)).toBe('12m');
  });

  it('describes a worktree like the design’s example', () => {
    expect(
      worktreeParts({ id: 7, name: 'retry-backoff', path: '~/x', is_main: false, branch: 'feat/retry-backoff', uncommitted: 3, ahead: 12, behind: 0 }),
    ).toEqual(['retry-backoff', 'feat/retry-backoff', '3 uncommitted', '12 ahead']);
    expect(worktreeParts({ id: 8, name: 'shop', path: '~/x', is_main: true, detached_at: 'a1b2c3d', uncommitted_many: true })).toEqual([
      'shop (main checkout)',
      'detached at a1b2c3d',
      'many uncommitted',
    ]);
  });
});

describe('backoffDelay', () => {
  it('doubles up to 30 seconds', () => {
    expect([0, 1, 2, 3, 4, 5, 6, 10].map(backoffDelay)).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
  });
});

describe('createRefresher', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('turns a burst of requests into one run', async () => {
    vi.useFakeTimers();
    const run = vi.fn(async () => undefined);
    const refresher = createRefresher(run, 100);
    refresher.request();
    refresher.request();
    refresher.request();
    await vi.advanceTimersByTimeAsync(150);
    expect(run).toHaveBeenCalledTimes(1);
  });

  it('runs once more when asked while a run is in progress', async () => {
    let release: () => void = () => undefined;
    const run = vi.fn(() => new Promise<void>((resolve) => (release = resolve)));
    const refresher = createRefresher(run, 0);
    const first = refresher.now();
    refresher.request();
    refresher.request();
    release();
    await Promise.resolve();
    await Promise.resolve();
    release();
    await first;
    expect(run).toHaveBeenCalledTimes(2);
  });
});
