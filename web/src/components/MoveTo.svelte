<script lang="ts">
  import type { ActionState } from '../lib/action.svelte';
  import { apiPath } from '../lib/api';
  import type { ChatSummary, MoveTarget } from '../lib/types';

  let { chat, targets, action }: { chat: ChatSummary; targets: MoveTarget[]; action: ActionState } = $props();

  let choice = $state('');
  const options = $derived(targets.filter((t) => t.id !== chat.block));

  async function move(): Promise<void> {
    const value = choice;
    choice = '';
    if (value === '') return;
    const block = value === 'unsorted' ? null : Number(value.slice(2));
    if (block !== null && !Number.isSafeInteger(block)) return;
    await action.run(apiPath('chats', chat.id, 'move'), { block });
  }
</script>

<select
  class="select ghost small move"
  bind:value={choice}
  onchange={move}
  disabled={action.busy}
  aria-label="Move this chat to another block"
  title="Move to another block"
>
  <option value="" disabled>Move to…</option>
  {#each options as target (target.id)}
    <option value={`b:${target.id}`}>{target.title}</option>
  {/each}
  {#if chat.block != null}
    <option value="unsorted">Unsorted</option>
  {/if}
</select>

<style>
  .move {
    max-width: 9.5em;
    font-size: 0.86em;
  }
</style>
