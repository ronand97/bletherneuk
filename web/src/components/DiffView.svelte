<script lang="ts">
  // A virtualised diff: only rows on screen, plus an overscan either side, are in the DOM.
  // Row positions come from lib/diffModel.ts. Highlighting runs per hunk, only for hunks
  // that come on screen, and is cached for as long as the same diff is shown.
  import { untrack } from 'svelte';
  import type { Snippet } from 'svelte';
  import { SvelteMap } from 'svelte/reactivity';
  import {
    buildLayout,
    computeWindow,
    fileAt,
    fileDisplayPath,
    fileOffset,
    filePath,
    gutterDigits,
    hasBody,
    startsCollapsed,
  } from '../lib/diffModel';
  import type { DiffRow } from '../lib/diffModel';
  import { formatCount } from '../lib/format';
  import { highlightHunk, languageForPath, languageLabel } from '../lib/highlight';
  import type { ThemedToken } from '../lib/highlight';
  import type { FileDiff, FileStatus } from '../lib/types';
  import CodeTokens from './CodeTokens.svelte';
  import Icon from './Icon.svelte';

  let {
    files,
    footer,
    initialViewport = 600,
    initialLineHeight = 20,
  }: {
    files: FileDiff[];
    footer?: Snippet;
    /** Viewport height in px before it is measured (also used when rendering on a server). */
    initialViewport?: number;
    /** Line height in px before it is measured. */
    initialLineHeight?: number;
  } = $props();

  /** Rows rendered above and below the viewport. */
  const OVERSCAN = 30;
  const uid = $props.id();

  const STATUS_TEXT: Record<FileStatus, string> = {
    added: 'added',
    deleted: 'deleted',
    modified: 'modified',
    renamed: 'renamed',
    copied: 'copied',
    typechange: 'type changed',
  };

  const COLLAPSED_TEXT: Record<string, string> = {
    lockfile: 'Lockfile',
    minified: 'Minified file',
    large: 'Large diff',
  };

  // Collapse choices made by hand, keyed by path so they survive a reload of the same diff.
  const overrides = new SvelteMap<string, boolean>();
  const indexOverrides = $derived.by(() => {
    const out = new Map<number, boolean>();
    files.forEach((f, i) => {
      const o = overrides.get(fileDisplayPath(f));
      if (o !== undefined) out.set(i, o);
    });
    return out;
  });
  const layout = $derived(buildLayout(files, indexOverrides));

  let scroller = $state<HTMLDivElement | null>(null);
  let probe = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  // Only the starting values; they are measured from then on.
  let viewport = $state(untrack(() => initialViewport));
  let viewportWidth = $state(0);
  let lineHeight = $state(untrack(() => initialLineHeight));

  const win = $derived(computeWindow(layout.offsets, scrollTop / lineHeight, viewport / lineHeight, OVERSCAN));
  const visible = $derived(
    layout.rows.slice(win.start, win.end).map((row, i) => {
      const index = win.start + i;
      return {
        row,
        top: layout.offsets[index] * lineHeight,
        height: (layout.offsets[index + 1] - layout.offsets[index]) * lineHeight,
      };
    }),
  );
  const currentFile = $derived(fileAt(layout, scrollTop / lineHeight) ?? 0);
  const digits = $derived(gutterDigits(layout.maxLineNumber));
  const languages = $derived(files.map((f) => languageForPath(filePath(f))));

  // Measure the line height and viewport, and follow theme and window changes.
  $effect(() => {
    const el = scroller;
    const p = probe;
    if (!el || !p) return;
    const measure = () => {
      const h = p.getBoundingClientRect().height;
      if (h > 0) lineHeight = h;
      viewport = el.clientHeight;
      viewportWidth = el.clientWidth;
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    observer.observe(p);
    return () => observer.disconnect();
  });

  let frame = 0;
  function onScroll(): void {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (scroller) scrollTop = scroller.scrollTop;
    });
  }

  function toggle(fileIndex: number, collapsed: boolean): void {
    overrides.set(fileDisplayPath(files[fileIndex]), !collapsed);
  }

  function jumpTo(fileIndex: number): void {
    if (!scroller) return;
    scroller.scrollTop = fileOffset(layout, fileIndex) * lineHeight;
    scrollTop = scroller.scrollTop;
  }

  function onJump(ev: Event): void {
    const value = Number((ev.currentTarget as HTMLSelectElement).value);
    if (Number.isInteger(value) && value >= 0 && value < files.length) jumpTo(value);
  }

  // Highlighting ------------------------------------------------------------------
  // Tokens per hunk ("file:hunk"), with one entry per line; null entries are plain text.
  let hlVersion = $state(0);
  let cacheFor: FileDiff[] | null = null;
  let cache = new Map<string, (ThemedToken[] | null)[] | null>();
  let pending = new Set<string>();

  function currentCache(): Map<string, (ThemedToken[] | null)[] | null> {
    if (cacheFor !== files) {
      cacheFor = files;
      cache = new Map();
      pending = new Set();
    }
    return cache;
  }

  $effect(() => {
    const rows = visible;
    const shownFiles = files;
    const store = currentCache();
    for (const { row } of rows) {
      if (row.kind !== 'line') continue;
      const key = `${row.file}:${row.hunk}`;
      if (store.has(key) || pending.has(key)) continue;
      const lang = languages[row.file];
      const hunk = shownFiles[row.file]?.hunks[row.hunk];
      if (lang === null || !hunk) {
        store.set(key, null);
        continue;
      }
      pending.add(key);
      void highlightHunk(hunk, lang).then((tokens) => {
        if (cacheFor !== shownFiles) return;
        store.set(key, tokens);
        pending.delete(key);
        hlVersion += 1;
      });
    }
  });

  function tokensFor(row: Extract<DiffRow, { kind: 'line' }>): ThemedToken[] | null {
    void hlVersion;
    return currentCache().get(`${row.file}:${row.hunk}`)?.[row.line] ?? null;
  }

  function lineOf(row: Extract<DiffRow, { kind: 'line' }>) {
    return files[row.file].hunks[row.hunk].lines[row.line];
  }

  function lineClass(kind: string): string {
    return kind === '+' ? 'k-add' : kind === '-' ? 'k-del' : 'k-ctx';
  }
