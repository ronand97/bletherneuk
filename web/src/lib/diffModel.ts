// Layout for the virtualised diff view. Every file, hunk header, line and notice becomes a
// row with a height in "units" (one unit is one code line). Offsets are prefix sums, so the
// rows on screen are found with a binary search and only those go into the DOM.

import type { FileDiff } from './types';

export const ROW_UNITS = { file: 2, hunk: 1, line: 1, notice: 2, gap: 1 } as const;

/** Columns a tab takes when sizing the scroll width. */
export const TAB_WIDTH = 4;
/** The scroll width never grows past this many columns; longer lines are clipped. */
export const MAX_COLUMNS = 2000;

export type NoticeKind = 'binary' | 'collapsed' | 'truncated' | 'unavailable' | 'empty';

export type DiffRow =
  | { kind: 'file'; key: string; file: number; collapsed: boolean }
  | { kind: 'notice'; key: string; file: number; notice: NoticeKind }
  | { kind: 'hunk'; key: string; file: number; hunk: number }
  | { kind: 'line'; key: string; file: number; hunk: number; line: number }
  | { kind: 'gap'; key: string; file: number };

export interface DiffLayout {
  rows: DiffRow[];
  /** `offsets[i]` is the top of row i in units. `offsets[rows.length]` is the total height. */
  offsets: number[];
  /** Row index of each file's header row. */
  fileRow: number[];
  total: number;
  /** Widest line shown, in columns, capped at MAX_COLUMNS. */
  maxColumns: number;
  /** Largest line number shown, for sizing the gutters. */
  maxLineNumber: number;
}

export type CollapseOverrides = ReadonlyMap<number, boolean>;

/** The path a file is known by now: the new path, or the old one for deletions. */
export function filePath(f: Pick<FileDiff, 'new_path' | 'old_path'>): string {
  return f.new_path ?? f.old_path ?? '(unnamed file)';
}

/** "old → new" for renames and copies, otherwise the path. */
export function fileDisplayPath(f: Pick<FileDiff, 'new_path' | 'old_path' | 'status'>): string {
  if ((f.status === 'renamed' || f.status === 'copied') && f.old_path && f.new_path && f.old_path !== f.new_path) {
    return `${f.old_path} → ${f.new_path}`;
  }
  return filePath(f);
}

/** Lockfiles, minified files and very large files start collapsed. */
export function startsCollapsed(f: Pick<FileDiff, 'collapsed'>): boolean {
  return f.collapsed != null;
}

/** Binary and unavailable files have no body to show or collapse. */
export function hasBody(f: Pick<FileDiff, 'binary' | 'unavailable'>): boolean {
  return !f.binary && !f.unavailable;
}

export function isCollapsed(f: FileDiff, index: number, overrides: CollapseOverrides): boolean {
  if (!hasBody(f)) return false;
  return overrides.get(index) ?? startsCollapsed(f);
}

export function lineColumns(text: string): number {
  let cols = 0;
  for (let i = 0; i < text.length; i++) cols += text.charCodeAt(i) === 9 ? TAB_WIDTH : 1;
  return cols;
}

export function buildLayout(files: FileDiff[], overrides: CollapseOverrides = new Map()): DiffLayout {
  const rows: DiffRow[] = [];
  const offsets: number[] = [];
  const fileRow: number[] = [];
  let y = 0;
  let maxColumns = 0;
  let maxLineNumber = 0;

  const push = (row: DiffRow, units: number) => {
    rows.push(row);
    offsets.push(y);
    y += units;
  };

  files.forEach((f, fi) => {
    const collapsed = isCollapsed(f, fi, overrides);
    fileRow.push(rows.length);
    push({ kind: 'file', key: `f${fi}`, file: fi, collapsed }, ROW_UNITS.file);

    if (f.unavailable) {
      push({ kind: 'notice', key: `n${fi}`, file: fi, notice: 'unavailable' }, ROW_UNITS.notice);
    } else if (f.binary) {
      push({ kind: 'notice', key: `n${fi}`, file: fi, notice: 'binary' }, ROW_UNITS.notice);
    } else if (collapsed) {
      // A file collapsed by hand shows only its header. One that starts collapsed says why.
      if (startsCollapsed(f)) push({ kind: 'notice', key: `n${fi}`, file: fi, notice: 'collapsed' }, ROW_UNITS.notice);
    } else {
      const hunks = f.hunks ?? [];
      if (hunks.length === 0 && !f.truncated) {
        push({ kind: 'notice', key: `n${fi}`, file: fi, notice: 'empty' }, ROW_UNITS.notice);
      }
      hunks.forEach((h, hi) => {
        push({ kind: 'hunk', key: `h${fi}:${hi}`, file: fi, hunk: hi }, ROW_UNITS.hunk);
        h.lines.forEach((line, li) => {
          push({ kind: 'line', key: `l${fi}:${hi}:${li}`, file: fi, hunk: hi, line: li }, ROW_UNITS.line);
          const cols = lineColumns(line.text);
          if (cols > maxColumns) maxColumns = cols;
          if (line.old != null && line.old > maxLineNumber) maxLineNumber = line.old;
          if (line.new != null && line.new > maxLineNumber) maxLineNumber = line.new;
        });
      });
      if (f.truncated) push({ kind: 'notice', key: `t${fi}`, file: fi, notice: 'truncated' }, ROW_UNITS.notice);
    }

    if (fi < files.length - 1) push({ kind: 'gap', key: `g${fi}`, file: fi }, ROW_UNITS.gap);
  });

  offsets.push(y);
  return { rows, offsets, fileRow, total: y, maxColumns: Math.min(maxColumns, MAX_COLUMNS), maxLineNumber };
}

/** The index of the row containing position `y` (in units), clamped to the rows that exist. */
export function rowAt(offsets: number[], y: number): number {
  const n = offsets.length - 1;
  if (n <= 0 || y <= offsets[0]) return 0;
  let lo = 0;
  let hi = n - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (offsets[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

export interface RowWindow {
  /** First row to render. */
  start: number;
  /** One past the last row to render. */
  end: number;
}

/**
 * The rows to render for a viewport: every row that overlaps [scrollTop, scrollTop + viewport),
 * plus `overscan` rows either side. All values are in units.
 */
export function computeWindow(offsets: number[], scrollTop: number, viewport: number, overscan: number): RowWindow {
  const n = offsets.length - 1;
  if (n <= 0) return { start: 0, end: 0 };
  const top = Math.max(0, Math.min(scrollTop, offsets[n]));
  const bottom = top + Math.max(0, viewport);
  const first = rowAt(offsets, top);
  let last = rowAt(offsets, bottom);
  if (offsets[last] < bottom) last += 1;
  last = Math.max(last, first + 1);
  return { start: Math.max(0, first - overscan), end: Math.min(n, last + overscan) };
}

/** The file whose rows are at the top of the viewport. */
export function fileAt(layout: DiffLayout, scrollTop: number): number | null {
  if (layout.rows.length === 0) return null;
  return layout.rows[rowAt(layout.offsets, scrollTop)].file;
}

/** The top of a file's header row, in units. */
export function fileOffset(layout: DiffLayout, file: number): number {
  const row = layout.fileRow[file];
  return row === undefined ? 0 : layout.offsets[row];
}

/** Digits needed for the largest line number, at least 3. */
export function gutterDigits(maxLineNumber: number): number {
  return Math.max(3, String(Math.max(0, Math.floor(maxLineNumber))).length);
}
