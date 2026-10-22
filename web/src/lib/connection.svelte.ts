// Live updates over server-sent events (docs/protocol.md section 2.4, GET /api/events).
// The stream is read with fetch() so it can carry the write key header. On error it is
// reopened with exponential backoff, and pages show a "reconnecting" overlay. When it
// reopens, pages refetch, since events may have been missed. If the service no longer
// knows the key (it restarted), the page has expired: the toolbelt panel reloads, which
// makes the bridge register it again; other pages say how to reopen them.

import { authHeaders } from './api';
import { SseParser, type SseMessage } from './sse';
import type { BridgeEvent, ChangedEvent, Focus, ThemeResponse } from './types';

export type ConnectionState = 'connecting' | 'open' | 'reconnecting' | 'expired';

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
  /** The toolbelt panel reloads itself when it expires; other pages show a message. */
  reloadWhenExpired = false;

  #abort: AbortController | null = null;
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
    if (this.#abort || this.state === 'expired') return;
    const ctrl = new AbortController();
    this.#abort = ctrl;
    void this.#run(ctrl);
  }

  async #run(ctrl: AbortController): Promise<void> {
    let res: Response;
    try {
      res = await fetch('/api/events', {
        headers: authHeaders({ Accept: 'text/event-stream' }),
        credentials: 'omit',
        cache: 'no-store',
        signal: ctrl.signal,
      });
    } catch {
      this.#drop(ctrl);
      return;
    }
    if (res.status === 401 || res.status === 403) {
      this.#abort = null;
      this.#expired();
      return;
    }
    if (!res.ok || !res.body) {
      this.#drop(ctrl);
      return;
    }
    const reopened = this.#everOpened;
    this.#everOpened = true;
    this.#attempt = 0;
    this.state = 'open';
    if (reopened) this.#emit('reopen', null);
    const parser = new SseParser();
    const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        for (const message of parser.push(value)) this.#dispatch(message);
      }
    } catch {
      // The stream broke; reconnect below.
    }
    this.#drop(ctrl);
  }

  #dispatch(message: SseMessage): void {
    let data: unknown;
    try {
      data = JSON.parse(message.data);
    } catch {
      return;
    }
    if (data === null || typeof data !== 'object') return;
    switch (message.event) {
      case 'changed':
        this.#emit('changed', data as ChangedEvent);
        break;
      case 'theme':
        this.#emit('theme', data as ThemeResponse);
        break;
      case 'focus':
        this.#emit('focus', data as Focus);
        break;
      case 'bridge': {
        const connected = (data as BridgeEvent).connected === true;
        this.bridge = connected;
        this.#emit('bridge', { connected });
        break;
      }
    }
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

  #drop(ctrl: AbortController): void {
    if (this.#abort !== ctrl) return;
    this.#abort = null;
    this.state = 'reconnecting';
    if (this.#timer) return;
    const delay = backoffDelay(this.#attempt) * (0.85 + Math.random() * 0.3);
    this.#attempt += 1;
    this.retryAt = Date.now() + delay;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      this.start();
    }, delay);
  }

  #expired(): void {
    if (this.reloadWhenExpired) {
      reloadOnce();
      return;
    }
    this.state = 'expired';
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
    connection.state = 'expired';
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
