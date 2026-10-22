<script lang="ts">
  import { ActionState } from '../lib/action.svelte';
  import { apiPath } from '../lib/api';
  import { CHECKS_LABEL, reviewLabel } from '../lib/format';
  import { hostOf, linkKindLabel, safeHttpUrl } from '../lib/links';
  import type { Link } from '../lib/types';
  import ErrorLine from './ErrorLine.svelte';
  import Icon from './Icon.svelte';
  import PrBadge from './PrBadge.svelte';

  let { link }: { link: Link } = $props();

  const action = new ActionState();
  const href = $derived(safeHttpUrl(link.url));
  const host = $derived(hostOf(link.url));
  const review = $derived(reviewLabel(link.status?.review));

  // The toolbelt panel must never navigate: every click asks the service to open the link.
  function open(ev: MouseEvent): void {
    ev.preventDefault();
    if (href === null) return;
    void action.run(apiPath('links', link.id, 'open'));
  }

  function remove(): void {
    void action.run(apiPath('links', link.id, 'delete'));
  }
</script>

<li class="link">
  <span class="kind tiny">{linkKindLabel(link.kind)}</span>
  <div class="body">
    <div class="line">
      {#if href !== null}
        <a class="title ellipsis" {href} onclick={open} onauxclick={open} rel="noreferrer noopener" title={href}>
          {link.title || host}
        </a>
      {:else}
        <span class="title ellipsis blocked" title="Only http and https links can be opened">{link.title || '(link)'}</span>
      {/if}
      <span class="host tiny muted ellipsis">{host ?? 'not an http(s) link'}</span>
    </div>
    {#if link.status || link.added_by_claude}
      <div class="line status">
        {#if link.status}
          <PrBadge state={link.status.state} draft={link.status.draft} checks={link.status.checks} />
          {#if link.status.checks && link.status.checks !== 'none' && Object.hasOwn(CHECKS_LABEL, link.status.checks)}
            <span class="tiny muted">{CHECKS_LABEL[link.status.checks]}</span>
          {/if}
          {#if review}
            <span class="tiny muted">{review}</span>
          {/if}
        {/if}
        {#if link.added_by_claude}
          <span class="chip">added by Claude</span>
        {/if}
      </div>
    {/if}
    <ErrorLine message={action.error} ondismiss={() => action.clear()} />
  </div>
  <button type="button" class="btn ghost icon remove" onclick={remove} disabled={action.busy} title="Remove this link" aria-label="Remove link">
    <Icon name="x" />
  </button>
</li>

<style>
  .link {
    display: flex;
    align-items: flex-start;
    gap: var(--gap-s);
    padding: var(--pad-xs) var(--pad-s);
    border-radius: var(--radius);
    min-width: 0;
  }

  .link:hover {
    background: var(--hover);
  }

  .kind {
    flex: none;
    min-width: 3.8em;
    padding-top: 0.25em;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-weight: 600;
  }

  .body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.15em;
  }

  .line {
    display: flex;
    align-items: baseline;
    gap: 0.5em;
    min-width: 0;
  }

  .title {
    flex: 0 1 auto;
    min-width: 2em;
    color: var(--text);
    font-weight: 500;
  }

  a.title:hover {
    color: var(--accent);
  }

  .blocked {
    color: var(--muted);
    text-decoration: line-through;
  }

  /* The host is always shown in full (design section 11) unless it is very long. */
  .host {
    flex: 0 0 auto;
    max-width: 55%;
  }

  .status {
    align-items: center;
    flex-wrap: wrap;
  }

  .remove {
    opacity: 0;
    width: 1.6em;
    height: 1.6em;
  }

  .link:hover .remove,
  .remove:focus-visible {
    opacity: 1;
  }

  @media (hover: none) {
    .remove {
      opacity: 1;
    }
  }
</style>
