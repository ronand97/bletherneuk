<script lang="ts" module>
  export type IconName =
    | 'moon'
    | 'branch'
    | 'folder'
    | 'external'
    | 'eye'
    | 'eye-off'
    | 'pencil'
    | 'plus'
    | 'x'
    | 'chevron-right'
    | 'chevron-down'
    | 'search'
    | 'file'
    | 'check'
    | 'alert'
    | 'terminal'
    | 'diff'
    | 'archive'
    | 'board';
</script>

<script lang="ts">
  let { name, label = null }: { name: IconName; label?: string | null } = $props();
</script>

<svg
  class="icon-inline"
  viewBox="0 0 16 16"
  fill="none"
  stroke="currentColor"
  stroke-width="1.5"
  stroke-linecap="round"
  stroke-linejoin="round"
  role={label ? 'img' : undefined}
  aria-label={label ?? undefined}
  aria-hidden={label ? undefined : 'true'}
  focusable="false"
>
  {#if name === 'moon'}
    <path d="M13.5 9.6A5.5 5.5 0 0 1 6.4 2.5a5.5 5.5 0 1 0 7.1 7.1Z" />
  {:else if name === 'branch'}
    <circle cx="4.5" cy="3.5" r="1.5" /><circle cx="4.5" cy="12.5" r="1.5" /><circle cx="11.5" cy="5" r="1.5" />
    <path d="M4.5 5v6M11.5 6.5c0 2.5-3 2.5-5.6 4.6" />
  {:else if name === 'folder'}
    <path d="M2 4.5c0-.6.4-1 1-1h3l1.5 1.5H13c.6 0 1 .4 1 1V12c0 .6-.4 1-1 1H3c-.6 0-1-.4-1-1Z" />
  {:else if name === 'external'}
    <path d="M9.5 2.5h4v4M13.5 2.5 8 8M11.5 9.5V13c0 .3-.2.5-.5.5H3c-.3 0-.5-.2-.5-.5V5c0-.3.2-.5.5-.5h3.5" />
  {:else if name === 'eye'}
    <path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8Z" /><circle cx="8" cy="8" r="2" />
  {:else if name === 'eye-off'}
    <path d="M6.6 3.7A6 6 0 0 1 8 3.5c4 0 6.5 4.5 6.5 4.5a11 11 0 0 1-1.7 2.2M4.3 4.9A11 11 0 0 0 1.5 8S4 12.5 8 12.5c1.2 0 2.3-.4 3.2-1M2 2l12 12" />
  {:else if name === 'pencil'}
    <path d="M10.5 2.5 13.5 5.5 5.5 13.5H2.5v-3Z" />
  {:else if name === 'plus'}
    <path d="M8 3v10M3 8h10" />
  {:else if name === 'x'}
    <path d="M4 4l8 8M12 4l-8 8" />
  {:else if name === 'chevron-right'}
    <path d="M6 3.5 10.5 8 6 12.5" />
  {:else if name === 'chevron-down'}
    <path d="M3.5 6 8 10.5 12.5 6" />
  {:else if name === 'search'}
    <circle cx="7" cy="7" r="4.5" /><path d="m10.5 10.5 3.5 3.5" />
  {:else if name === 'file'}
    <path d="M9 1.5H4c-.6 0-1 .4-1 1v11c0 .6.4 1 1 1h8c.6 0 1-.4 1-1V5.5Z" /><path d="M9 1.5v4h4" />
  {:else if name === 'check'}
    <path d="m3 8.5 3 3 7-7" />
  {:else if name === 'alert'}
    <path d="M8 2 14.5 13.5h-13Z" /><path d="M8 6.5v3M8 11.6v.1" />
  {:else if name === 'terminal'}
    <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" /><path d="m4.5 6 2 2-2 2M8.5 10.5h3" />
  {:else if name === 'diff'}
    <path d="M5 2.5v5M2.5 5h5M8.5 11h5" /><rect x="1.5" y="1.5" width="13" height="13" rx="1.5" />
  {:else if name === 'archive'}
    <rect x="1.5" y="2.5" width="13" height="3" rx=".8" /><path d="M2.5 5.5V13c0 .3.2.5.5.5h10c.3 0 .5-.2.5-.5V5.5M6.5 8.5h3" />
  {:else if name === 'board'}
    <rect x="1.5" y="2.5" width="5" height="11" rx="1" /><rect x="9.5" y="2.5" width="5" height="6" rx="1" />
  {/if}
</svg>
