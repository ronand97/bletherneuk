// Live updates over server-sent events (docs/protocol.md section 2.4, GET /api/events).
// On error the stream is closed and reopened with exponential backoff, and pages show a
// "reconnecting" overlay. When it reopens, pages refetch, since events may have been missed.

import type { BridgeEvent, ChangedEvent, Focus, ThemeResponse } from './types';

export type ConnectionState = 'connecting' | 'open' | 'reconnecting';

interface EventMap {
  changed: ChangedEvent;
  theme: ThemeResponse;
  focus: Focus;
  bridge: BridgeEvent;
  /** The stream opened again after a drop. */
  reopen: null;
}

type Handler<K extends keyof EventMap> = (data: EventMap[K]) => void;

const BASE_DELAY_MS = 1000;
const MAX_DELAY_MS = 30_000;
const RELOAD_GUARD_KEY = 'bn-reload-at';

/** Backoff delay for a reconnect attempt (0-based), before jitter. */
export function backoffDelay(attempt: number): number {
  return Math.min(MAX_DELAY_MS, BASE_DELAY_MS * 2 ** Math.max(0, attempt));
}

class Connection {
  state = $state<ConnectionState>('connecting');
  /** Whether the iTerm2 bridge is connected. Null until the board or a `bridge` event says. */
  bridge = $state<boolean | null>(null);
  /** When the next reconnect attempt is due (ms since epoch), while reconnecting. */
  retryAt = $state(0);

  #source: EventSource | null = null;
  #attempt = 0;
  #everOpened = false;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #handlers: { [K in keyof EventMap]: Set<Handler<K>> } = {
    changed: new Set(),
    theme: new Set(),
    focus: new Set(),
    bridge: new Set(),
    reopen: new Set(),
  };

  get bridgeDown(): boolean {
    return this.bridge === false;
  }

  setBridge(connected: boolean): void {
    this.bridge = connected;
  }

  on<K extends keyof EventMap>(event: K, handler: Handler<K>): () => void {
    this.#handlers[event].add(handler);
    return () => {
      this.#handlers[event].delete(handler);
    };
  }

  start(): void {
    if (this.#source || typeof EventSource === 'undefined') return;
    const source = new EventSource('/api/events');
    this.#source = source;
    source.addEventListener('open', () => {
      const reopened = this.#everOpened;
      this.#everOpened = true;
      this.#attempt = 0;
      this.state = 'open';
      if (reopened) this.#emit('reopen', null);
    });
    source.addEventListener('error', () => this.#onError());
    this.#listen(source, 'changed', (d) => this.#emit('changed', d as ChangedEvent));
    this.#listen(source, 'theme', (d) => this.#emit('theme', d as ThemeResponse));
    this.#listen(source, 'focus', (d) => this.#emit('focus', d as Focus));
    this.#listen(source, 'bridge', (d) => {
      const connected = (d as BridgeEvent).connected === true;
      this.bridge = connected;
      this.#emit('bridge', { connected });
    });
  }

  #listen(source: EventSource, name: string, fn: (data: unknown) => void): void {
    source.addEventListener(name, (ev) => {
      let data: unknown;
      try {
        data = JSON.parse((ev as MessageEvent<string>).data);
      } catch {
        return;
      }
      if (data === null || typeof data !== 'object') return;
      fn(data);
    });
  }

  #emit<K extends keyof EventMap>(event: K, data: EventMap[K]): void {
    for (const handler of [...this.#handlers[event]]) {
      try {
        handler(data);
      } catch {
        // A failing handler must not stop the others.
      }
    }
  }

  #onError(): void {
    this.#source?.close();
    this.#source = null;
    this.state = 'reconnecting';
    if (this.#timer) return;
    const delay = backoffDelay(this.#attempt) * (0.85 + Math.random() * 0.3);
    this.#attempt += 1;
    this.retryAt = Date.now() + delay;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#retry();
    }, delay);
  }

  async #retry(): Promise<void> {
    // After a service restart the session cookie and write key are no longer valid.
    // Reloading leads to the service's "connecting" page, which signs the page in again.
    if (await sessionExpired()) {
      reloadOnce();
      return;
    }
    this.start();
  }
}

async function sessionExpired(): Promise<boolean> {
  try {
    const res = await fetch('/api/theme', { cache: 'no-store', credentials: 'same-origin' });
    return res.status === 401 || res.status === 403;
  } catch {
    return false;
  }
}

function reloadOnce(): void {
  // Guard against a reload loop if the service keeps refusing a freshly loaded page.
  let last = 0;
  try {
    last = Number(sessionStorage.getItem(RELOAD_GUARD_KEY) ?? '0') || 0;
  } catch {
    last = 0;
  }
  if (Date.now() - last < 15_000) {
    connection.start();
    return;
  }
  try {
    sessionStorage.setItem(RELOAD_GUARD_KEY, String(Date.now()));
  } catch {
    // Storage may be unavailable; the reload still happens.
  }
  location.reload();
}

export const connection = new Connection();
