<script lang="ts">
  import { connection } from '../lib/connection.svelte';

  let now = $state(Date.now());
  const seconds = $derived(Math.max(0, Math.ceil((connection.retryAt - now) / 1000)));

  $effect(() => {
    if (connection.state !== 'reconnecting') return;
    now = Date.now();
    const timer = setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => clearInterval(timer);
  });
</script>

{#if connection.state === 'reconnecting'}
  <div class="overlay" role="status" aria-live="polite">
    <div class="box">
      <span class="spinner" aria-hidden="true"></span>
      <div>
        <p class="title">Reconnecting to bletherneuk…</p>
        <p class="note">
          The background service isn’t answering.
          {#if seconds > 1}Trying again in {seconds}s.{:else}Trying again now.{/if}
        </p>
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: calc(var(--pad) * 3) var(--pad);
    background: color-mix(in srgb, var(--background) 72%, transparent);
    backdrop-filter: blur(2px);
  }

  .box {
    display: flex;
    align-items: flex-start;
    gap: var(--gap);
    max-width: 26em;
    padding: var(--pad) calc(var(--pad) * 1.3);
    border: 1px solid var(--border);
    border-radius: calc(var(--radius) * 1.4);
    background: var(--surface);
    box-shadow: 0 8px 30px color-mix(in srgb, black 25%, transparent);
  }

  .title {
    font-weight: 600;
  }

  .spinner {
    flex: none;
    width: 1em;
    height: 1em;
    margin-top: 0.2em;
    border-radius: 50%;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    animation: spin 0.9s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation: none;
    }
  }
</style>
