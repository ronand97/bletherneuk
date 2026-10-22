<script lang="ts">
  import { onMount } from 'svelte';
  import { describeLoadError, getJSON } from '../lib/api';
  import { byLastActive, chatsByActivity, moveTargets } from '../lib/board';
  import { clock } from '../lib/clock.svelte';
  import { connection } from '../lib/connection.svelte';
  import { createRefresher } from '../lib/refresher';
  import type { Board, SearchResult } from '../lib/types';
  import BlockCardView from '../components/BlockCardView.svelte';
  import ChatRow from '../components/ChatRow.svelte';
  import ConnectionOverlay from '../components/ConnectionOverlay.svelte';
  import ErrorLine from '../components/ErrorLine.svelte';
  import Icon from '../components/Icon.svelte';
  import BoardView from './BoardView.svelte';

  let board = $state<Board | null>(null);
  let archive = $state<Board | null>(null);
  let view = $state<'active' | 'archive'>('active');
  let loadError = $state<string | null>(null);

  let query = $state('');
  let results = $state<SearchResult | null>(null);
  let searchError = $state<string | null>(null);
  let searching = $state(false);
  let searchSeq = 0;

  const targets = $derived(board ? moveTargets(board.blocks) : []);
  const bridgeDown = $derived(connection.bridgeDown);
  const trimmed = $derived(query.trim());

  async function load(): Promise<void> {
    try {
      const next = await getJSON<Board>('/api/board?archived=0');
      board = next;
      connection.setBridge(next.iterm_connected);
      if (view === 'archive') archive = await getJSON<Board>('/api/board?archived=1');
      loadError = null;
    } catch (err) {
      loadError = describeLoadError(err);
    }
    if (trimmed !== '') void runSearch(trimmed);
  }

  const refresher = createRefresher(load);

  async function runSearch(q: string): Promise<void> {
    const seq = ++searchSeq;
    searching = true;
    try {
      const res = await getJSON<SearchResult>(`/api/search?q=${encodeURIComponent(q)}`);
      if (seq !== searchSeq) return;
      results = res;
      searchError = null;
    } catch (err) {
      if (seq !== searchSeq) return;
      searchError = describeLoadError(err);
    } finally {
      if (seq === searchSeq) searching = false;
    }
  }

  // Search as you type, a moment after typing stops.
  $effect(() => {
    const q = trimmed;
    if (q === '') {
      searchSeq += 1;
      results = null;
      searchError = null;
      searching = false;
      return;
    }
    const timer = setTimeout(() => void runSearch(q), 180);
    return () => clearTimeout(timer);
  });

  function setView(next: 'active' | 'archive'): void {
    view = next;
    if (next === 'archive') void refresher.now();
  }

  function onSearchKey(ev: KeyboardEvent): void {
    if (ev.key === 'Escape') query = '';
  }

  onMount(() => {
    void refresher.now();
    const offs = [
      connection.on('changed', (ev) => {
        const shown = new Set((view === 'archive' ? archive : board)?.blocks.map((b) => b.id) ?? []);
        if (ev.board || (ev.blocks ?? []).some((id) => shown.has(id))) refresher.request();
      }),
      connection.on('reopen', () => refresher.request()),
    ];
    return () => {
      offs.forEach((off) => off());
      refresher.dispose();
    };
  });
</script>

<div class="page">
  <header class="topbar">
    <h1 class="brand">bletherneuk</h1>
    <div class="search">
      <Icon name="search" />
      <input
        class="input"
        type="search"
        placeholder="Search blocks and chats"
        aria-label="Search blocks and chats, including archived ones"
        bind:value={query}
        onkeydown={onSearchKey}
        autocomplete="off"
        spellcheck="false"
      />
    </div>
    <div class="views" role="group" aria-label="Which blocks to show">
      <button type="button" class="btn small" aria-pressed={view === 'active'} onclick={() => setView('active')}>
        Active
      </button>
      <button type="button" class="btn small" aria-pressed={view === 'archive'} onclick={() => setView('archive')}>
        <Icon name="archive" /> Archive
      </button>
    </div>
  </header>

  <main class="content">
    <ErrorLine message={loadError} />

    {#if trimmed !== ''}
      <section class="section" aria-labelledby="results-title" aria-busy={searching}>
        <div class="section-head">
          <h2 class="section-title" id="results-title">Search results</h2>
          {#if searching}<span class="count">searching…</span>{/if}
        </div>
        <ErrorLine message={searchError} />
        {#if results}
          {#if results.blocks.length === 0 && results.chats.length === 0}
            <p class="empty">Nothing matches “{trimmed}”.</p>
          {/if}
          {#if results.blocks.length > 0}
            <div class="grid">
              {#each byLastActive(results.blocks) as card (card.id)}
                <BlockCardView {card} now={clock.now} {targets} {bridgeDown} />
              {/each}
            </div>
          {/if}
          {#if results.chats.length > 0}
            <ul class="chat-results">
              {#each chatsByActivity(results.chats) as chat (chat.id)}
                <ChatRow {chat} now={clock.now} {targets} {bridgeDown} variant="compact" showBlock />
              {/each}
            </ul>
          {/if}
        {/if}
      </section>
    {:else if view === 'archive'}
      {#if archive}
        <BoardView board={archive} now={clock.now} {targets} {bridgeDown} archived />
      {:else if !loadError}
        <p class="empty">Loading the archive…</p>
      {/if}
    {:else if board}
      <BoardView {board} now={clock.now} {targets} {bridgeDown} />
    {:else if !loadError}
      <p class="empty">Loading…</p>
    {/if}
  </main>

  <ConnectionOverlay />
</div>

<style>
  .page {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    position: sticky;
    top: 0;
    z-index: 10;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--gap) calc(var(--gap) * 1.5);
    padding: var(--pad) calc(var(--pad) * 1.6);
    border-bottom: 1px solid var(--line-soft);
    background: color-mix(in srgb, var(--background) 92%, transparent);
    backdrop-filter: blur(6px);
  }

  .brand {
    font-size: 1.05em;
    font-weight: 700;
    letter-spacing: 0.01em;
  }

  .search {
    position: relative;
    flex: 1 1 16em;
    max-width: 30em;
    display: flex;
    align-items: center;
    color: var(--muted);
  }

  .search :global(.icon-inline) {
    position: absolute;
    left: 0.6em;
    pointer-events: none;
  }

  .search .input {
    width: 100%;
    padding-left: 2em;
  }

  .views {
    display: inline-flex;
    gap: var(--gap-s);
    margin-left: auto;
  }

  .content {
    flex: 1;
    width: 100%;
    max-width: 1400px;
    margin: 0 auto;
    padding: calc(var(--pad) * 1.6);
    display: flex;
    flex-direction: column;
    gap: calc(var(--gap) * 1.5);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 300px), 1fr));
    gap: var(--gap);
    align-items: start;
  }

  .chat-results {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: 52em;
  }

  @media (max-width: 520px) {
    .topbar {
      padding: var(--pad-s) var(--pad);
    }
    .content {
      padding: var(--pad);
    }
  }
</style>
