<script lang="ts">
  import { tick } from 'svelte';
  import { ActionState } from '../lib/action.svelte';
  import { apiPath } from '../lib/api';
  import { plural, worktreeParts } from '../lib/format';
  import { safeHttpUrl } from '../lib/links';
  import type { BlockDetail, ChatId, FileItem, MoveTarget } from '../lib/types';
  import ChatRow from '../components/ChatRow.svelte';
  import ErrorLine from '../components/ErrorLine.svelte';
  import FileRow from '../components/FileRow.svelte';
  import Icon from '../components/Icon.svelte';
  import LinkRow from '../components/LinkRow.svelte';
  import NewChatMenu from '../components/NewChatMenu.svelte';

  let {
    detail,
    now,
    focusChat = null,
    targets,
    bridgeDown = false,
  }: {
    detail: BlockDetail;
    now: number;
    focusChat?: ChatId | null;
    targets: MoveTarget[];
    bridgeDown?: boolean;
  } = $props();

  const uid = $props.id();
  const block = $derived(detail.block);
  const focusWorktree = $derived(detail.chats.find((c) => c.id === focusChat)?.worktree ?? null);
  const working = $derived(detail.chats.filter((c) => c.status === 'working').length);

  // Rename -------------------------------------------------------------------
  const renameAction = new ActionState();
  let renaming = $state(false);
  let newTitle = $state('');
  let titleInput = $state<HTMLInputElement | null>(null);

  async function startRename(): Promise<void> {
    newTitle = block.title;
    renaming = true;
    await tick();
    titleInput?.select();
  }

  async function saveRename(ev: SubmitEvent): Promise<void> {
    ev.preventDefault();
    const title = newTitle.trim();
    const ok = await renameAction.run(apiPath('blocks', block.id, 'rename'), { title: title === '' ? null : title });
    if (ok) renaming = false;
  }

  function onRenameKey(ev: KeyboardEvent): void {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      renaming = false;
      renameAction.clear();
    }
  }

  // Links --------------------------------------------------------------------
  const linkAction = new ActionState();
  let newLink = $state('');

  async function addLink(ev: SubmitEvent): Promise<void> {
    ev.preventDefault();
    const url = safeHttpUrl(newLink);
    if (url === null) {
      linkAction.fail('Only http:// and https:// links can be added.');
      return;
    }
    const ok = await linkAction.run(apiPath('blocks', block.id, 'links'), { url, title: null });
    if (ok) newLink = '';
  }

  // Block actions --------------------------------------------------------------
  const awakeAction = new ActionState();
  const fileAction = new ActionState();
  const changesAction = new ActionState();

  function toggleAwake(): void {
    void awakeAction.run(apiPath('blocks', block.id, 'keep-awake'), { on: !detail.keep_awake.block });
  }

  function addFile(): void {
    void fileAction.run(apiPath('blocks', block.id, 'add-file'));
  }

  function openChanges(): void {
    void changesAction.run(apiPath('blocks', block.id, 'changes'));
  }

  // Files ----------------------------------------------------------------------
  let showHiddenDocs = $state(false);
  let showHiddenOutputs = $state(false);

  function visible(files: FileItem[], showHidden: boolean): FileItem[] {
    return showHidden ? files : files.filter((f) => !f.hidden);
  }

  const hiddenDocs = $derived(detail.docs.filter((f) => f.hidden).length);
  const hiddenOutputs = $derived(detail.outputs.filter((f) => f.hidden).length);
</script>

