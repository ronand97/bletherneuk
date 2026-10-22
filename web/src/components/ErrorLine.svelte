<script lang="ts">
  import Icon from './Icon.svelte';

  let { message, ondismiss = null }: { message: string | null; ondismiss?: (() => void) | null } = $props();
</script>

{#if message}
  <p class="error-line" role="alert">
    <Icon name="alert" />
    <span class="text">{message}</span>
    {#if ondismiss}
      <button type="button" class="btn ghost icon dismiss" onclick={ondismiss} aria-label="Dismiss">
        <Icon name="x" />
      </button>
    {/if}
  </p>
{/if}

<style>
  .text {
    flex: 1;
    min-width: 0;
  }

  .dismiss {
    width: 1.5em;
    height: 1.5em;
    margin-top: -0.15em;
  }
</style>
