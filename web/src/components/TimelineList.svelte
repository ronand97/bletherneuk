<script lang="ts">
  import { clockTime } from '../lib/format';
  import { PARALLEL_NOTE, promptPreview, turnState, turnStats } from '../lib/timeline';
  import type { TimelineItem } from '../lib/timeline';
  import type { Turn } from '../lib/types';
  import Icon from './Icon.svelte';

  let {
    items,
    selectedTurn = null,
    worktreeNames,
    showWorktree = false,
    onselect,
  }: {
    items: TimelineItem[];
    selectedTurn?: number | null;
    worktreeNames: ReadonlyMap<number, string>;
    showWorktree?: boolean;
    onselect: (turn: Turn) => void;
  } = $props();
</script>

{#if items.length === 0}
  <p class="empty pad">No turns yet. Each prompt you send in this block’s chats shows up here.</p>
{:else}
  <ol class="timeline">
    {#each items as item (item.key)}
      {#if item.kind === 'day'}
        <li class="day">{item.label}</li>
      {:else if item.kind === 'review'}
        <li class="review" class:latest={item.latest}>
          <span class="rule" aria-hidden="true"></span>
          <span class="label">
            <Icon name="check" />
            Reviewed
            {#if showWorktree && worktreeNames.has(item.review.worktree)}
              <span class="mono">{worktreeNames.get(item.review.worktree)}</span>
            {/if}
            <span class="muted">{clockTime(item.review.created_at)}</span>
          </span>
          <span class="rule" aria-hidden="true"></span>
        </li>
      {:else}
        {@const turn = item.turn}
        {@const stats = turnStats(turn)}
        {@const state = turnState(turn)}
        <li class="turn" class:unreviewed={item.unreviewed}>
          <button
            type="button"
            class="text-btn entry"
            aria-pressed={selectedTurn === turn.id}
            onclick={() => onselect(turn)}
            title="Show this turn’s diff"
          >
            <span class="head">
              <span class="time">{clockTime(turn.started_at)}</span>
              <span class="chat ellipsis">{turn.chat_title || '(untitled chat)'}</span>
              {#if showWorktree && turn.worktree != null && worktreeNames.has(turn.worktree)}
                <span class="wt mono ellipsis">{worktreeNames.get(turn.worktree)}</span>
              {/if}
            </span>
            <span class="prompt" class:muted={turn.prompt == null}>{promptPreview(turn.prompt)}</span>
            <span class="meta tiny">
              {#if stats.files}<span class="muted">{stats.files}</span>{/if}
              {#if stats.added}<span class="add">{stats.added}</span>{/if}
              {#if stats.removed}<span class="del">{stats.removed}</span>{/if}
              {#if state}
                <span class="state st-{state.tone}">{state.text}</span>
              {/if}
            </span>
            {#if state?.detail}
              <span class="detail tiny muted">{state.detail}</span>
            {/if}
            {#if turn.parallel}
              <span class="parallel tiny">{PARALLEL_NOTE}</span>
            {/if}
          </button>
        </li>
      {/if}
    {/each}
  </ol>
{/if}

<style>
  .pad {
    padding: var(--pad);
  }

  .timeline {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: var(--pad-s);
  }

  .day {
    position: sticky;
    top: 0;
    z-index: 1;
    padding: var(--pad-s) var(--pad-s) var(--pad-xs);
    background: var(--background);
    color: var(--muted);
    font-size: 0.76em;
    font-weight: 650;
    letter-spacing: 0.07em;
    text-transform: uppercase;
  }

  .review {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    padding: var(--pad-xs) var(--pad-s);
    color: var(--done);
    font-size: 0.82em;
  }

  .review:not(.latest) {
    color: var(--muted);
  }

  .review .label {
    display: inline-flex;
    align-items: center;
    gap: 0.4em;
    white-space: nowrap;
  }

  .rule {
    flex: 1;
    height: 1px;
    background: currentColor;
    opacity: 0.4;
  }

  .turn {
    border-left: 2px solid transparent;
    border-radius: var(--radius);
  }

  .turn.unreviewed {
    border-left-color: color-mix(in srgb, var(--accent) 70%, transparent);
  }

  .entry {
    display: flex;
    flex-direction: column;
    gap: 0.2em;
    padding: var(--pad-s);
    border-radius: var(--radius);
  }

  .entry:hover {
    background: var(--hover);
  }

  .entry[aria-pressed='true'] {
    background: var(--selected);
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 0.5em;
    min-width: 0;
    font-size: 0.88em;
  }

  .time {
    flex: none;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .chat {
    flex: 0 1 auto;
    font-weight: 600;
  }

  .wt {
    flex: 0 1 auto;
    margin-left: auto;
    color: var(--muted);
    font-size: 0.9em;
  }

  .prompt {
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
    line-height: 1.4;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.7em;
    font-family: var(--font-code), ui-monospace, Menlo, monospace;
    font-variant-numeric: tabular-nums;
  }

  .state {
    font-family: var(--font-ui), system-ui, sans-serif;
    padding: 0 0.45em;
    border-radius: 999px;
    border: 1px solid var(--line-soft);
  }

  .st-running {
    color: var(--working);
  }

  .st-missed {
    color: var(--needs-you);
  }

  .st-deleted {
    color: var(--muted);
  }

  .detail {
    overflow-wrap: anywhere;
  }

  .parallel {
    color: var(--working);
    line-height: 1.35;
  }
</style>
