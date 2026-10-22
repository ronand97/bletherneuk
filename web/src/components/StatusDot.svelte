<script lang="ts">
  import { STATUS_LABEL } from '../lib/format';
  import type { ChatStatus } from '../lib/types';

  let { status, background = false }: { status: ChatStatus; background?: boolean } = $props();

  const label = $derived(
    (Object.hasOwn(STATUS_LABEL, status) ? STATUS_LABEL[status] : 'unknown') +
      (background ? ' (background work)' : ''),
  );
</script>

<span class="dot s-{status}" title={label}><span class="sr-only">{label}</span></span>