</script>

<div class="diff">
  <div class="bar">
    <label class="sr-only" for="{uid}-jump">Jump to file</label>
    <select id="{uid}-jump" class="select jump mono" value={String(currentFile)} onchange={onJump}>
      {#each files as f, i (i)}
        <option value={String(i)}>{fileDisplayPath(f)} (+{formatCount(f.additions)} −{formatCount(f.deletions)})</option>
      {/each}
    </select>
    <span class="tiny muted pos">{currentFile + 1} of {files.length}</span>
  </div>

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="scroll" bind:this={scroller} onscroll={onScroll} tabindex="0" role="region" aria-label="Diff">
    <div class="probe" bind:this={probe} aria-hidden="true">0</div>
    <div
      class="virtual"
      style:height="{layout.total * lineHeight}px"
      style:min-width="calc({digits * 2 + 4 + layout.maxColumns}ch + 3em)"
      style:--gutter-digits={digits}
    >
      {#each visible as v (v.row.key)}
        {@const row = v.row}
        {@const f = files[row.file]}
        {#if row.kind === 'file'}
          <div class="row r-file" style:transform="translateY({v.top}px)" style:height="{v.height}px">
            <div class="file-head" style:width={viewportWidth ? `${viewportWidth}px` : undefined}>
              {#if hasBody(f)}
                <button
                  type="button"
                  class="btn ghost icon toggle"
                  aria-expanded={!row.collapsed}
                  onclick={() => toggle(row.file, row.collapsed)}
                  title={row.collapsed ? 'Show this file’s diff' : 'Collapse this file'}
                  aria-label={row.collapsed ? 'Expand file' : 'Collapse file'}
                >
                  <Icon name={row.collapsed ? 'chevron-right' : 'chevron-down'} />
                </button>
              {:else}
                <span class="toggle-space"></span>
              {/if}
              <span class="path ellipsis" title={fileDisplayPath(f)}>{fileDisplayPath(f)}</span>
              <span class="status st-{f.status}">{Object.hasOwn(STATUS_TEXT, f.status) ? STATUS_TEXT[f.status] : 'changed'}</span>
              {#if languages[row.file]}
                <span class="lang">{languageLabel(languages[row.file])}</span>
              {/if}
              <span class="counts"><span class="add">+{formatCount(f.additions)}</span> <span class="del">−{formatCount(f.deletions)}</span></span>
            </div>
          </div>
        {:else if row.kind === 'notice'}
          <div class="row r-notice" style:transform="translateY({v.top}px)" style:height="{v.height}px">
            <div class="notice" style:width={viewportWidth ? `${viewportWidth}px` : undefined}>
              {#if row.notice === 'binary'}
                <span>Binary file, not shown.</span>
              {:else if row.notice === 'unavailable'}
                <span>Content no longer available.</span>
              {:else if row.notice === 'truncated'}
                <span>The rest of this file’s diff was cut short; it is too large to show in full.</span>
              {:else if row.notice === 'empty'}
                <span>
                  {f.status === 'renamed' ? 'Renamed without changes.' : f.status === 'typechange' ? 'File type changed.' : 'No content changes.'}
                </span>
              {:else if row.notice === 'collapsed'}
                <span>
                  {f.collapsed && Object.hasOwn(COLLAPSED_TEXT, f.collapsed) ? COLLAPSED_TEXT[f.collapsed] : 'Large file'}, collapsed
                  ({formatCount(f.additions + f.deletions)} changed lines).
                </span>
                <button type="button" class="btn small" onclick={() => toggle(row.file, true)}>Show diff</button>
              {/if}
            </div>
          </div>
        {:else if row.kind === 'hunk'}
          <div class="row r-hunk" style:transform="translateY({v.top}px)" style:height="{v.height}px">
            <span class="hunk-head">{f.hunks[row.hunk].header}</span>
          </div>
        {:else if row.kind === 'line'}
          {@const line = lineOf(row)}
          <div class="row r-line {lineClass(line.kind)}" style:transform="translateY({v.top}px)" style:height="{v.height}px">
            <span class="gutter" aria-hidden="true"><span class="ln">{line.old ?? ''}</span><span class="ln">{line.new ?? ''}</span><span class="sign">{line.kind === ' ' ? '' : line.kind === '-' ? '−' : '+'}</span></span>
            <span class="sr-only">{line.kind === '+' ? 'added' : line.kind === '-' ? 'removed' : ''}</span>
            <span class="code"><CodeTokens text={line.text} tokens={tokensFor(row)} /></span>
          </div>
        {/if}
      {/each}
    </div>
    {@render footer?.()}
  </div>
</div>

<style>
  .diff {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    min-width: 0;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: var(--gap-s);
    padding: var(--pad-xs) var(--pad-s);
    border-bottom: 1px solid var(--line-soft);
    background: var(--surface);
  }

  .jump {
    flex: 1;
    min-width: 0;
    max-width: 48em;
    font-size: 0.86em;
  }

  .pos {
    flex: none;
    font-variant-numeric: tabular-nums;
  }

  .scroll {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    overscroll-behavior: contain;
  }

  .scroll:focus-visible {
    outline-offset: -2px;
  }

  .probe,
  .virtual {
    font-family: var(--font-code), ui-monospace, Menlo, monospace;
    font-size: 0.92em;
    line-height: var(--line-height);
  }

  .probe {
    position: absolute;
    visibility: hidden;
    pointer-events: none;
    white-space: pre;
  }

  .virtual {
    position: relative;
    tab-size: 4;
  }

  .row {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: stretch;
    white-space: pre;
    /* clip, not hidden: hidden would make each row a scroll container and break sticky gutters. */
    overflow: clip;
    will-change: transform;
  }

  /* File headers and notices stay in view when the diff scrolls sideways. */
  .file-head,
  .notice,
  .hunk-head {
    position: sticky;
    left: 0;
  }

  .r-file {
    align-items: flex-end;
  }

  .file-head {
    display: flex;
    align-items: center;
    gap: 0.6em;
    min-width: 0;
    width: 100%;
    height: calc(100% - 0.2em);
    padding: 0 var(--pad-s) 0 var(--pad-xs);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    font-family: var(--font-ui), system-ui, sans-serif;
    font-size: calc(var(--font-size) * 0.95);
  }

  .toggle {
    width: 1.6em;
    height: 1.6em;
  }

  .toggle-space {
    width: 1.6em;
    flex: none;
  }

  .path {
    flex: 0 1 auto;
    font-family: var(--font-code), ui-monospace, Menlo, monospace;
    font-size: 0.95em;
    font-weight: 600;
  }

  .status,
  .lang {
    flex: none;
    font-size: 0.82em;
    color: var(--muted);
  }

  .st-added {
    color: var(--done);
  }

  .st-deleted {
    color: var(--needs-you);
  }

  .st-renamed,
  .st-copied {
    color: var(--accent);
  }

  .lang::before {
    content: '·';
    margin-right: 0.6em;
    opacity: 0.6;
  }

  .counts {
    flex: none;
    margin-left: auto;
    font-family: var(--font-code), ui-monospace, Menlo, monospace;
    font-size: 0.86em;
    font-variant-numeric: tabular-nums;
  }

  .r-notice {
    align-items: center;
  }

  .notice {
    display: flex;
    align-items: center;
    gap: var(--gap);
    height: 100%;
    padding: 0 var(--pad);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--surface) 55%, transparent);
    color: var(--muted);
    font-family: var(--font-ui), system-ui, sans-serif;
    font-size: calc(var(--font-size) * 0.9);
    white-space: normal;
  }

  .r-hunk {
    align-items: center;
    background: color-mix(in srgb, var(--accent) 7%, transparent);
    color: var(--muted);
  }

  .hunk-head {
    padding-left: calc((var(--gutter-digits) * 2 + 4) * 1ch);
    font-size: 0.92em;
  }

  .gutter {
    position: sticky;
    left: 0;
    z-index: 1;
    display: flex;
    flex: none;
    background: var(--background);
    color: var(--muted);
    user-select: none;
  }

  .ln {
    width: calc((var(--gutter-digits) + 1) * 1ch);
    padding-right: 0.5ch;
    text-align: right;
    opacity: 0.7;
    font-variant-numeric: tabular-nums;
  }

  .sign {
    width: 2ch;
    text-align: center;
  }

  .code {
    flex: 1;
    padding-right: 2em;
    color: var(--shiki-foreground);
  }

  .k-add {
    background: var(--diff-added-bg);
  }

  .k-add .gutter {
    background: linear-gradient(var(--diff-added-bg), var(--diff-added-bg)), var(--background);
  }

  .k-add .sign {
    color: var(--done);
  }

  .k-del {
    background: var(--diff-removed-bg);
  }

  .k-del .gutter {
    background: linear-gradient(var(--diff-removed-bg), var(--diff-removed-bg)), var(--background);
  }

  .k-del .sign {
    color: var(--needs-you);
  }

</style>
