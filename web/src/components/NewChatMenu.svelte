<script lang="ts">
  import { tick } from 'svelte';
  import { ActionState } from '../lib/action.svelte';
  import { apiPath } from '../lib/api';
  import { worktreeNameProblem } from '../lib/names';
  import type { BlockId, Worktree } from '../lib/types';
  import ErrorLine from './ErrorLine.svelte';
  import Icon from './Icon.svelte';

  let {
    blockId,
    worktrees,
    focusWorktree = null,
    suggestedName,
    bridgeDown = false,
  }: {
    blockId: BlockId;
    worktrees: Worktree[];
    /** Name of the worktree the focused chat is in, if any. */
    focusWorktree?: string | null;
    suggestedName: string;
    bridgeDown?: boolean;
  } = $props();

  const uid = $props.id();
  const action = new ActionState();
  let open = $state(false);
  let naming = $state(false);
  let name = $state('');
  let touched = $state(false);
  let trigger = $state<HTMLButtonElement | null>(null);
  let menu = $state<HTMLDivElement | null>(null);
  let nameInput = $state<HTMLInputElement | null>(null);

  const linked = $derived(
    worktrees
      .filter((w) => !w.is_main && !w.removed)
      .sort((a, b) => Number(b.name === focusWorktree) - Number(a.name === focusWorktree)),
  );
  const hasMain = $derived(worktrees.some((w) => w.is_main && !w.removed) || worktrees.length === 0);
  const problem = $derived(worktreeNameProblem(name));

  async function toggle(): Promise<void> {
    if (bridgeDown) {
      action.fail('iTerm2 not connected');
      return;
    }
    open = !open;
    naming = false;
    if (open) {
      await tick();
      menu?.querySelector<HTMLButtonElement>('button')?.focus();
    }
  }

  async function close(): Promise<void> {
    open = false;
    naming = false;
    await tick();
    trigger?.focus();
  }

  async function start(where: 'worktree' | 'main' | 'new_worktree', worktree: number | null = null): Promise<void> {
    if (where === 'new_worktree') {
      touched = true;
      if (problem !== null) return;
    }
    const ok = await action.run(apiPath('blocks', blockId, 'new-chat'), {
      where,
      worktree,
      name: where === 'new_worktree' ? name : null,
    });
    if (ok) {
      open = false;
      naming = false;
    }
  }

  async function askName(): Promise<void> {
    naming = true;
    touched = false;
    name = suggestedName;
    await tick();
    nameInput?.select();
  }

  // Arrow keys move between the menu's buttons; Escape closes it.
  function onMenuKey(ev: KeyboardEvent): void {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      void close();
      return;
    }
    if (ev.key !== 'ArrowDown' && ev.key !== 'ArrowUp') return;
    const items = [...(menu?.querySelectorAll<HTMLButtonElement>('button.item') ?? [])];
    const index = items.indexOf(ev.currentTarget as HTMLButtonElement);
    if (index < 0 || items.length === 0) return;
    ev.preventDefault();
    const next = (index + (ev.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
    items[next].focus();
  }

  function onNameKey(ev: KeyboardEvent): void {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      void close();
    }
  }

  function submitName(ev: SubmitEvent): void {
    ev.preventDefault();
    void start('new_worktree');
  }
</script>

<div class="new-chat">
  <button
    type="button"
    class="btn primary"
    bind:this={trigger}
    onclick={toggle}
    aria-expanded={open}
    aria-controls="{uid}-menu"
    aria-disabled={bridgeDown ? 'true' : undefined}
    title={bridgeDown ? 'iTerm2 not connected' : 'Start a new Claude Code chat for this block'}
  >
    <Icon name="plus" /> New chat <Icon name={open ? 'chevron-down' : 'chevron-right'} />
  </button>

  {#if open}
    <div class="menu" id="{uid}-menu" bind:this={menu}>
      {#each linked as w (w.id)}
        <button type="button" class="item" onclick={() => start('worktree', w.id)} onkeydown={onMenuKey} disabled={action.busy}>
          <span class="label">
            {w.name === focusWorktree ? 'This worktree' : 'In worktree'}
            <span class="mono">{w.name}</span>
          </span>
          {#if w.branch}<span class="sub mono tiny muted ellipsis">{w.branch}</span>{/if}
        </button>
      {/each}
      {#if hasMain}
        <button type="button" class="item" onclick={() => start('main')} onkeydown={onMenuKey} disabled={action.busy}>
          <span class="label">In the main checkout</span>
        </button>
      {/if}
      <button type="button" class="item" onclick={askName} onkeydown={onMenuKey} aria-expanded={naming} disabled={action.busy}>
        <span class="label">In a new worktree…</span>
        <span class="sub tiny muted">runs <span class="mono">claude --worktree</span></span>
      </button>

      {#if naming}
        <form class="name-form" onsubmit={submitName}>
          <label class="tiny muted" for="{uid}-name">Worktree name</label>
          <div class="name-row">
            <input
              id="{uid}-name"
              class="input mono"
              bind:this={nameInput}
              bind:value={name}
              oninput={() => (touched = true)}
              onkeydown={onNameKey}
              maxlength="64"
              autocomplete="off"
              spellcheck="false"
              aria-invalid={touched && problem !== null ? 'true' : undefined}
              aria-describedby="{uid}-name-help"
            />
            <button type="submit" class="btn primary" disabled={action.busy || problem !== null}>Start</button>
          </div>
          <p class="tiny" class:muted={!(touched && problem)} class:del={touched && problem !== null} id="{uid}-name-help">
            {touched && problem ? problem : 'Starts with a letter or digit; then letters, digits, dots, hyphens and underscores; up to 64.'}
          </p>
        </form>
      {/if}
    </div>
  {/if}
  <ErrorLine message={action.error} ondismiss={() => action.clear()} />
</div>

<style>
  .new-chat {
    display: flex;
    flex-direction: column;
    gap: var(--gap-s);
    min-width: 0;
  }

  .new-chat > .btn {
    align-self: flex-start;
  }

  .menu {
    display: flex;
    flex-direction: column;
    padding: var(--pad-xs);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: 0 6px 20px color-mix(in srgb, black 18%, transparent);
  }

  .item {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0;
    width: 100%;
    padding: var(--pad-xs) var(--pad-s);
    border: 0;
    border-radius: calc(var(--radius) * 0.7);
    background: none;
    text-align: left;
    cursor: pointer;
    min-width: 0;
  }

  .item:hover,
  .item:focus-visible {
    background: var(--hover);
  }

  .item[aria-expanded='true'] {
    background: var(--selected);
  }

  .item:disabled {
    cursor: progress;
    opacity: 0.6;
  }

  .sub {
    max-width: 100%;
  }

  .name-form {
    display: flex;
    flex-direction: column;
    gap: 0.25em;
    padding: var(--pad-s);
  }

  .name-row {
    display: flex;
    gap: var(--gap-s);
  }

  .name-row .input {
    flex: 1;
  }
</style>
