<script lang="ts">
  // Renders one diff line. Highlighted tokens become text spans whose colour is a theme
  // variable; without tokens the line is plain text. Never {@html}.
  import { FONT_BOLD, FONT_ITALIC, FONT_UNDERLINE, tokenColor } from '../lib/highlight';
  import type { ThemedToken } from '../lib/highlight';

  let { text, tokens = null }: { text: string; tokens?: ThemedToken[] | null } = $props();

  function has(token: ThemedToken, flag: number): boolean {
    const style = token.fontStyle ?? 0;
    return style > 0 && (style & flag) !== 0;
  }
</script>

{#if tokens}{#each tokens as token, i (i)}<span class:it={has(token, FONT_ITALIC)} class:bd={has(token, FONT_BOLD)} class:ul={has(token, FONT_UNDERLINE)} style:color={tokenColor(token)}>{token.content}</span>{/each}{:else}{text}{/if}

<style>
  .it {
    font-style: italic;
  }

  .bd {
    font-weight: 600;
  }

  .ul {
    text-decoration: underline;
  }
</style>
