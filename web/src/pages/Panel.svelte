<script lang="ts">
  import { onMount } from 'svelte';
  import { apiPath, describeLoadError, getJSON } from '../lib/api';
  import { moveTargets } from '../lib/board';
  import { clock } from '../lib/clock.svelte';
  import { connection } from '../lib/connection.svelte';
  import { createRefresher } from '../lib/refresher';
  import type { BlockDetail, BlockId, Board, ChatId, Focus } from '../lib/types';
  import ConnectionOverlay from '../components/ConnectionOverlay.svelte';
  import ErrorLine from '../components/ErrorLine.svelte';
  import CompactBoard from './CompactBoard.svelte';
  import PanelBlock from './PanelBlock.svelte';

  let board = $state<Board | null>(null);
  let boardError = $state<string | null>(null);
  let focusBlock = $state<BlockId | null>(null);
  let focusChat = $state<ChatId | null>(null);
  let focusKnown = false;
  let detail = $state<BlockDetail | null>(null);
  let detailError = $state<string | null>(null);

  const targets = $derived(board ? moveTargets(board.blocks) : []);
  const bridgeDown = $derived(connection.bridgeDown);
  const stale = $derived(detail !== null && detail.block.id !== focusBlock);

  async function loadBoard(): Promise<void> {
    try {
      const next = await getJSON<Board>('/api/board?archived=0');
      board = next;
      boardError = null;
      connection.setBridge(next.iterm_connected);
      // Until the first `focus` event, the board says which tab is focused.
      if (!focusKnown && next.focus) setFocus(next.focus, false);
    } catch (err) {
      boardError = describeLoadError(err);
    }
  }

  async function loadDetail(): Promise<void> {
    const id = focusBlock;
    if (id === null) return;
    try {
      const next = await getJSON<BlockDetail>(apiPath('blocks', id));
      if (focusBlock !== id) return;
      detail = next;
      detailError = null;
    } catch (err) {
      if (focusBlock !== id) return;
      detailError = describeLoadError(err);
    }
  }

  const boardRefresher = createRefresher(loadBoard);
  const detailRefresher = createRefresher(loadDetail);

  function setFocus(focus: Focus, fromEvent: boolean): void {
    if (fromEvent) focusKnown = true;
    focusChat = focus.chat ?? null;
    const block = focus.block ?? null;
    if (block === focusBlock) return;
    focusBlock = block;
    detailError = null;
    if (block === null) {
      detail = null;
    } else {
      void detailRefresher.now();
    }
  }

  onMount(() => {
    void boardRefresher.now();
    const offs = [
      connection.on('focus', (focus) => setFocus(focus, true)),
      connection.on('changed', (ev) => {
        if (ev.board) boardRefresher.request();
        if (focusBlock !== null && (ev.blocks ?? []).includes(focusBlock)) detailRefresher.request();
      }),
      connection.on('reopen', () => {
        boardRefresher.request();
        detailRefresher.request();
      }),
    ];
    return () => {
      offs.forEach((off) => off());
      boardRefresher.dispose();
      detailRefresher.dispose();
    };
  });
</script>

<main class="panel">
  {#if focusBlock !== null && detail !== null}
    <div class="detail" class:stale aria-busy={stale}>
      <PanelBlock {detail} now={clock.now} {focusChat} {targets} {bridgeDown} />
    </div>
  {:else if focusBlock !== null && detailError === null}
    <p class="empty">Loading…</p>
  {:else if board}
    {#if detailError}
      <ErrorLine message={detailError} />
    {/if}
    <CompactBoard {board} now={clock.now} {targets} {bridgeDown} />
  {:else if !boardError}
    <p class="empty">Loading…</p>
  {/if}
  {#if focusBlock !== null && detail !== null}
    <ErrorLine message={detailError} />
  {/if}
  <ErrorLine message={boardError} />
  <ConnectionOverlay />
</main>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
    padding: var(--pad);
    min-width: 0;
    max-width: 640px;
  }

  .detail {
    transition: opacity 0.15s ease;
  }

  .detail.stale {
    opacity: 0.55;
  }

  @media (prefers-reduced-motion: reduce) {
    .detail {
      transition: none;
    }
  }
</style>