<article class="block" aria-labelledby="{uid}-title">
  <header class="head">
    {#if renaming}
      <form class="rename" onsubmit={saveRename}>
        <label class="sr-only" for="{uid}-rename">New title</label>
        <input
          id="{uid}-rename"
          class="input"
          bind:this={titleInput}
          bind:value={newTitle}
          onkeydown={onRenameKey}
          maxlength="200"
          autocomplete="off"
        />
        <div class="rename-actions">
          <button type="submit" class="btn primary small" disabled={renameAction.busy}>Save</button>
          <button type="button" class="btn ghost small" onclick={() => (renaming = false)}>Cancel</button>
          <span class="tiny muted">Leave empty to use the automatic title.</span>
        </div>
      </form>
    {:else}
      <div class="title-row">
        <h1 class="title" id="{uid}-title">{block.title}</h1>
        <button type="button" class="btn ghost icon" onclick={startRename} title="Rename this block" aria-label="Rename block">
          <Icon name="pencil" />
        </button>
      </div>
    {/if}
    <ErrorLine message={renameAction.error} ondismiss={() => renameAction.clear()} />
    <p class="repo small muted sep-list">
      {#if block.repo_name}<span class="mono" title={block.repo ?? undefined}>{block.repo_name}</span>{/if}
      {#if block.title_source === 'rename'}<span>renamed</span>{/if}
      {#if block.archived}<span>archived</span>{/if}
    </p>
  </header>

  {#if bridgeDown}
    <p class="banner" role="status">
      <Icon name="terminal" />
      <span><strong>iTerm2 not connected.</strong> New chats and resuming wait until it reconnects.</span>
    </p>
  {/if}

  <div class="toolbar">
    <NewChatMenu
      blockId={block.id}
      worktrees={detail.worktrees}
      {focusWorktree}
      suggestedName={detail.new_chat_name}
      {bridgeDown}
    />
    <button type="button" class="btn" onclick={addFile} disabled={fileAction.busy} title="Pick a file to add to this block">
      <Icon name="file" /> Add file
    </button>
  </div>
  <ErrorLine message={fileAction.error} ondismiss={() => fileAction.clear()} />

  <div class="awake">
    <button
      type="button"
      class="btn small"
      aria-pressed={detail.keep_awake.block}
      onclick={toggleAwake}
      disabled={awakeAction.busy || (!detail.keep_awake.block && working === 0)}
      title={!detail.keep_awake.block && working === 0 ? 'No chat here is working right now' : undefined}
    >
      <Icon name="moon" />
      {detail.keep_awake.block ? 'Keeping awake until all chats here are done' : 'Keep awake until all chats here are done'}
    </button>
    <p class="tiny muted">Closing the lid still puts most Macs to sleep.</p>
    <ErrorLine message={awakeAction.error} ondismiss={() => awakeAction.clear()} />
  </div>

  <button type="button" class="changes" onclick={openChanges} disabled={changesAction.busy}>
    <Icon name="diff" />
    <span class="changes-text">
      {#if detail.changes_since_review && detail.changes_since_review.files > 0}
        <strong>{plural(detail.changes_since_review.files, 'file')} changed</strong> since your last review
      {:else if detail.changes_since_review}
        No changes since your last review
      {:else}
        Review what changed, turn by turn
      {/if}
    </span>
    <span class="go small">Changes <Icon name="chevron-right" /></span>
  </button>
  <ErrorLine message={changesAction.error} ondismiss={() => changesAction.clear()} />

  <section class="section" aria-labelledby="{uid}-wt">
    <div class="section-head">
      <h2 class="section-title" id="{uid}-wt">Worktrees</h2>
      <span class="count">{detail.worktrees.length}</span>
    </div>
    {#if detail.worktrees.length === 0}
      <p class="empty">No worktrees found for this block.</p>
    {:else}
      <ul class="rows">
        {#each detail.worktrees as w (w.id)}
          <li class="wt small" class:removed={w.removed} title={w.path}>
            <Icon name="branch" />
            <span class="sep-list">
              {#each worktreeParts(w) as part, i (i)}
                <span class:mono={i < 2} class:name={i === 0}>{part}</span>
              {/each}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="section" aria-labelledby="{uid}-links">
    <div class="section-head">
      <h2 class="section-title" id="{uid}-links">Tracking</h2>
      <span class="count">{detail.links.length}</span>
    </div>
    {#if detail.links.length > 0}
      <ul class="rows">
        {#each detail.links as link (link.id)}
          <LinkRow {link} />
        {/each}
      </ul>
    {/if}
    <form class="add-link" onsubmit={addLink} novalidate>
      <label class="sr-only" for="{uid}-link">Add a link</label>
      <input
        id="{uid}-link"
        class="input"
        type="url"
        placeholder="+ link (https://…)"
        bind:value={newLink}
        oninput={() => linkAction.clear()}
        autocomplete="off"
        spellcheck="false"
      />
      {#if newLink.trim() !== ''}
        <button type="submit" class="btn small" disabled={linkAction.busy}>Add</button>
      {/if}
    </form>
    <ErrorLine message={linkAction.error} ondismiss={() => linkAction.clear()} />
  </section>

  <section class="section" aria-labelledby="{uid}-chats">
    <div class="section-head">
      <h2 class="section-title" id="{uid}-chats">Chats</h2>
      <span class="count">{detail.chats.length}</span>
    </div>
    {#if detail.chats.length === 0}
      <p class="empty">No chats yet.</p>
    {:else}
      <ul class="rows">
        {#each detail.chats as chat (chat.id)}
          <ChatRow {chat} {now} {targets} {bridgeDown} focused={chat.id === focusChat} />
        {/each}
      </ul>
    {/if}
  </section>

  {#each [{ key: 'docs', label: 'Docs', files: detail.docs, hidden: hiddenDocs }, { key: 'outputs', label: 'Outputs', files: detail.outputs, hidden: hiddenOutputs }] as group (group.key)}
    {@const showHidden = group.key === 'docs' ? showHiddenDocs : showHiddenOutputs}
    {@const shown = visible(group.files, showHidden)}
    {#if group.files.length > 0}
      <section class="section" aria-labelledby="{uid}-{group.key}">
        <div class="section-head">
          <h2 class="section-title" id="{uid}-{group.key}">{group.label}</h2>
          <span class="count">{group.files.length - group.hidden}</span>
          <span class="spacer"></span>
          {#if group.hidden > 0}
            <button
              type="button"
              class="btn ghost small"
              aria-pressed={showHidden}
              onclick={() => {
                if (group.key === 'docs') showHiddenDocs = !showHiddenDocs;
                else showHiddenOutputs = !showHiddenOutputs;
              }}
            >
              {showHidden ? 'Hide hidden' : `${group.hidden} hidden`}
            </button>
          {/if}
        </div>
        {#if shown.length === 0}
          <p class="empty">All {group.label.toLowerCase()} here are hidden.</p>
        {:else}
          <ul class="rows">
            {#each shown as file (file.id)}
              <FileRow {file} {now} />
            {/each}
          </ul>
        {/if}
      </section>
    {/if}
  {/each}
</article>

<style>
  .block {
    display: flex;
    flex-direction: column;
    gap: calc(var(--gap) * 1.4);
  }

  .head {
    display: flex;
    flex-direction: column;
    gap: 0.15em;
  }

  .title-row {
    display: flex;
    align-items: flex-start;
    gap: var(--gap-s);
  }

  .title {
    flex: 1;
    min-width: 0;
    font-size: 1.15em;
    font-weight: 650;
    line-height: 1.3;
    overflow-wrap: anywhere;
  }

  .title-row .btn {
    opacity: 0.6;
  }

  .title-row:hover .btn,
  .title-row .btn:focus-visible {
    opacity: 1;
  }

  .rename {
    display: flex;
    flex-direction: column;
    gap: var(--gap-s);
  }

  .rename-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--gap-s);
  }

  .toolbar {
    display: flex;
    align-items: flex-start;
    flex-wrap: wrap;
    gap: var(--gap-s);
  }

  .toolbar > :global(.new-chat) {
    flex: 1 1 auto;
  }

  .awake {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.2em;
  }

  .awake .btn {
    white-space: normal;
    text-align: left;
  }

  .changes {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    width: 100%;
    padding: var(--pad-s) var(--pad);
    border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--border));
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--accent) 7%, var(--surface));
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }

  .changes:hover {
    background: color-mix(in srgb, var(--accent) 13%, var(--surface));
  }

  .changes-text {
    flex: 1;
    min-width: 0;
  }

  .go {
    display: inline-flex;
    align-items: center;
    gap: 0.15em;
    color: var(--accent);
    white-space: nowrap;
  }

  .rows {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .wt {
    display: flex;
    align-items: baseline;
    gap: 0.45em;
    padding: 0.1em var(--pad-s);
    color: var(--muted);
  }

  .wt :global(.icon-inline) {
    flex: none;
    align-self: center;
  }

  .wt .name {
    color: var(--text);
    font-weight: 500;
  }

  .wt.removed {
    opacity: 0.6;
  }

  .wt.removed .name {
    text-decoration: line-through;
  }

  .add-link {
    display: flex;
    gap: var(--gap-s);
  }

  .add-link .input {
    flex: 1;
    background: transparent;
    border-style: dashed;
  }

  .add-link .input:focus {
    border-style: solid;
    background: var(--background);
  }
</style>
