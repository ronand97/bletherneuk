import { describe, expect, it } from 'vitest';
import {
  PROMPT_PREVIEW_CHARS,
  defaultWorktree,
  isUnreviewed,
  latestReviews,
  mergeTimeline,
  promptPreview,
  turnState,
  turnStats,
  withDays,
} from './timeline';
import type { Review, Turn } from './types';

// Local-time helper, so day headings don't depend on the machine's time zone.
const at = (day: number, hour: number, minute = 0) => new Date(2026, 9, day, hour, minute).getTime();

function turn(id: number, startedAt: number, extra: Partial<Turn> = {}): Turn {
  return {
    id,
    chat: `00000000-0000-4000-8000-00000000000${id % 10}`,
    chat_title: `Chat ${id}`,
    prompt: `Prompt ${id}`,
    started_at: startedAt,
    ended_at: startedAt + 60_000,
    worktree: 7,
    files_changed: 1,
    lines_added: 2,
    lines_removed: 1,
    parallel: false,
    missed: null,
    snapshot_deleted: false,
    ...extra,
  };
}

function review(id: number, createdAt: number, worktree = 7): Review {
  return { id, worktree, created_at: createdAt };
}

const keys = (entries: { key: string }[]) => entries.map((e) => e.key);

describe('mergeTimeline', () => {
  it('interleaves turns and review markers newest first', () => {
    const entries = mergeTimeline(
      [turn(1, at(22, 9)), turn(2, at(22, 11)), turn(3, at(22, 13))],
      [review(1, at(22, 10)), review(2, at(22, 12))],
      7,
    );
    expect(keys(entries)).toEqual(['t3', 'r2', 't2', 'r1', 't1']);
  });

  it('sorts lists that arrive out of order', () => {
    const entries = mergeTimeline([turn(1, at(22, 9)), turn(2, at(22, 15))], [review(5, at(22, 8)), review(6, at(22, 12))], 7);
    expect(keys(entries)).toEqual(['t2', 'r6', 't1', 'r5']);
  });

  it('places a turn that starts at the same moment as a review above it', () => {
    const entries = mergeTimeline([turn(1, at(22, 10))], [review(1, at(22, 10))], 7);
    expect(keys(entries)).toEqual(['t1', 'r1']);
  });

  it('keeps the server order for turns that start at the same time', () => {
    const entries = mergeTimeline([turn(4, at(22, 10)), turn(3, at(22, 10))], [], 7);
    expect(keys(entries)).toEqual(['t4', 't3']);
  });

  it('marks only the newest review of each worktree as latest', () => {
    const entries = mergeTimeline([], [review(1, at(22, 9), 7), review(2, at(22, 10), 7), review(3, at(22, 8), 8)], 7);
    const latest = entries.filter((e) => e.kind === 'review' && e.latest).map((e) => e.key);
    expect(latest).toEqual(['r2', 'r3']);
  });

  it('marks turns in the chosen worktree with changes after its latest review', () => {
    const entries = mergeTimeline(
      [
        turn(1, at(22, 9)), // before the review
        turn(2, at(22, 9, 59), { ended_at: at(22, 10, 5) }), // ran across the review
        turn(3, at(22, 11)), // after the review
        turn(4, at(22, 12), { worktree: 8 }), // another worktree
        turn(5, at(22, 13), { ended_at: null }), // still running
      ],
      [review(1, at(22, 10))],
      7,
    );
    const unreviewed = entries.filter((e) => e.kind === 'turn' && e.unreviewed).map((e) => e.key);
    expect(unreviewed).toEqual(['t5', 't3', 't2']);
  });
});

describe('isUnreviewed', () => {
  it('treats every turn in a worktree with no review as unreviewed', () => {
    expect(isUnreviewed(turn(1, at(22, 9)), 7, latestReviews([]))).toBe(true);
  });

  it('never marks turns when no worktree is chosen', () => {
    expect(isUnreviewed(turn(1, at(22, 9)), null, latestReviews([]))).toBe(false);
  });
});

describe('withDays', () => {
  it('adds a heading before the first entry of each day', () => {
    const now = at(22, 18);
    const entries = mergeTimeline([turn(1, at(20, 9)), turn(2, at(21, 9)), turn(3, at(22, 9)), turn(4, at(22, 10))], [], 7);
    const items = withDays(entries, now);
    expect(items.map((i) => (i.kind === 'day' ? i.label : i.key))).toEqual([
      'Today',
      't4',
      't3',
      'Yesterday',
      't2',
      'Tue 20 Oct',
      't1',
    ]);
  });

  it('returns nothing for an empty timeline', () => {
    expect(withDays([], at(22, 18))).toEqual([]);
  });
});

describe('promptPreview', () => {
  it('keeps the first 200 characters on one line', () => {
    const long = 'word '.repeat(100);
    const preview = promptPreview(long);
    expect(preview.endsWith('…')).toBe(true);
    expect(Array.from(preview.replace('…', '')).length).toBeLessThanOrEqual(PROMPT_PREVIEW_CHARS);
    expect(promptPreview('first\n\n  second\tthird')).toBe('first second third');
  });

  it('counts characters, not UTF-16 units', () => {
    const emoji = String.fromCodePoint(0x1f600).repeat(PROMPT_PREVIEW_CHARS);
    expect(promptPreview(emoji)).toBe(emoji);
  });

  it('labels missing and empty prompts', () => {
    expect(promptPreview(null)).toBe('(no prompt text)');
    expect(promptPreview('   ')).toBe('(empty prompt)');
  });
});

describe('turnStats', () => {
  it('formats files changed and lines added and removed', () => {
    expect(turnStats(turn(1, at(22, 9), { files_changed: 3, lines_added: 10, lines_removed: 2 }))).toEqual({
      files: '3 files',
      added: '+10',
      removed: '−2',
    });
    expect(turnStats(turn(1, at(22, 9), { files_changed: 1 })).files).toBe('1 file');
  });

  it('leaves out counts the service doesn’t have', () => {
    expect(turnStats(turn(1, at(22, 9), { files_changed: null, lines_added: null, lines_removed: null }))).toEqual({
      files: null,
      added: null,
      removed: null,
    });
  });
});

describe('turnState', () => {
  it('reports a missed snapshot with its reason', () => {
    expect(turnState(turn(1, at(22, 9), { missed: 'not a git repo' }))).toEqual({
      tone: 'missed',
      text: 'snapshot missed',
      detail: 'not a git repo',
    });
  });

  it('reports a deleted snapshot', () => {
    expect(turnState(turn(1, at(22, 9), { snapshot_deleted: true }))?.text).toBe('snapshot deleted to save space');
  });

  it('reports a turn still in progress', () => {
    expect(turnState(turn(1, at(22, 9), { ended_at: null }))?.tone).toBe('running');
  });

  it('returns null for a finished turn with its snapshots', () => {
    expect(turnState(turn(1, at(22, 9)))).toBeNull();
  });
});

describe('defaultWorktree', () => {
  const worktrees = [{ id: 7 }, { id: 8 }, { id: 9, removed: true }];

  it('picks the worktree of the most recent turn', () => {
    expect(defaultWorktree([turn(1, at(22, 9), { worktree: 7 }), turn(2, at(22, 10), { worktree: 8 })], null, worktrees)).toBe(8);
  });

  it('falls back to the worktree with unreviewed changes, then the first live one', () => {
    expect(defaultWorktree([], { worktree: 8 }, worktrees)).toBe(8);
    expect(defaultWorktree([], null, [{ id: 9, removed: true }, { id: 7 }])).toBe(7);
    expect(defaultWorktree([], null, [])).toBeNull();
  });
});
