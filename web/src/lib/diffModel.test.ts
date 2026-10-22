import { describe, expect, it } from 'vitest';
import {
  MAX_COLUMNS,
  ROW_UNITS,
  buildLayout,
  computeWindow,
  fileAt,
  fileDisplayPath,
  fileOffset,
  gutterDigits,
  lineColumns,
  rowAt,
} from './diffModel';
import type { DiffLine, FileDiff, Hunk } from './types';

function lines(count: number, kind: DiffLine['kind'] = ' ', start = 1): DiffLine[] {
  return Array.from({ length: count }, (_, i) => ({
    kind,
    old: kind === '+' ? null : start + i,
    new: kind === '-' ? null : start + i,
    text: `line ${start + i}`,
  }));
}

function hunk(count: number, kind: DiffLine['kind'] = ' ', start = 1): Hunk {
  return { header: `@@ -${start},${count} +${start},${count} @@`, lines: lines(count, kind, start) };
}

function file(overrides: Partial<FileDiff>): FileDiff {
  return {
    old_path: 'src/a.ts',
    new_path: 'src/a.ts',
    status: 'modified',
    binary: false,
    additions: 0,
    deletions: 0,
    collapsed: null,
    truncated: false,
    unavailable: false,
    hunks: [],
    ...overrides,
  };
}

describe('buildLayout', () => {
  it('lays out a file header, hunk headers and lines with prefix-sum offsets', () => {
    const layout = buildLayout([file({ hunks: [hunk(3), hunk(2, '+', 10)] })]);

    expect(layout.rows.map((r) => r.kind)).toEqual(['file', 'hunk', 'line', 'line', 'line', 'hunk', 'line', 'line']);
    // file header 2 units, then 1 unit per hunk header and line.
    expect(layout.offsets).toEqual([0, 2, 3, 4, 5, 6, 7, 8, 9]);
    expect(layout.total).toBe(9);
    expect(layout.fileRow).toEqual([0]);
  });

  it('puts a gap between files but not after the last one', () => {
    const layout = buildLayout([file({ hunks: [hunk(1)] }), file({ new_path: 'src/b.ts', hunks: [hunk(1)] })]);

    expect(layout.rows.map((r) => r.kind)).toEqual(['file', 'hunk', 'line', 'gap', 'file', 'hunk', 'line']);
    expect(layout.fileRow).toEqual([0, 4]);
    expect(fileOffset(layout, 1)).toBe(ROW_UNITS.file + 2 + ROW_UNITS.gap);
    expect(layout.total).toBe(2 * (ROW_UNITS.file + 2) + ROW_UNITS.gap);
  });

  it('starts lockfile, minified and large files collapsed with a notice and no lines', () => {
    const files = (['lockfile', 'minified', 'large'] as const).map((reason, i) =>
      file({ new_path: `f${i}`, collapsed: reason, hunks: [hunk(5000)] }),
    );
    const layout = buildLayout(files);

    expect(layout.rows.filter((r) => r.kind === 'line')).toHaveLength(0);
    expect(layout.rows.filter((r) => r.kind === 'notice' && r.notice === 'collapsed')).toHaveLength(3);
    expect(layout.rows.filter((r) => r.kind === 'file').every((r) => r.kind === 'file' && r.collapsed)).toBe(true);
    // Each collapsed file is header + notice, with gaps between files.
    expect(layout.total).toBe(3 * (ROW_UNITS.file + ROW_UNITS.notice) + 2 * ROW_UNITS.gap);
  });

  it('expands a collapsed file when the user asks, and collapses a normal one', () => {
    const files = [file({ collapsed: 'lockfile', hunks: [hunk(4)] }), file({ new_path: 'b', hunks: [hunk(2)] })];
    const layout = buildLayout(files, new Map([[0, false], [1, true]]));

    expect(layout.rows.map((r) => r.kind)).toEqual(['file', 'hunk', 'line', 'line', 'line', 'line', 'gap', 'file']);
    // A file collapsed by hand shows only its header: no notice.
    expect(layout.rows.at(-1)).toMatchObject({ kind: 'file', collapsed: true });
  });

  it('lists binary files with a notice and never shows their lines', () => {
    const layout = buildLayout([file({ binary: true, hunks: [hunk(3)] })]);
    expect(layout.rows.map((r) => r.kind)).toEqual(['file', 'notice']);
    expect(layout.rows[1]).toMatchObject({ notice: 'binary' });
  });

  it('says when a file is unavailable, truncated or has no content changes', () => {
    const layout = buildLayout([
      file({ new_path: 'gone', unavailable: true }),
      file({ new_path: 'cut', truncated: true, hunks: [hunk(2)] }),
      file({ new_path: 'moved', status: 'renamed', old_path: 'old' }),
    ]);
    const notices = layout.rows.filter((r) => r.kind === 'notice').map((r) => (r.kind === 'notice' ? r.notice : null));
    expect(notices).toEqual(['unavailable', 'truncated', 'empty']);
    // The truncated notice comes after the file's lines.
    const cutStart = layout.fileRow[1];
    expect(layout.rows.slice(cutStart, cutStart + 5).map((r) => r.kind)).toEqual(['file', 'hunk', 'line', 'line', 'notice']);
  });

  it('records the widest line, counting tabs, and the largest line number', () => {
    const layout = buildLayout([
      file({
        hunks: [
          {
            header: '@@ -1,2 +1,2 @@',
            lines: [
              { kind: ' ', old: 9, new: 9, text: '\tabc' },
              { kind: '+', old: null, new: 1234, text: 'x' },
            ],
          },
        ],
      }),
    ]);
    expect(layout.maxColumns).toBe(7);
    expect(layout.maxLineNumber).toBe(1234);
    expect(gutterDigits(layout.maxLineNumber)).toBe(4);
    expect(gutterDigits(7)).toBe(3);
  });

  it('caps the scroll width for very long lines', () => {
    const layout = buildLayout([file({ hunks: [{ header: '@@', lines: [{ kind: '+', new: 1, text: 'x'.repeat(50_000) }] }] })]);
    expect(layout.maxColumns).toBe(MAX_COLUMNS);
  });

  it('handles tens of thousands of lines', () => {
    const layout = buildLayout([file({ hunks: [hunk(40_000, '+')] })]);
    expect(layout.rows).toHaveLength(40_002);
    expect(layout.total).toBe(ROW_UNITS.file + 1 + 40_000);
  });
});

