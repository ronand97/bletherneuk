<script lang="ts">
  import { byLastActive, chatsByActivity, needsYouOrder } from '../lib/board';
  import type { Board, MoveTarget } from '../lib/types';
  import BlockCardView from '../components/BlockCardView.svelte';
  import ChatRow from '../components/ChatRow.svelte';
  import StatusBanners from '../components/StatusBanners.svelte';

  let {
    board,
    now,
    targets,
    bridgeDown = false,
    archived = false,
  }: { board: Board; now: number; targets: MoveTarget[]; bridgeDown?: boolean; archived?: boolean } = $props();

  const needsYou = $derived(needsYouOrder(board.needs_you));
  const blocks = $derived(byLastActive(board.blocks));
  const unsorted = $derived(chatsByActivity(board.unsorted));
</script>

<div class="board-view">
  {#if !archived}
    <StatusBanners {bridgeDown} keepAwake={board.keep_awake} />
  {/if}

  {#if needsYou.length > 0}
    <section class="section needs" aria-labelledby="needs-title">
      <div class="section-head">
        <h2 class="section-title" id="needs-title">Needs you</h2>
        <span class="count">{needsYou.length}</span>
      </div>
      <ul class="needs-list">
        {#each needsYou as chat (chat.id)}
          <ChatRow {chat} {now} {targets} {bridgeDown} variant="compact" showBlock />
        {/each}
      </ul>
    </section>
  {/if}

  <section class="section" aria-labelledby="blocks-title">
    <div class="section-head">
      <h2 class="section-title" id="blocks-title">{archived ? 'Archived blocks' : 'Blocks'}</h2>
      <span class="count">{blocks.length}</span>
    </div>
    {#if blocks.length === 0 && unsorted.length === 0}
      <p class="empty">
        {archived
          ? 'Nothing archived yet. Blocks are archived when their PR is merged or closed and they’ve been idle for a while.'
          : 'No work blocks yet. Start a Claude Code chat in iTerm2 and it will show up here.'}
      </p>
    {/if}
    <div class="grid">
      {#each blocks as card (card.id)}
        <BlockCardView {card} {now} {targets} {bridgeDown} />
      {/each}
      {#if !archived && unsorted.length > 0}
        <section class="unsorted" aria-labelledby="unsorted-title">
          <div class="section-head">
            <h3 class="unsorted-title" id="unsorted-title">Unsorted</h3>
            <span class="count">{unsorted.length}</span>
          </div>
          <p class="note">Chats with no branch, PR or ticket to group them by. Use “Move to…” to file them.</p>
          <ul>
            {#each unsorted as chat (chat.id)}
              <ChatRow {chat} {now} {targets} {bridgeDown} variant="compact" />
            {/each}
          </ul>
        </section>
      {/if}
    </div>
  </section>
</div>

<style>
  .board-view {
    display: flex;
    flex-direction: column;
    gap: calc(var(--gap) * 2);
  }

  .needs-list {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 280px), 1fr));
    gap: var(--gap-s);
  }

  .needs-list :global(li) {
    border: 1px solid color-mix(in srgb, var(--needs-you) 30%, var(--border));
    background: color-mix(in srgb, var(--needs-you) 6%, var(--surface));
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 300px), 1fr));
    gap: var(--gap);
    align-items: start;
  }

  .unsorted {
    display: flex;
    flex-direction: column;
    gap: var(--gap-s);
    padding: var(--pad);
    border: 1px dashed var(--border);
    border-radius: calc(var(--radius) * 1.4);
    min-width: 0;
  }

  .unsorted-title {
    font-size: 1.05em;
    font-weight: 600;
  }
</style>
