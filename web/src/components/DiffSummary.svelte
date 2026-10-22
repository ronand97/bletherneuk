<script lang="ts">
  import { formatCount, plural } from '../lib/format';
  import { PARALLEL_NOTE } from '../lib/timeline';
  import type { DiffResponse, RangeKind } from '../lib/types';
  import Icon from './Icon.svelte';

  let {
    diff,
    requested,
    stale = false,
    onreload,
  }: {
    diff: DiffResponse;
    /** The range the page asked for; the service may fall back to another. */
    requested: RangeKind;
    stale?: boolean;
    onreload: () => void;
  } = $props();
</script>

<div class="summary">
  <span class="range-label small">
    <span class="muted">{diff.range.from_label}</span>
    <span class="muted" aria-hidden="true">→</span>
    <span class="sr-only">to</span>
    <span class="muted">{diff.range.to_label}</span>
  </span>
  <span class="totals small">
    <span>{plural(diff.totals.files, 'file')}</span>
    <span class="add">+{formatCount(diff.totals.additions)}</span>
    <span class="del">−{formatCount(diff.totals.deletions)}</span>
  </span>
  {#if requested === 'review' && diff.range.kind === 'branch'}
    <span class="tiny muted">No review yet, so this is the whole branch.</span>
  {/if}
  {#if stale}
    <button type="button" class="btn small stale" onclick={onreload}>Newer changes, reload</button>
  {/if}
</div>
{#if diff.parallel}
  <p class="parallel small" role="note"><Icon name="alert" /> {PARALLEL_NOTE}</p>
{/if}

<style>
  .summary {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--gap-s) var(--gap);
    padding: var(--pad-s) var(--pad);
  }

  .range-label {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 0.4em;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .totals {
    display: inline-flex;
    gap: 0.6em;
    font-variant-numeric: tabular-nums;
  }

  .stale {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
    color: var(--accent);
  }

  .parallel {
    display: flex;
    align-items: center;
    gap: 0.4em;
    margin: 0 var(--pad) var(--pad-s);
    padding: var(--pad-xs) var(--pad-s);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--working) 10%, transparent);
    color: var(--working);
  }
</style>
