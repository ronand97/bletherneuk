// A link becomes an href only when it is http or https (design section 20.3 and 20.5).

export function safeHttpUrl(raw: unknown): string | null {
  if (typeof raw !== 'string') return null;
  let url: URL;
  try {
    url = new URL(raw.trim());
  } catch {
    return null;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return null;
  return url.href;
}

/** The host the link really goes to, or null when it isn't an http(s) URL. */
export function hostOf(raw: unknown): string | null {
  const href = safeHttpUrl(raw);
  return href === null ? null : new URL(href).host;
}

export const LINK_KIND_LABEL: Record<string, string> = {
  github_pr: 'PR',
  jira: 'Jira',
  notion: 'Notion',
  other: 'Link',
};

export function linkKindLabel(kind: string): string {
  return Object.hasOwn(LINK_KIND_LABEL, kind) ? LINK_KIND_LABEL[kind] : 'Link';
}
