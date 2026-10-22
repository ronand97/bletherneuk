import './styles/app.css';
import { mount } from 'svelte';
import type { Component } from 'svelte';
import { getJSON, setWriteKey } from './lib/api';
import { blockFromSearch, readBoot } from './lib/boot';
import { clock } from './lib/clock.svelte';
import { connection } from './lib/connection.svelte';
import { applyTheme } from './lib/theme';
import type { ThemeResponse } from './lib/types';

// Nothing may reach the console: iTerm2 copies the toolbelt's console output into its
// debug log (design section 20.8). The pages never log. Libraries (the Svelte runtime,
// Shiki's tokenizer) can warn in rare cases, so in built pages the console does nothing,
// and uncaught errors are swallowed instead of reported.
if (import.meta.env.PROD) {
  for (const method of ['debug', 'error', 'info', 'log', 'trace', 'warn'] as const) {
    console[method] = () => undefined;
  }
}
window.addEventListener('error', (ev) => ev.preventDefault());
window.addEventListener('unhandledrejection', (ev) => ev.preventDefault());

const boot = readBoot(document, location);
setWriteKey(boot.key);

void getJSON<ThemeResponse>('/api/theme')
  .then((theme) => applyTheme(theme.vars))
  .catch(() => undefined);
connection.on('theme', (theme) => applyTheme(theme.vars));
connection.on('reopen', () => {
  void getJSON<ThemeResponse>('/api/theme')
    .then((theme) => applyTheme(theme.vars))
    .catch(() => undefined);
});
connection.start();
clock.start();

const target = document.getElementById('app');

async function start(): Promise<void> {
  if (!target) return;
  document.body.dataset.page = boot.page;
  if (boot.page === 'changes') {
    // The Changes page carries Shiki, so it loads only when needed.
    const { default: Changes } = await import('./pages/Changes.svelte');
    mount(Changes, { target, props: { blockId: blockFromSearch(location.search) } });
    return;
  }
  const page: Component =
    boot.page === 'panel' ? (await import('./pages/Panel.svelte')).default : (await import('./pages/Board.svelte')).default;
  mount(page, { target });
}

void start();
