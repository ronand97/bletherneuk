<script lang="ts">
  import { ActionState } from '../lib/action.svelte';
  import { apiPath } from '../lib/api';
  import { fileSize, relativeTime } from '../lib/format';
  import type { FileItem } from '../lib/types';
  import ErrorLine from './ErrorLine.svelte';
  import Icon from './Icon.svelte';

  let { file, now }: { file: FileItem; now: number } = $props();

  const action = new ActionState();

  // A missing file with no saved copy can't be opened or revealed.
  const gone = $derived(file.missing === true && file.copy !== true);
  const openable = $derived(file.openable !== false);
  const hint = $derived(
    gone
      ? 'The file is gone and no copy was saved'
      : !openable
        ? 'This type of file is only shown in Finder'
        : file.copy
          ? 'The original is gone; opens the saved copy'
          : 'Open with the default app',
  );
  const details = $derived(
    [fileSize(file.size), file.modified ? `changed ${relativeTime(file.modified, now)}` : '']
      .filter((s) => s !== '')
      .join(' · '),
  );

  function open(): void {
    if (gone) {
      action.fail('This file is gone and no copy was saved.');
      return;
    }
    void action.run(apiPath('files', file.id, openable ? 'open' : 'reveal'));
  }

  function reveal(): void {
    void action.run(apiPath('files', file.id, 'reveal'));
  }

  function toggleHidden(): void {
    void action.run(apiPath('files', file.id, 'hide'), { hidden: file.hidden !== true });
  }
</script>

<li class="file" class:gone class:hidden={file.hidden}>
  <div class="line">
    <button type="button" class="text-btn name" onclick={open} aria-disabled={gone ? 'true' : undefined} title={hint}>
      <Icon name="file" />
      <span class="ellipsis">{file.name}</span>
    </button>
    {#if file.copy}
      <span class="chip tone-warn" title="The original is gone; this opens the copy bletherneuk saved">copy</span>
    {/if}
    {#if gone}
      <span class="chip tone-bad">missing</span>
    {/if}
    {#if !openable && !gone}
      <span class="chip" title="Only shown in Finder">Finder</span>
    {/if}
    <span class="tools">
      {#if !gone}
        <button type="button" class="btn ghost icon" onclick={reveal} disabled={action.busy} title="Show in Finder" aria-label="Show in Finder">
          <Icon name="folder" />
        </button>
      {/if}
      <button
        type="button"
        class="btn ghost icon"
        onclick={toggleHidden}
        disabled={action.busy}
        title={file.hidden ? 'Show this file in the list again' : 'Hide this file'}
        aria-label={file.hidden ? 'Unhide' : 'Hide'}
      >
        <Icon name={file.hidden ? 'eye' : 'eye-off'} />
      </button>
    </span>
  </div>
  <div class="folder tiny muted">
    <span class="ellipsis mono" title={file.folder}>{file.folder}</span>
    {#if details}<span class="details">{details}</span>{/if}
  </div>
  <ErrorLine message={action.error} ondismiss={() => action.clear()} />
</li>

<style>
  .file {
    display: flex;
    flex-direction: column;
    padding: var(--pad-xs) var(--pad-s);
    border-radius: var(--radius);
    min-width: 0;
  }

  .file:hover {
    background: var(--hover);
  }

  .file.hidden {
    opacity: 0.6;
  }

  .line {
    display: flex;
    align-items: center;
    gap: 0.45em;
    min-width: 0;
  }

  .name {
    flex: 0 1 auto;
    display: inline-flex;
    align-items: center;
    gap: 0.45em;
    width: auto;
    color: var(--text);
    font-weight: 500;
  }

  .name :global(.icon-inline) {
    color: var(--muted);
  }

  .name:hover:not([aria-disabled='true']) {
    color: var(--accent);
  }

  .gone .name {
    color: var(--muted);
  }

  .tools {
    display: inline-flex;
    margin-left: auto;
    opacity: 0;
  }

  .tools .btn {
    width: 1.6em;
    height: 1.6em;
  }

  .file:hover .tools,
  .file:focus-within .tools {
    opacity: 1;
  }

  @media (hover: none) {
    .tools {
      opacity: 1;
    }
  }

  .folder {
    display: flex;
    gap: 0.6em;
    padding-left: 1.5em;
    min-width: 0;
  }

  .folder .ellipsis {
    flex: 1 1 auto;
  }

  .details {
    flex: none;
  }
</style>
