<script lang="ts">
  import type { NotInDiffItem } from '../lib/types';

  let { items }: { items: NotInDiffItem[] } = $props();
  const uid = $props.id();
</script>

{#if items.length > 0}
  <section class="not-in-diff" aria-labelledby="{uid}-title">
    <h2 class="section-title" id="{uid}-title">Not in this diff</h2>
    <p class="note">“Mark reviewed” only covers what the diff shows. These changes aren’t in it.</p>
    <ul>
      {#each items as item, i (i)}
        <li>
          <span class="path mono">{item.path}</span>
          <span class="reason small muted">{item.reason}</span>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .not-in-diff {
    display: flex;
    flex-direction: column;
    gap: var(--gap-s);
    margin: calc(var(--gap) * 2) var(--pad) var(--pad);
    padding: var(--pad);
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    max-width: 60em;
  }

  ul {
    display: flex;
    flex-direction: column;
    gap: 0.3em;
  }

  li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    column-gap: 0.8em;
  }

  .path {
    overflow-wrap: anywhere;
  }
</style>
