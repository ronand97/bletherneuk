// HTTP access to the service (docs/protocol.md section 2).
// The write key lives only in this module variable: never in storage, cookies or the DOM.

import type { ActionResult } from './types';

let writeKey: string | null = null;

export function setWriteKey(key: string | null): void {
  writeKey = key && key.length > 0 ? key : null;
}

export class ApiError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

/** Builds an API path from fixed parts and IDs, encoding every ID. */
export function apiPath(...parts: (string | number)[]): string {
  return '/api/' + parts.map((p) => encodeURIComponent(String(p))).join('/');
}

export async function getJSON<T>(path: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, {
    headers: { Accept: 'application/json' },
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
  });
  if (!res.ok) throw new ApiError(res.status, `HTTP ${res.status}`);
  return (await res.json()) as T;
}

/** Sends an action. Never throws: failures come back as `{ok: false, error}` to show as text. */
export async function post(path: string, body: unknown = {}): Promise<ActionResult> {
  if (!writeKey) {
    return { ok: false, error: 'This page has no write key. Reopen it from bletherneuk.' };
  }
  let res: Response;
  try {
    res = await fetch(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Bletherneuk-Key': writeKey },
      body: JSON.stringify(body),
      credentials: 'same-origin',
      cache: 'no-store',
    });
  } catch {
    return { ok: false, error: 'Could not reach bletherneuk.' };
  }
  let data: unknown = null;
  try {
    data = await res.json();
  } catch {
    data = null;
  }
  return toActionResult(data, res.status);
}

export function toActionResult(data: unknown, status: number): ActionResult {
  if (data !== null && typeof data === 'object' && 'ok' in data) {
    const d = data as { ok: unknown; error?: unknown };
    if (d.ok === true) return { ok: true };
    const error = typeof d.error === 'string' && d.error.trim() !== '' ? d.error : 'Something went wrong.';
    return { ok: false, error };
  }
  if (status >= 200 && status < 300) return { ok: true };
  return { ok: false, error: `bletherneuk refused this (HTTP ${status}).` };
}

export function describeLoadError(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 404) return 'Not found. It may have been merged into another block.';
    if (err.status === 401 || err.status === 403) return 'This page’s session has ended. Reopen it from bletherneuk.';
    return `bletherneuk answered with an error (HTTP ${err.status}).`;
  }
  return 'Could not reach bletherneuk.';
}
