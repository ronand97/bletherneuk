<script lang="ts">
  import { onMount } from 'svelte';
  import { ActionState } from '../lib/action.svelte';
  import { apiPath, describeLoadError, getJSON } from '../lib/api';
  import { connection } from '../lib/connection.svelte';
  import { plural, relativeTime, totalChats } from '../lib/format';
  import type { BlockCard, BlockDetail, MoveTarget } from '../lib/types';
  import ChatCountsView from './ChatCountsView.svelte';
  import ChatRow from './ChatRow.svelte';
  import ErrorLine from './ErrorLine.svelte';
  import Icon from './Icon.svelte';
  import PrBadge from './PrBadge.svelte';

  let {
    card,
    now,
    targets,
    bridgeDown = false,
  }: { card: BlockCard; now: number; targets: MoveTarget[]; bridgeDown?: boolean } = $props();

  const uid = $props.id();
  const action = new ActionState();
  let expanded = $state(false);
  let detail = $state<BlockDetail | null>(null);
  let loadError = $state<string | null>(null);
  let loading = $state(false);

  const total = $derived(totalChats(card.chats));

  async function loadChats(): Promise<void> {
    loading = true;
    try {
      detail = await getJSON<BlockDetail>(apiPath('blocks', card.id));
      loadError = null;
    } catch (err) {
      loadError = describeLoadError(err);
    } finally {
      loading = false;
    }
  }

  function toggle(): void {
    expanded = !expanded;
    if (expanded) void loadChats();
  }

  function open(): void {
    if (bridgeDown) {
      action.fail('iTerm2 not connected');
      return;
    }
    void action.run(apiPath('blocks', card.id, 'open'));
  }

  onMount(() =>
    connection.on('changed', (ev) => {
      if (expanded && (ev.blocks ?? []).includes(card.id)) void loadChats();
    }),
  );
</script>

<article class="card" class:archived={card.archived} aria-labelledby="{uid}-title">
  <button
    type="button"
    class="text-btn main"
    onclick={open}
    aria-disabled={bridgeDown ? 'true' : undefined}
    title={bridgeDown ? 'iTerm2 not connected' : 'Open this block’s window'}
  >
    <span class="row top">
      <span class="title" id="{uid}-title">{card.title}</span>
      <span class="time tiny muted">{relativeTime(card.last_active, now)}</span>
    </span>
    {#if card.pr || card.ticket || card.repo_name}
      <span class="row badges">
        {#if card.pr}
          <PrBadge number={card.pr.number} state={card.pr.state} draft={card.pr.draft} checks={card.pr.checks} />
        {/if}
        {#if card.ticket}
          <span class="chip code">{card.ticket}</span>
        {/if}
        {#if card.repo_name}
          <span class="repo small muted ellipsis">{card.repo_name}</span>
        {/if}
      </span>
    {/if}
    {#if card.worktrees.length > 0}
      <span class="row worktrees">
        <Icon name="branch" />
        <span class="sep-list">
          {#each card.worktrees as name, i (i)}
            <span class="wt mono ellipsis">{name}</span>
          {/each}
        </span>
      </span>
    {/if}
    <span class="row counts">
      <ChatCountsView counts={card.chats} />
    </span>
  </button>

  <div class="foot">
    <button
      type="button"
      class="btn ghost small disclosure"
      aria-expanded={expanded}
      aria-controls="{uid}-chats"
      onclick={toggle}
      disabled={total === 0}
    >
      <Icon name={expanded ? 'chevron-down' : 'chevron-right'} />
      {plural(total, 'chat')}
    </button>
    <span class="files tiny muted">
      <span class="sep-list">
        <span>{plural(card.docs, 'doc')}</span>
        <span>{plural(card.outputs, 'output')}</span>
      </span>
    </span>
  </div>

  {#if expanded}
    <div class="chats" id="{uid}-chats">
      {#if detail}
        <ul>
          {#each detail.chats as chat (chat.id)}
            <ChatRow {chat} {now} {targets} {bridgeDown} variant="compact" />
          {/each}
        </ul>
      {:else if loading}
        <p class="empty">Loading chats…</p>
      {/if}
      <ErrorLine message={loadError} />
    </div>
  {/if}

  <ErrorLine message={action.error} ondismiss={() => action.clear()} />
</article>

<style>
  .card {
    display: flex;
    flex-direction: column;
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: calc(var(--radius) * 1.4);
    background: var(--surface);
    transition: border-color 0.12s ease;
  }

  .card:hover {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
  }

  .card.archived {
    opacity: 0.85;
  }

  .main {
    display: flex;
    flex-direction: column;
    gap: calc(var(--gap) * 0.6);
    padding: var(--pad) var(--pad) var(--pad-s);
    border-radius: calc(var(--radius) * 1.4) calc(var(--radius) * 1.4) 0 0;
  }

  .main:focus-visible {
    outline-offset: -2px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    min-width: 0;
  }

  .top {
    align-items: baseline;
  }

  .title {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    font-size: 1.05em;
    line-height: 1.3;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .main:hover:not([aria-disabled='true']) .title {
    color: var(--accent);
  }

  .time {
    flex: none;
    white-space: nowrap;
  }

  .badges {
    flex-wrap: wrap;
  }

  .repo {
    max-width: 50%;
  }

  .worktrees {
    color: var(--muted);
    flex-wrap: wrap;
    row-gap: 0.15em;
  }

  .wt {
    max-width: 14em;
    font-size: 0.84em;
    color: var(--text);
    opacity: 0.85;
  }

  .foot {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    padding: 0 var(--pad-s) var(--pad-s) calc(var(--pad-s) * 0.6);
  }

  .files {
    margin-left: auto;
  }

  .chats {
    border-top: 1px solid var(--line-soft);
    padding: var(--pad-s);
  }
</style>
