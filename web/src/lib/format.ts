// Small text helpers shared by every page.

import type { ChatCounts, ChatStatus, ChecksState, PrBadge, Worktree } from './types';

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${formatCount(n)} ${n === 1 ? one : many}`;
}

export function formatCount(n: number): string {
  return Number.isFinite(n) ? Math.round(n).toLocaleString('en-US') : '?';
}

/** "just now", "5m ago", "3h ago", "2d ago", then a date. */
export function relativeTime(ms: number | null | undefined, now: number): string {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return '';
  const diff = now - ms;
  if (diff < 45_000) return 'just now';
  if (diff < HOUR) return `${Math.max(1, Math.round(diff / MINUTE))}m ago`;
  if (diff < DAY) return `${Math.round(diff / HOUR)}h ago`;
  if (diff < 7 * DAY) return `${Math.round(diff / DAY)}d ago`;
  return shortDate(ms);
}

/** "40s", "12m", "3h", "2d": how long since `ms`. */
export function shortDuration(ms: number | null | undefined, now: number): string {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return '';
  const diff = Math.max(0, now - ms);
  if (diff < MINUTE) return `${Math.max(1, Math.round(diff / 1000))}s`;
  if (diff < HOUR) return `${Math.round(diff / MINUTE)}m`;
  if (diff < DAY) return `${Math.round(diff / HOUR)}h`;
  return `${Math.round(diff / DAY)}d`;
}

export function clockTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

export function shortDate(ms: number): string {
  const d = new Date(ms);
  return `${d.getDate()} ${MONTHS[d.getMonth()]}`;
}

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "Today", "Yesterday", "Mon 20 Oct". Uses local time. */
export function dayLabel(ms: number, now: number): string {
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / DAY);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  const d = new Date(ms);
  return `${WEEKDAYS[d.getDay()]} ${shortDate(ms)}`;
}

export function sameDay(a: number, b: number): boolean {
  return startOfDay(a) === startOfDay(b);
}

export function fileSize(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return '';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export const STATUS_LABEL: Record<ChatStatus, string> = {
  needs_you: 'needs you',
  working: 'working',
  done: 'done',
  idle: 'idle',
  closed: 'closed',
};

/** Statuses in the order they're shown in counts. */
export const STATUS_ORDER: ChatStatus[] = ['needs_you', 'working', 'done', 'idle', 'closed'];

export function totalChats(counts: ChatCounts): number {
  return STATUS_ORDER.reduce((sum, s) => sum + (counts[s] ?? 0), 0);
}

export type PrTone = 'draft' | 'open' | 'merged' | 'closed' | 'unknown';

export function prTone(pr: Pick<PrBadge, 'state' | 'draft'>): PrTone {
  if (pr.state == null) return 'unknown';
  if (pr.state === 'MERGED') return 'merged';
  if (pr.state === 'CLOSED') return 'closed';
  return pr.draft ? 'draft' : 'open';
}

export const PR_TONE_LABEL: Record<PrTone, string> = {
  draft: 'draft',
  open: 'open',
  merged: 'merged',
  closed: 'closed',
  unknown: 'unknown',
};

export const CHECKS_LABEL: Record<ChecksState, string> = {
  passing: 'checks passing',
  failing: 'checks failing',
  pending: 'checks running',
  none: 'no checks',
};

export function reviewLabel(review: string | null | undefined): string | null {
  switch (review) {
    case 'APPROVED':
      return 'approved';
    case 'CHANGES_REQUESTED':
      return 'changes requested';
    case 'REVIEW_REQUIRED':
      return 'review required';
    default:
      return null;
  }
}

/** `retry-backoff · feat/retry-backoff · 3 uncommitted · 12 ahead` as separate parts. */
export function worktreeParts(w: Worktree): string[] {
  const parts: string[] = [w.is_main ? `${w.name} (main checkout)` : w.name];
  if (w.branch) parts.push(w.branch);
  else if (w.detached_at) parts.push(`detached at ${w.detached_at}`);
  if (w.removed) {
    parts.push('removed');
    return parts;
  }
  if (w.uncommitted_many) parts.push('many uncommitted');
  else if (w.uncommitted != null && w.uncommitted > 0) parts.push(`${formatCount(w.uncommitted)} uncommitted`);
  if (w.ahead != null && w.ahead > 0) parts.push(`${formatCount(w.ahead)} ahead`);
  if (w.behind != null && w.behind > 0) parts.push(`${formatCount(w.behind)} behind`);
  return parts;
}
