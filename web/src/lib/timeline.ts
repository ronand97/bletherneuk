// The Changes page timeline (design section 13): turns and review markers interleaved,
// newest first, with day headings.

import { dayLabel, formatCount, plural, sameDay } from './format';
import type { Review, Turn } from './types';

export const PROMPT_PREVIEW_CHARS = 200;

export type TimelineEntry =
  | { kind: 'turn'; key: string; time: number; turn: Turn; unreviewed: boolean }
  | { kind: 'review'; key: string; time: number; review: Review; latest: boolean };

export type TimelineItem = TimelineEntry | { kind: 'day'; key: string; label: string };

function byNewest<T>(items: T[], time: (item: T) => number): T[] {
  return items
    .map((item, index) => ({ item, index }))
    .sort((a, b) => time(b.item) - time(a.item) || a.index - b.index)
    .map((x) => x.item);
}

/** The time of the latest review marker for each worktree. */
export function latestReviews(reviews: Review[]): Map<number, number> {
  const latest = new Map<number, number>();
  for (const r of reviews) {
    const prev = latest.get(r.worktree);
    if (prev === undefined || r.created_at > prev) latest.set(r.worktree, r.created_at);
  }
  return latest;
}

/**
 * Whether a turn has changes after the worktree's latest review. A turn still running, or
 * one that ended after the review, counts. Turns in other worktrees never count.
 */
export function isUnreviewed(turn: Turn, worktree: number | null, latest: Map<number, number>): boolean {
  if (worktree === null || turn.worktree !== worktree) return false;
  const reviewedAt = latest.get(worktree);
  if (reviewedAt === undefined) return true;
  return turn.ended_at == null || turn.ended_at > reviewedAt;
}

/**
 * Merges turns and review markers, newest first. Turns are placed by start time and
 * reviews by creation time. When a turn starts at the same moment as a review, the turn
 * is newer, since its changes come after the review's snapshot.
 */
export function mergeTimeline(turns: Turn[], reviews: Review[], worktree: number | null): TimelineEntry[] {
  const t = byNewest(turns, (x) => x.started_at);
  const r = byNewest(reviews, (x) => x.created_at);
  const latest = latestReviews(reviews);
  // The first review of each worktree in newest-first order is that worktree's latest.
  const latestIds = new Set<number>();
  const seen = new Set<number>();
  for (const review of r) {
    if (seen.has(review.worktree)) continue;
    seen.add(review.worktree);
    latestIds.add(review.id);
  }
  const out: TimelineEntry[] = [];
  let i = 0;
  let j = 0;
  while (i < t.length || j < r.length) {
    const takeTurn = j >= r.length || (i < t.length && t[i].started_at >= r[j].created_at);
    if (takeTurn) {
      const turn = t[i++];
      out.push({ kind: 'turn', key: `t${turn.id}`, time: turn.started_at, turn, unreviewed: isUnreviewed(turn, worktree, latest) });
    } else {
      const review = r[j++];
      out.push({ kind: 'review', key: `r${review.id}`, time: review.created_at, review, latest: latestIds.has(review.id) });
    }
  }
  return out;
}

/** Adds a day heading before the first entry of each local day. */
export function withDays(entries: TimelineEntry[], now: number): TimelineItem[] {
  const out: TimelineItem[] = [];
  let prev: number | null = null;
  for (const entry of entries) {
    if (prev === null || !sameDay(prev, entry.time)) {
      out.push({ kind: 'day', key: `d${entry.key}`, label: dayLabel(entry.time, now) });
    }
    out.push(entry);
    prev = entry.time;
  }
  return out;
}

/** The first 200 characters of a prompt on one line. */
export function promptPreview(prompt: string | null | undefined): string {
  if (prompt == null) return '(no prompt text)';
  const flat = prompt.replace(/\s+/g, ' ').trim();
  if (flat === '') return '(empty prompt)';
  const chars = Array.from(flat);
  if (chars.length <= PROMPT_PREVIEW_CHARS) return flat;
  return `${chars.slice(0, PROMPT_PREVIEW_CHARS).join('').trimEnd()}…`;
}

export interface TurnStats {
  files: string | null;
  added: string | null;
  removed: string | null;
}

export function turnStats(turn: Turn): TurnStats {
  return {
    files: turn.files_changed == null ? null : plural(turn.files_changed, 'file'),
    added: turn.lines_added == null ? null : `+${formatCount(turn.lines_added)}`,
    removed: turn.lines_removed == null ? null : `−${formatCount(turn.lines_removed)}`,
  };
}

export type TurnStateTone = 'running' | 'missed' | 'deleted';

export interface TurnState {
  tone: TurnStateTone;
  text: string;
  detail: string | null;
}

/** A turn's special state, or null for a finished turn with its snapshots. */
export function turnState(turn: Turn): TurnState | null {
  if (turn.missed != null && turn.missed !== '') {
    return { tone: 'missed', text: 'snapshot missed', detail: turn.missed };
  }
  if (turn.snapshot_deleted) {
    return { tone: 'deleted', text: 'snapshot deleted to save space', detail: null };
  }
  if (turn.ended_at == null) {
    return { tone: 'running', text: 'in progress', detail: null };
  }
  return null;
}

export const PARALLEL_NOTE = 'Other chats were working here; this diff may include their changes.';

/**
 * The worktree the Changes page shows first: that of the most recent turn with a worktree
 * (the service's default), then the one with unreviewed changes, then the first one.
 */
export function defaultWorktree(
  turns: Turn[],
  changesSinceReview: { worktree: number } | null | undefined,
  worktrees: { id: number; removed?: boolean }[],
): number | null {
  const known = new Set(worktrees.map((w) => w.id));
  const recent = byNewest(turns, (x) => x.started_at).find((x) => x.worktree != null && known.has(x.worktree));
  if (recent?.worktree != null) return recent.worktree;
  if (changesSinceReview && known.has(changesSinceReview.worktree)) return changesSinceReview.worktree;
  const live = worktrees.find((w) => !w.removed) ?? worktrees[0];
  return live ? live.id : null;
}
