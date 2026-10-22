<script lang="ts">
  import { plural } from '../lib/format';
  import Icon from './Icon.svelte';

  let {
    bridgeDown = false,
    keepAwake = null,
  }: { bridgeDown?: boolean; keepAwake?: { holding: boolean; chats: number } | null } = $props();
</script>

{#if bridgeDown}
  <p class="banner" role="status">
    <Icon name="terminal" />
    <span><strong>iTerm2 not connected.</strong> Opening and resuming chats will work again once it reconnects.</span>
  </p>
{/if}
{#if keepAwake && keepAwake.holding && keepAwake.chats > 0}
  <p class="banner tone-awake" role="status">
    <Icon name="moon" />
    <span>Keeping awake for {plural(keepAwake.chats, 'chat')}</span>
  </p>
{/if}
