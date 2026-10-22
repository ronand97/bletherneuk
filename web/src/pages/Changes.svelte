<script lang="ts">
  import { onMount } from 'svelte';
  import { ActionState } from '../lib/action.svelte';
  import { apiPath, describeLoadError, getJSON } from '../lib/api';
  import { clock } from '../lib/clock.svelte';
  import { connection } from '../lib/connection.svelte';
  import { createRefresher } from '../lib/refresher';
  import { defaultWorktree, mergeTimeline, withDays } from '../lib/timeline';
  import type { BlockDetail, BlockId, DiffResponse, RangeKind, Timeline, Turn } from '../lib/types';
  import ConnectionOverlay from '../components/ConnectionOverlay.svelte';
  import DiffSummary from '../components/DiffSummary.svelte';
  import DiffView from '../components/DiffView.svelte';
  import ErrorLine from '../components/ErrorLine.svelte';
  import Icon from '../components/Icon.svelte';
  import NotInDiff from '../components/NotInDiff.svelte';
  import TimelineList from '../components/TimelineList.svelte';

  let { blockId }: { blockId: BlockId | null } = $props();

  let detail = $state<BlockDetail | null>(null);
  let timeline = $state<Timeline | null>(null);
  let diff = $state<DiffResponse | null>(null);
  let loadError = $state<string | null>(null);
  let diffError = $state<string | null>(null);
  let diffLoading = $state(false);

  let worktree = $state<number | null>(null);
  let range = $state<RangeKind>('review');
  let turnId = $state<number | null>(null);
  let showTimeline = $state(true);

  // What the shown diff was loaded against, to tell when newer changes have arrived.
  let diffSignature = $state('');
  // The range the shown diff is for. The diff view starts afresh when this changes.
  let shownKey = $state('');
  let shownRange = $state<RangeKind>('review');
  let diffSeq = 0;

  const reviewAction = new ActionState();

  const worktreeNames = $derived(new Map((detail?.worktrees ?? []).map((w) => [w.id, w.name])));
  const items = $derived(
    timeline ? withDays(mergeTimeline(timeline.turns, timeline.reviews, worktree), clock.now) : [],
  );
  const currentSignature = $derived(signature(timeline, worktree, range, turnId));
  const stale = $derived(diff !== null && diffSignature !== '' && currentSignature !== diffSignature);
  // The newest finished turn here, which "One turn" shows when no turn is picked yet.
  const latestTurnHere = $derived.by(() => {
    const here = (timeline?.turns ?? [])
      .filter((t) => worktree === null || t.worktree === worktree)
      .sort((a, b) => b.started_at - a.started_at);
    return here.find((t) => t.ended_at != null) ?? here[0] ?? null;
  });

  function requestKey(): string {
    return `${worktree}|${range}|${range === 'turn' ? turnId : ''}`;
  }

  /**
   * A summary of what a diff depends on; it changes whenever the diff could. For one turn,
   * that's the turn itself. Otherwise it's every turn and review in the worktree.
   */
  function signature(t: Timeline | null, wt: number | null, r: RangeKind, turn: number | null): string {
    if (!t) return '';
    const turnKey = (x: Turn) => `${x.id}:${x.ended_at ?? '-'}:${x.files_changed ?? '-'}`;
    if (r === 'turn') {
      const picked = t.turns.find((x) => x.id === turn);
      return picked ? turnKey(picked) : '';
    }
    const turns = t.turns
      .filter((x) => wt === null || x.worktree === wt)
      .map(turnKey)
      .join(',');
    const reviews = t.reviews
      .filter((x) => wt === null || x.worktree === wt)
      .map((x) => x.id)
      .join(',');
    return `${turns}|${reviews}`;
  }

  async function loadDetail(): Promise<void> {
    if (blockId === null) return;
    try {
      detail = await getJSON<BlockDetail>(apiPath('blocks', blockId));
      loadError = null;
    } catch (err) {
      loadError = describeLoadError(err);
    }
  }

  async function loadTimeline(): Promise<void> {
    if (blockId === null) return;
    try {
      timeline = await getJSON<Timeline>(apiPath('blocks', blockId, 'timeline'));
      loadError = null;
    } catch (err) {
      loadError = describeLoadError(err);
    }
  }

  async function loadDiff(): Promise<void> {
    if (blockId === null) return;
    const seq = ++diffSeq;
    const params = new URLSearchParams();
    if (worktree !== null) params.set('worktree', String(worktree));
    params.set('range', range);
    if (range === 'turn' && turnId !== null) params.set('turn', String(turnId));
    const sig = signature(timeline, worktree, range, turnId);
    const key = requestKey();
    const requested = range;
    diffLoading = true;
    try {
      const next = await getJSON<DiffResponse>(`${apiPath('blocks', blockId, 'diff')}?${params}`);
      if (seq !== diffSeq) return;
      diff = next;
      diffSignature = sig;
      shownKey = key;
      shownRange = requested;
      diffError = null;
    } catch (err) {
      if (seq !== diffSeq) return;
      diffError = describeLoadError(err);
    } finally {
      if (seq === diffSeq) diffLoading = false;
    }
  }

  const refresher = createRefresher(async () => {
    await Promise.all([loadDetail(), loadTimeline()]);
    // With nothing on screen to lose, newer changes are shown straight away.
    if (stale && (diff === null || diff.unavailable || diff.files.length === 0)) await loadDiff();
  });

  function selectTurn(turn: Turn): void {
    turnId = turn.id;
    range = 'turn';
    if (turn.worktree != null) worktree = turn.worktree;
    void loadDiff();
  }

  function setRange(next: RangeKind): void {
    if (next === 'turn') {
      const pick = turnId !== null ? timeline?.turns.find((t) => t.id === turnId) : latestTurnHere;
      if (!pick) return;
      selectTurn(pick);
      return;
    }
    range = next;
    void loadDiff();
  }

  function setWorktree(ev: Event): void {
    const id = Number((ev.currentTarget as HTMLSelectElement).value);
    if (!Number.isSafeInteger(id)) return;
    worktree = id;
    if (range === 'turn') {
      const turn = timeline?.turns.find((t) => t.id === turnId);
      if (!turn || turn.worktree !== id) {
        range = 'review';
        turnId = null;
      }
    }
    void loadDiff();
  }

  async function markReviewed(): Promise<void> {
    if (blockId === null || worktree === null) return;
    // Mark exactly what's on screen as reviewed, not whatever the worktree holds now.
    const ok = await reviewAction.run(apiPath('blocks', blockId, 'review'), {
      worktree,
      tree: diff?.to_tree ?? null,
    });
    if (!ok) return;
    await loadTimeline();
    range = 'review';
    await loadDiff();
  }

  onMount(() => {
    void (async () => {
      await Promise.all([loadDetail(), loadTimeline()]);
      if (detail && timeline) {
        worktree = defaultWorktree(timeline.turns, detail.changes_since_review, detail.worktrees);
      }
      await loadDiff();
    })();
    const offs = [
      connection.on('changed', (ev) => {
        if (blockId !== null && (ev.blocks ?? []).includes(blockId)) refresher.request();
      }),
      connection.on('reopen', () => refresher.request()),
    ];
    return () => {
      offs.forEach((off) => off());
      refresher.dispose();
    };
  });
