<script lang="ts">
  import { ActionState } from '../lib/action.svelte';
  import { apiPath } from '../lib/api';
  import { relativeTime, shortDuration } from '../lib/format';
  import type { ChatSummary, MoveTarget } from '../lib/types';
  import ErrorLine from './ErrorLine.svelte';
  import Icon from './Icon.svelte';
  import MoveTo from './MoveTo.svelte';
  import StatusDot from './StatusDot.svelte';

  let {
    chat,
    now,
    targets = null,
    variant = 'full',
    showBlock = false,
    focused = false,
    bridgeDown = false,
  }: {
    chat: ChatSummary;
    now: number;
    /** Blocks for the "Move to…" menu, or null to leave the menu out. */
    targets?: MoveTarget[] | null;
    variant?: 'full' | 'compact';
    showBlock?: boolean;
    focused?: boolean;
    bridgeDown?: boolean;
  } = $props();

  const action = new ActionState();

  const resumable = $derived(chat.open_in_iterm === true || chat.can_resume !== false);
  const canOpen = $derived(resumable && !bridgeDown);
  const openHint = $derived(
    bridgeDown
      ? 'iTerm2 not connected'
      : !resumable
        ? (chat.resume_note ?? 'This chat can’t be resumed')
        : chat.open_in_iterm
          ? 'Go to this chat’s tab'
          : 'Resume this chat in iTerm2',
  );

  const timeLabel = $derived.by(() => {
    if (chat.status === 'needs_you' && chat.status_since) return `waiting ${shortDuration(chat.status_since, now)}`;
    if (chat.status === 'working' && chat.status_since) return `working ${shortDuration(chat.status_since, now)}`;
    return relativeTime(chat.last_activity, now);
  });

  const showKeepAwake = $derived(variant === 'full' && (chat.status === 'working' || chat.keep_awake === true));

  function open(): void {
    if (!canOpen) {
      action.fail(openHint);
      return;
    }
    void action.run(apiPath('chats', chat.id, 'open'));
  }

  function toggleAwake(): void {
    void action.run(apiPath('chats', chat.id, 'keep-awake'), { on: chat.keep_awake !== true });
  }
</script>

<li class="chat v-{variant}" class:focused class:needs={chat.status === 'needs_you'} aria-current={focused ? 'true' : undefined}>
  <div class="head">
    <StatusDot status={chat.status} background={chat.background === true} />
    <button
      type="button"
      class="text-btn title"
      onclick={open}
      aria-disabled={canOpen ? undefined : 'true'}
      title={openHint}
    >
      <span class="ellipsis title-text">{chat.title || '(untitled chat)'}</span>
    </button>
    {#if chat.keep_awake}
      <span class="awake" title="Keeping the Mac awake until this chat is done"><Icon name="moon" label="Keeping awake" /></span>
    {/if}
    <span class="time tiny muted">{timeLabel}</span>
  </div>

  <div class="meta small muted">
    <span class="sep-list">
      {#if showBlock}
        <span class="ellipsis block-name">{chat.block_title ?? 'Unsorted'}</span>
      {/if}
      {#if chat.worktree}
        <span class="mono ellipsis">{chat.worktree}</span>
      {/if}
      {#if variant === 'full' && chat.branch}
        <span class="mono ellipsis">{chat.branch}</span>
      {/if}
      {#if variant === 'full' && chat.now_on}
        <span class="now-on">now on <span class="mono">{chat.now_on}</span></span>
      {/if}
      {#if chat.background}
        <span>background work</span>
      {/if}
      {#if chat.hidden}
        <span>hidden</span>
      {/if}
    </span>
    {#if targets && variant === 'compact'}
      <MoveTo {chat} {targets} {action} />
    {/if}
  </div>

  {#if variant === 'full' && (chat.group_reason || targets)}
    <div class="reason-row">
      <p class="reason tiny muted">{chat.group_reason ?? ''}</p>
      {#if targets}
        <MoveTo {chat} {targets} {action} />
      {/if}
    </div>
  {/if}
  {#if !resumable && chat.resume_note}
    <p class="note">{chat.resume_note}</p>
  {/if}

  {#if showKeepAwake}
    <div class="actions">
      <button
        type="button"
        class="btn small"
        aria-pressed={chat.keep_awake === true}
        onclick={toggleAwake}
        disabled={action.busy}
        title={chat.keep_awake ? 'Stop keeping the Mac awake for this chat' : 'Keep the Mac awake until this chat is done'}
      >
        <Icon name="moon" />
        {chat.keep_awake ? 'Keeping awake' : 'Keep awake until done'}
      </button>
    </div>
  {/if}

  <ErrorLine message={action.error} ondismiss={() => action.clear()} />
</li>

<style>
  .chat {
    display: flex;
    flex-direction: column;
    gap: 0.1em;
    padding: var(--pad-s) var(--pad-s) var(--pad-s) calc(var(--pad-s) + 2px);
    border-radius: var(--radius);
    border-left: 2px solid transparent;
    min-width: 0;
  }

  .chat:hover {
    background: var(--hover);
  }

  .chat.focused {
    background: var(--selected);
    border-left-color: var(--accent);
  }

  .chat.needs {
    border-left-color: var(--needs-you);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 0.5em;
    min-width: 0;
  }

  .title {
    flex: 1;
    min-width: 0;
    display: flex;
  }

  .title-text {
    font-weight: 550;
  }

  .title:hover:not([aria-disabled='true']) .title-text {
    color: var(--accent);
  }

  .title[aria-disabled='true'] .title-text {
    color: var(--muted);
  }

  .awake {
    display: inline-flex;
    color: var(--accent);
  }

  .time {
    flex: none;
    font-variant-numeric: tabular-nums;
  }

  .meta {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    padding-left: 1.1em;
    min-width: 0;
  }

  .meta .sep-list {
    flex: 1;
    flex-wrap: nowrap;
    overflow: hidden;
  }

  .meta .sep-list > * {
    flex: 0 1 auto;
  }

  .block-name {
    color: var(--text);
    opacity: 0.85;
  }

  .now-on {
    color: var(--working);
    white-space: nowrap;
  }

  .note {
    padding-left: 1.1em;
  }

  .reason-row {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    padding-left: 1.1em;
    min-width: 0;
  }

  .reason {
    flex: 1;
    min-width: 0;
    font-style: italic;
    opacity: 0.9;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    padding-left: 1.1em;
    margin-top: 0.2em;
    flex-wrap: wrap;
  }

  .v-full .meta .sep-list {
    flex-wrap: wrap;
  }
</style>
