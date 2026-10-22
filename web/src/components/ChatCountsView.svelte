<script lang="ts">
  import { STATUS_LABEL, STATUS_ORDER, formatCount } from '../lib/format';
  import type { ChatCounts } from '../lib/types';

  let { counts, compact = false }: { counts: ChatCounts; compact?: boolean } = $props();

  const parts = $derived(STATUS_ORDER.filter((s) => (counts[s] ?? 0) > 0).map((s) => ({ status: s, n: counts[s] })));
</script>

{#if parts.length === 0}
  <span class="counts muted">no chats</span>
{:else}
  <span class="counts">
    {#each parts as part (part.status)}
      <span class="count" title={`${formatCount(part.n)} ${STATUS_LABEL[part.status]}`}>
        <span class="dot s-{part.status}" aria-hidden="true"></span>
        <span class="n">{formatCount(part.n)}</span>
        {#if compact}
          <span class="sr-only">{STATUS_LABEL[part.status]}</span>
        {:else}
          <span class="label">{STATUS_LABEL[part.status]}</span>
        {/if}
      </span>
    {/each}
  </span>
{/if}

<style>
  .counts {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    column-gap: 0.85em;
    row-gap: 0.1em;
    font-size: 0.88em;
  }

  .count {
    display: inline-flex;
    align-items: center;
    gap: 0.35em;
    white-space: nowrap;
  }

  .n {
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }

  .label {
    color: var(--muted);
  }

  .dot.s-working {
    animation: none;
  }
</style>