</script>

<div class="changes">
  <header class="bar">
    <div class="title-line">
      <h1 class="title ellipsis">{detail?.block.title ?? 'Changes'}</h1>
      {#if detail}<span class="small muted">Changes</span>{/if}
    </div>
    <div class="controls">
      <div class="seg" role="group" aria-label="Range">
        <button type="button" class="btn small" aria-pressed={range === 'review'} onclick={() => setRange('review')}>
          Since last review
        </button>
        <button
          type="button"
          class="btn small"
          aria-pressed={range === 'turn'}
          onclick={() => setRange('turn')}
          disabled={!latestTurnHere && turnId === null}
          title="Pick a turn in the timeline"
        >
          One turn
        </button>
        <button type="button" class="btn small" aria-pressed={range === 'branch'} onclick={() => setRange('branch')}>
          Whole branch
        </button>
      </div>
      {#if detail && detail.worktrees.length > 1}
        <label class="sr-only" for="worktree-pick">Worktree</label>
        <select id="worktree-pick" class="select wt-pick mono" value={worktree === null ? '' : String(worktree)} onchange={setWorktree}>
          {#each detail.worktrees as w (w.id)}
            <option value={String(w.id)}>{w.name}{w.branch ? ` · ${w.branch}` : ''}{w.removed ? ' (removed)' : ''}</option>
          {/each}
        </select>
      {:else if detail && detail.worktrees.length === 1}
        <span class="mono small muted ellipsis">{detail.worktrees[0].name}</span>
      {/if}
      <span class="spacer"></span>
      <button
        type="button"
        class="btn small timeline-toggle"
        aria-pressed={showTimeline}
        aria-controls="timeline-pane"
        onclick={() => (showTimeline = !showTimeline)}
      >
        Timeline
      </button>
      <button
        type="button"
        class="btn primary"
        onclick={markReviewed}
        disabled={reviewAction.busy || worktree === null || range === 'turn' || stale || diffLoading}
        title={range === 'turn'
          ? 'Switch to “Since last review” or “Whole branch” to mark everything up to now as reviewed'
          : stale
            ? 'Newer changes arrived; reload the diff first'
            : 'Move the review marker for this worktree to now'}
      >
        <Icon name="check" /> Mark reviewed
      </button>
    </div>
    <ErrorLine message={reviewAction.error} ondismiss={() => reviewAction.clear()} />
    <ErrorLine message={loadError} />
  </header>

  {#if blockId === null}
    <p class="empty pad">This page needs a block. Open it from a block’s panel.</p>
  {:else}
    <div class="body" class:no-timeline={!showTimeline}>
      {#if showTimeline}
        <aside class="timeline-pane" id="timeline-pane" aria-label="Timeline">
          {#if timeline}
            <TimelineList
              {items}
              selectedTurn={range === 'turn' ? turnId : null}
              {worktreeNames}
              showWorktree={(detail?.worktrees.length ?? 0) > 1}
              onselect={selectTurn}
            />
          {:else if !loadError}
            <p class="empty pad">Loading timeline…</p>
          {/if}
        </aside>
      {/if}

      <section class="diff-pane" class:loading={diffLoading && diff !== null} aria-label="Diff" aria-busy={diffLoading}>
        {#if diff}
          <DiffSummary {diff} requested={shownRange} {stale} onreload={() => void loadDiff()} />
        {/if}
        <ErrorLine message={diffError} />

        {#if diff}
          {#if diff.unavailable}
            <div class="state-box">
              <p>{diff.unavailable}</p>
              <NotInDiff items={diff.not_in_diff} />
            </div>
          {:else if diff.files.length === 0}
            <div class="state-box">
              <p class="muted">
                {shownRange === 'review' ? 'Nothing has changed since your last review.' : 'No changes in this range.'}
              </p>
              <NotInDiff items={diff.not_in_diff} />
            </div>
          {:else}
            {#key shownKey}
              <DiffView files={diff.files}>
                {#snippet footer()}
                  <NotInDiff items={diff?.not_in_diff ?? []} />
                {/snippet}
              </DiffView>
            {/key}
          {/if}
        {:else if diffLoading}
          <p class="empty pad">Loading diff…</p>
        {/if}
      </section>
    </div>
  {/if}

  <ConnectionOverlay />
</div>

<style>
  .changes {
    height: 100vh;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .bar {
    display: flex;
    flex-direction: column;
    gap: var(--gap-s);
    padding: var(--pad-s) var(--pad);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .title-line {
    display: flex;
    align-items: baseline;
    gap: var(--gap-s);
    min-width: 0;
  }

  .title {
    font-size: 1.05em;
    font-weight: 650;
  }

  .controls {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--gap-s);
  }

  .seg {
    display: inline-flex;
  }

  .seg .btn {
    border-radius: 0;
  }

  .seg .btn + .btn {
    margin-left: -1px;
  }

  .seg .btn:first-child {
    border-radius: var(--radius) 0 0 var(--radius);
  }

  .seg .btn:last-child {
    border-radius: 0 var(--radius) var(--radius) 0;
  }

  .wt-pick {
    max-width: 18em;
    font-size: 0.86em;
  }

  .spacer {
    flex: 1;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(220px, min(32%, 380px)) minmax(0, 1fr);
  }

  .body.no-timeline {
    grid-template-columns: minmax(0, 1fr);
  }

  .timeline-pane {
    min-height: 0;
    overflow: auto;
    border-right: 1px solid var(--border);
  }

  .diff-pane {
    min-height: 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    transition: opacity 0.15s ease;
  }

  .diff-pane.loading {
    opacity: 0.6;
  }

  @media (prefers-reduced-motion: reduce) {
    .diff-pane {
      transition: none;
    }
  }

  .state-box {
    padding: var(--pad);
    overflow: auto;
  }

  .pad {
    padding: var(--pad);
  }

  @media (max-width: 720px) {
    .body {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: minmax(0, 34%) minmax(0, 1fr);
    }

    .body.no-timeline {
      grid-template-rows: minmax(0, 1fr);
    }

    .timeline-pane {
      border-right: 0;
      border-bottom: 1px solid var(--border);
    }
  }
</style>
