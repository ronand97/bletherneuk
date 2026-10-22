<script lang="ts">
  import { CHECKS_LABEL, PR_TONE_LABEL, prTone } from '../lib/format';
  import type { ChecksState } from '../lib/types';
  import Icon from './Icon.svelte';

  let {
    number = null,
    state,
    draft = false,
    checks = null,
  }: {
    number?: number | null;
    state: string | null | undefined;
    draft?: boolean;
    checks?: ChecksState | null;
  } = $props();

  const knownState = $derived(state === 'OPEN' || state === 'MERGED' || state === 'CLOSED' ? state : null);
  const tone = $derived(prTone({ state: knownState, draft }));
  const checksLabel = $derived(
    checks && checks !== 'none' && Object.hasOwn(CHECKS_LABEL, checks) ? CHECKS_LABEL[checks] : null,
  );
</script>

<span class="pr tone-{tone}" title={checksLabel ? `${PR_TONE_LABEL[tone]}, ${checksLabel}` : PR_TONE_LABEL[tone]}>
  {#if number !== null}<span class="num">#{number}</span>{/if}
  <span class="state">{PR_TONE_LABEL[tone]}</span>
  {#if checksLabel}
    <span class="checks c-{checks}">
      {#if checks === 'passing'}
        <Icon name="check" />
      {:else if checks === 'failing'}
        <Icon name="x" />
      {:else}
        <span class="pending-dot" aria-hidden="true"></span>
      {/if}
      <span class="sr-only">{checksLabel}</span>
    </span>
  {/if}
</span>

<style>
  .pr {
    display: inline-flex;
    align-items: center;
    gap: 0.35em;
    padding: 0 0.5em;
    border-radius: 999px;
    border: 1px solid var(--line-soft);
    font-size: 0.84em;
    line-height: 1.65;
    white-space: nowrap;
    color: var(--muted);
  }

  .num {
    font-variant-numeric: tabular-nums;
    color: var(--text);
    font-weight: 600;
  }

  .tone-open {
    border-color: color-mix(in srgb, var(--done) 45%, transparent);
    background: color-mix(in srgb, var(--done) 10%, transparent);
  }

  .tone-open .state {
    color: var(--done);
  }

  .tone-merged {
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }

  .tone-merged .state {
    color: var(--accent);
  }

  .tone-closed .state {
    color: var(--needs-you);
  }

  .tone-draft .state,
  .tone-unknown .state {
    color: var(--muted);
  }

  .checks {
    display: inline-flex;
    align-items: center;
  }

  .c-passing {
    color: var(--done);
  }

  .c-failing {
    color: var(--needs-you);
  }

  .c-pending {
    color: var(--working);
  }

  .pending-dot {
    width: 0.5em;
    height: 0.5em;
    border-radius: 50%;
    background: currentColor;
  }
</style>
