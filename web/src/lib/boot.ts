// Reads the two <meta> tags the service adds to the page (docs/protocol.md section 2.3).

import type { BlockId, PageKind } from './types';

export interface Boot {
  page: PageKind;
  key: string | null;
}

interface MetaLike {
  getAttribute(name: string): string | null;
  remove(): void;
}

interface DocLike {
  querySelector(selector: string): MetaLike | null;
}

interface LocationLike {
  pathname: string;
  search: string;
}

/**
 * Reads the page kind and the write key, then removes the key's meta element so it
 * stays only in memory. Falls back to the URL path when the page meta is missing.
 */
export function readBoot(doc: DocLike, location: LocationLike): Boot {
  const pageMeta = doc.querySelector('meta[name="bn-page"]');
  const keyMeta = doc.querySelector('meta[name="bn-key"]');
  const key = keyMeta?.getAttribute('content') ?? null;
  keyMeta?.remove();
  return { page: parsePage(pageMeta?.getAttribute('content') ?? null, location.pathname), key };
}

export function parsePage(meta: string | null, pathname: string): PageKind {
  if (meta === 'board' || meta === 'panel' || meta === 'changes') return meta;
  if (pathname.startsWith('/panel')) return 'panel';
  if (pathname.startsWith('/changes')) return 'changes';
  return 'board';
}

/** The block ID in `/changes?block=<id>`, or null when it is missing or not a positive integer. */
export function blockFromSearch(search: string): BlockId | null {
  const raw = new URLSearchParams(search).get('block');
  if (raw === null || !/^[0-9]{1,15}$/.test(raw)) return null;
  const id = Number(raw);
  return id > 0 ? id : null;
}