describe('lineColumns', () => {
  it('counts a tab as four columns', () => {
    expect(lineColumns('a\tb')).toBe(6);
    expect(lineColumns('')).toBe(0);
  });
});

describe('rowAt', () => {
  const offsets = [0, 2, 3, 4, 10];

  it('finds the row containing a position', () => {
    expect(rowAt(offsets, 0)).toBe(0);
    expect(rowAt(offsets, 1.9)).toBe(0);
    expect(rowAt(offsets, 2)).toBe(1);
    expect(rowAt(offsets, 3.5)).toBe(2);
    expect(rowAt(offsets, 9.99)).toBe(3);
  });

  it('clamps positions outside the content', () => {
    expect(rowAt(offsets, -5)).toBe(0);
    expect(rowAt(offsets, 10)).toBe(3);
    expect(rowAt(offsets, 500)).toBe(3);
    expect(rowAt([0], 3)).toBe(0);
  });
});

describe('computeWindow', () => {
  // 100 rows of one unit each.
  const offsets = Array.from({ length: 101 }, (_, i) => i);

  it('returns only the rows in view when there is no overscan', () => {
    expect(computeWindow(offsets, 10, 5, 0)).toEqual({ start: 10, end: 15 });
  });

  it('includes partly visible rows at both edges', () => {
    expect(computeWindow(offsets, 10.5, 5, 0)).toEqual({ start: 10, end: 16 });
  });

  it('adds overscan either side and clamps to the rows that exist', () => {
    expect(computeWindow(offsets, 10, 5, 3)).toEqual({ start: 7, end: 18 });
    expect(computeWindow(offsets, 0, 5, 20)).toEqual({ start: 0, end: 25 });
    expect(computeWindow(offsets, 98, 5, 20)).toEqual({ start: 78, end: 100 });
  });

  it('clamps a scroll position past the end', () => {
    expect(computeWindow(offsets, 1000, 5, 0)).toEqual({ start: 99, end: 100 });
  });

  it('works with rows of different heights', () => {
    const layout = buildLayout([file({ hunks: [hunk(3)] }), file({ new_path: 'b', hunks: [hunk(3)] })]);
    // offsets: file 0-2, hunk 2-3, lines 3-6, gap 6-7, file 7-9, hunk 9-10, lines 10-13
    const win = computeWindow(layout.offsets, 6.5, 3, 0);
    expect(layout.rows.slice(win.start, win.end).map((r) => r.kind)).toEqual(['gap', 'file', 'hunk']);
    expect(fileAt(layout, 6.5)).toBe(0);
    expect(fileAt(layout, 7)).toBe(1);
  });

  it('returns an empty window for an empty diff', () => {
    expect(computeWindow([0], 0, 100, 10)).toEqual({ start: 0, end: 0 });
    expect(fileAt(buildLayout([]), 0)).toBeNull();
  });

  it('keeps the DOM small for a huge diff', () => {
    const layout = buildLayout([file({ hunks: [hunk(50_000, '+')] })]);
    const win = computeWindow(layout.offsets, 25_000, 40, 30);
    expect(win.end - win.start).toBe(40 + 2 * 30);
  });
});

describe('fileDisplayPath', () => {
  it('shows both paths for a rename', () => {
    expect(fileDisplayPath({ status: 'renamed', old_path: 'a.ts', new_path: 'b.ts' })).toBe('a.ts → b.ts');
  });

  it('uses the old path for a deletion', () => {
    expect(fileDisplayPath({ status: 'deleted', old_path: 'a.ts', new_path: null })).toBe('a.ts');
  });
});
