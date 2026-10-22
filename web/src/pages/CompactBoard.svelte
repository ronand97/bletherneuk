<script lang="ts">
  import { ActionState } from '../lib/action.svelte';
  import { apiPath } from '../lib/api';
  import { byLastActive, chatsByActivity, needsYouOrder } from '../lib/board';
  import { relativeTime } from '../lib/format';
  import type { Board, BlockCard, MoveTarget } from '../lib/types';
  import ChatCountsView from '../components/ChatCountsView.svelte';
  import ChatRow from '../components/ChatRow.svelte';
  import ErrorLine from '../components/ErrorLine.svelte';
  import Icon from '../components/Icon.svelte';
  import PrBadge from '../components/PrBadge.svelte';
  import StatusBanners from '../components/StatusBanners.svelte';

  let {
    board,
    now,
    targets,
    bridgeDown = false,
  }: { board: Board; now: number; targets: MoveTarget[]; bridgeDown?: boolean } = $props();

  const uid = $props.id();
  const action = new ActionState();
  const needsYou = $derived(needsYouOrder(board.needs_you));
  const blocks = $derived(byLastActive(board.blocks));
  const unsorted = $derived(chatsByActivity(board.unsorted));

  function openBoard(): void {
    void action.run('/api/board/open');
  }

  function openBlock(card: BlockCard): void {
    if (bridgeDown) {
      action.fail('iTerm2 not connected');
      return;
    }
    void action.run(apiPath('blocks', card.id, 'open'));
  }
</script>

<div class="compact">
  <header class="head">
    <h1 class="brand">bletherneuk</h1>
    <button type="button" class="btn small" onclick={openBoard} disabled={action.busy} title="Open the board in its own window">
      <Icon name="board" /> Board
    </button>
  </header>
  <p class="tiny muted">This tab isn’t part of a work block.</p>
  <ErrorLine message={action.error} ondismiss={() => action.clear()} />

  <StatusBanners {bridgeDown} keepAwake={board.keep_awake} />

  {#if needsYou.length > 0}
    <section class="section" aria-labelledby="{uid}-needs">
      <div class="section-head">
        <h2 class="section-title" id="{uid}-needs">Needs you</h2>
        <span class="count">{needsYou.length}</span>
      </div>
      <ul class="rows">
        {#each needsYou as chat (chat.id)}
          <ChatRow {chat} {now} {targets} {bridgeDown} variant="compact" showBlock />
        {/each}
      </ul>
    </section>
  {/if}

  <section class="section" aria-labelledby="{uid}-blocks">
    <div class="section-head">
      <h2 class="section-title" id="{uid}-blocks">Blocks</h2>
      <span class="count">{blocks.length}</span>
    </div>
    {#if blocks.length === 0}
      <p class="empty">No work blocks yet.</p>
    {:else}
      <ul class="rows">
        {#each blocks as card (card.id)}
          <li>
            <button
              type="button"
              class="text-btn block-row"
              onclick={() => openBlock(card)}
              aria-disabled={bridgeDown ? 'true' : undefined}
              title={bridgeDown ? 'iTerm2 not connected' : 'Open this block’s window'}
            >
              <span class="line">
                <span class="title ellipsis">{card.title}</span>
                <span class="tiny muted time">{relativeTime(card.last_active, now)}</span>
              </span>
              <span class="line sub">
                {#if card.pr}
                  <PrBadge number={card.pr.number} state={card.pr.state} draft={card.pr.draft} checks={card.pr.checks} />
                {:else if card.ticket}
                  <span class="chip code">{card.ticket}</span>
                {/if}
                <ChatCountsView counts={card.chats} compact />
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if unsorted.length > 0}
    <section class="section" aria-labelledby="{uid}-unsorted">
      <div class="section-head">
        <h2 class="section-title" id="{uid}-unsorted">Unsorted</h2>
        <span class="count">{unsorted.length}</span>
      </div>
      <ul class="rows">
        {#each unsorted as chat (chat.id)}
          <ChatRow {chat} {now} {targets} {bridgeDown} variant="compact" />
        {/each}
      </ul>
    </section>
  {/if}
</div>

<style>
  .compact {
    display: flex;
    flex-direction: column;
    gap: calc(var(--gap) * 1.3);
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--gap-s);
  }

  .head + p {
    margin-top: calc(var(--gap) * -1);
  }

  .brand {
    font-size: 1.05em;
    font-weight: 700;
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .block-row {
    display: flex;
    flex-direction: column;
    gap: 0.2em;
    padding: var(--pad-s);
    border-radius: var(--radius);
  }

  .block-row:hover {
    background: var(--hover);
  }

  .block-row:hover:not([aria-disabled='true']) .title {
    color: var(--accent);
  }

  .line {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    min-width: 0;
  }

  .title {
    flex: 1;
    font-weight: 550;
  }

  .time {
    flex: none;
  }

  .sub {
    flex-wrap: wrap;
  }
</style>
