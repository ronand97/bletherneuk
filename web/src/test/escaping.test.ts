// Design section 20.3 and 20.11: script payloads in every rendered field must render as text.
// Each component is rendered with Svelte's server renderer, and the HTML is checked for
// elements the payloads would create if they were ever treated as markup.

import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import type { Component, ComponentProps } from 'svelte';
import { tokenizeLines, languageForPath } from '../lib/highlight';
import { mergeTimeline } from '../lib/timeline';
import type {
  BlockCard,
  BlockDetail,
  Board,
  ChatSummary,
  DiffResponse,
  FileDiff,
  FileItem,
  Link,
  Turn,
  Worktree,
} from '../lib/types';
import BoardView from '../pages/BoardView.svelte';
import CompactBoard from '../pages/CompactBoard.svelte';
import PanelBlock from '../pages/PanelBlock.svelte';
import CodeTokens from '../components/CodeTokens.svelte';
import DiffSummary from '../components/DiffSummary.svelte';
import DiffView from '../components/DiffView.svelte';
import ErrorLine from '../components/ErrorLine.svelte';
import LinkRow from '../components/LinkRow.svelte';
import NotInDiff from '../components/NotInDiff.svelte';
import TimelineList from '../components/TimelineList.svelte';

const IMG = '<img src=x onerror=alert(1)>';
const SCRIPT = '</script><script>alert(1)</script>';
const PAYLOADS = [IMG, SCRIPT];

const NOW = new Date(2026, 9, 22, 12, 0).getTime();

/** How Svelte writes `<` and `&` in text and attributes. */
function escaped(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;');
}

function renderHtml<C extends Component<any>>(component: C, props: ComponentProps<C>): string {
  return render(component as Component<any>, { props }).body;
}

// Every element the components are allowed to produce.
const ALLOWED_TAGS = new Set([
  'a', 'article', 'aside', 'button', 'circle', 'div', 'form', 'h1', 'h2', 'h3', 'header', 'input', 'label',
  'li', 'main', 'ol', 'option', 'p', 'path', 'rect', 'section', 'select', 'span', 'strong', 'svg', 'ul',
]);

interface Element {
  tag: string;
  attrs: Map<string, string>;
}

/**
 * The start tags in server-rendered HTML with their attributes. Svelte always double-quotes
 * attribute values, so a `>` inside a value doesn't end the tag.
 */
function elements(html: string): Element[] {
  const out: Element[] = [];
  const tag = /<([a-zA-Z][\w:-]*)((?:\s+[^\s"'>/=]+(?:="[^"]*")?)*)\s*\/?>/g;
  for (const m of html.matchAll(tag)) {
    const attrs = new Map<string, string>();
    for (const a of m[2].matchAll(/\s+([^\s"'>/=]+)(?:="([^"]*)")?/g)) attrs.set(a[1].toLowerCase(), a[2] ?? '');
    out.push({ tag: m[1].toLowerCase(), attrs });
  }
  return out;
}

/**
 * Fails if the HTML has any element or attribute a payload could have created, or if the
 * payload doesn't appear, escaped, at least `times` times.
 */
function expectInert(html: string, payload: string, times = 1): void {
  for (const el of elements(html)) {
    expect(ALLOWED_TAGS, `unexpected <${el.tag}>`).toContain(el.tag);
    for (const [name, value] of el.attrs) {
      expect(name.startsWith('on'), `event attribute ${name} on <${el.tag}>`).toBe(false);
      if (name === 'href') expect(value).toMatch(/^https?:\/\//);
    }
  }
  // Nothing raw: a start of a tag only ever appears escaped.
  expect(html).not.toMatch(/<img[\s/>]/i);
  expect(html).not.toMatch(/<\/?script[\s/>]/i);
  const count = html.split(escaped(payload)).length - 1;
  expect(count, 'escaped payload occurrences').toBeGreaterThanOrEqual(times);
}

function chat(p: string, extra: Partial<ChatSummary> = {}): ChatSummary {
  return {
    id: '11111111-2222-4333-8444-555555555555',
    title: p,
    status: 'needs_you',
    block: 3,
    block_title: p,
    last_activity: NOW - 60_000,
    status_since: NOW - 120_000,
    worktree: p,
    branch: p,
    now_on: p,
    group_reason: p,
    keep_awake: false,
    can_resume: false,
    resume_note: p,
    hidden: false,
    open_in_iterm: false,
    ...extra,
  };
}

function card(p: string): BlockCard {
  return {
    id: 3,
    title: p,
    repo_name: p,
    archived: false,
    last_active: NOW - 60_000,
    pr: { number: 42, url: 'https://github.com/acme/shop/pull/42', state: 'OPEN', draft: false, checks: 'failing' },
    ticket: p,
    worktrees: [p],
    chats: { working: 1, needs_you: 1, done: 0, idle: 0, closed: 2 },
    docs: 1,
    outputs: 2,
  };
}

function board(p: string): Board {
  return {
    now: NOW,
    iterm_connected: true,
    keep_awake: { holding: true, chats: 2 },
    needs_you: [chat(p)],
    blocks: [card(p)],
    unsorted: [chat(p, { id: '66666666-7777-4888-8999-000000000000', block: null, status: 'idle' })],
    focus: { block: null, chat: null },
  };
}

function worktree(p: string): Worktree {
  return {
    id: 7,
    name: p,
    path: p,
    is_main: false,
    branch: p,
    detached_at: null,
    uncommitted: 3,
    uncommitted_many: false,
    ahead: 12,
    behind: 0,
    removed: false,
  };
}

function link(p: string, url = `https://example.atlassian.net/browse/ABC-123?q=${encodeURIComponent(p)}`): Link {
  return {
    id: 1,
    kind: 'jira',
    url,
    host: p,
    title: p,
    status: { state: 'OPEN', draft: false, checks: 'passing', review: 'APPROVED' },
    source: 'manual',
    added_by_claude: true,
  };
}

function fileItem(p: string, id: number, kind: 'doc' | 'output'): FileItem {
  return {
    id,
    name: p,
    folder: p,
    kind,
    found_by: 'tool',
    size: 1234,
    modified: NOW - 3_600_000,
    missing: false,
    copy: true,
    openable: true,
    hidden: false,
  };
}

function detail(p: string): BlockDetail {
  return {
    block: { id: 3, title: p, title_source: 'rename', repo: p, repo_name: p, archived: false, last_active: NOW },
    worktrees: [worktree(p), { ...worktree(p), id: 8, is_main: true, branch: null, detached_at: p }],
    links: [link(p)],
    chats: [chat(p, { status: 'working', keep_awake: true })],
    docs: [fileItem(p, 1, 'doc')],
    outputs: [fileItem(p, 2, 'output')],
    changes_since_review: { worktree: 7, files: 12 },
    keep_awake: { block: false },
    new_chat_name: p,
  };
}

function diffFile(p: string, extra: Partial<FileDiff> = {}): FileDiff {
  return {
    old_path: `src/${p}.ts`,
    new_path: `src/${p}.rs`,
    status: 'renamed',
    binary: false,
    additions: 2,
    deletions: 1,
    collapsed: null,
    truncated: false,
    unavailable: false,
    hunks: [
      {
        header: `@@ -1,3 +1,4 @@ ${p}`,
        lines: [
          { kind: ' ', old: 1, new: 1, text: p },
          { kind: '-', old: 2, new: null, text: p },
          { kind: '+', old: null, new: 2, text: p },
          { kind: '+', old: null, new: 3, text: `const x = "${p}";` },
        ],
      },
    ],
    ...extra,
  };
}

function turn(p: string, id: number, extra: Partial<Turn> = {}): Turn {
  return {
    id,
    chat: '11111111-2222-4333-8444-555555555555',
    chat_title: p,
    prompt: p,
    started_at: NOW - id * 60_000,
    ended_at: NOW - id * 60_000 + 30_000,
    worktree: 7,
    files_changed: 3,
    lines_added: 10,
    lines_removed: 2,
    parallel: true,
    missed: null,
    snapshot_deleted: false,
    ...extra,
  };
}

describe.each(PAYLOADS)('payload %s', (p) => {
  it('renders as text on the board: block titles, PR and ticket, worktrees, chat titles and branches', () => {
    const b = board(p);
    const html = renderHtml(BoardView, { board: b, now: NOW, targets: [{ id: 3, title: p }] });
    // Block title, repo name, ticket, worktree name, needs-you chat title + block title + worktree,
    // unsorted chat title + worktree, and the "Move to…" option for the block.
    expectInert(html, p, 9);
  });

  it('renders as text in the compact board', () => {
    const html = renderHtml(CompactBoard, { board: board(p), now: NOW, targets: [{ id: 3, title: p }] });
    expectInert(html, p, 4);
  });

  it('renders as text in the panel: title, repo, worktrees, links, chats, docs and outputs', () => {
    const html = renderHtml(PanelBlock, { detail: detail(p), now: NOW, targets: [{ id: 9, title: p }] });
    // block title, repo name, 2 worktree names, branch, detached-at, link title, host-free link,
    // chat title/worktree/branch/now_on/group_reason, doc and output names and folders, move option.
    expectInert(html, p, 15);
  });

  it('renders link titles as text and keeps the host visible', () => {
    const html = renderHtml(LinkRow, { link: link(p) });
    expectInert(html, p, 1);
    expect(html).toContain('example.atlassian.net');
  });

  it('renders the timeline: chat titles, prompts and snapshot-missed reasons', () => {
    const turns = [turn(p, 1), turn(p, 2, { missed: p, ended_at: null })];
    const items = mergeTimeline(turns, [{ id: 1, worktree: 7, created_at: NOW - 90_000 }], 7);
    const html = renderHtml(TimelineList, {
      items,
      worktreeNames: new Map([[7, p]]),
      showWorktree: true,
      onselect: () => undefined,
    });
    // 2 chat titles, 2 prompts, 1 missed reason, 3 worktree names (2 turns + review).
    expectInert(html, p, 8);
  });

  it('renders diff file names, hunk headers and code lines as text', () => {
    const files = [diffFile(p), diffFile(p, { status: 'added', old_path: null, new_path: `${p}.weird` })];
    const html = renderHtml(DiffView, { files });
    // Paths in headers and the jump menu, hunk headers, and every code line.
    expectInert(html, p, 10);
  });

  it('renders the range labels and the "Not in this diff" list as text', () => {
    const diff: DiffResponse = {
      range: { kind: 'review', from_label: p, to_label: p },
      files: [],
      totals: { files: 0, additions: 0, deletions: 0 },
      not_in_diff: [{ path: p, reason: p }],
      parallel: true,
      unavailable: null,
    };
    expectInert(renderHtml(DiffSummary, { diff, requested: 'review', onreload: () => undefined }), p, 2);
    expectInert(renderHtml(NotInDiff, { items: diff.not_in_diff }), p, 2);
  });

  it('renders error text from the service as text', () => {
    expectInert(renderHtml(ErrorLine, { message: p }), p, 1);
  });

  it('renders highlighted code tokens as text', async () => {
    const tokens = await tokenizeLines([p, `const s = '${p}';`, `// ${p}`], 'typescript');
    expect(tokens).not.toBeNull();
    const html = (tokens ?? []).map((t, i) => renderHtml(CodeTokens, { text: `line ${i}`, tokens: t })).join('\n');
    expect(html).not.toMatch(/<img[\s/>]/i);
    expect(html).not.toMatch(/<script[\s/>]/i);
    // Token colours are only theme variables.
    for (const m of html.matchAll(/style="([^"]*)"/g)) expect(m[1]).toMatch(/^color: var\(--shiki-[a-z-]+\);$/);
    expect(html).toContain('&lt;');
  });

  it('renders code inside an HTML file, where the grammar sees real tags, as text', async () => {
    const tokens = await tokenizeLines([`<div>${p}</div>`, p], 'html');
    expect(tokens).not.toBeNull();
    const html = (tokens ?? []).map((t) => renderHtml(CodeTokens, { text: '', tokens: t })).join('\n');
    expect(html).not.toMatch(/<img[\s/>]/i);
    expect(html).not.toMatch(/<script[\s/>]/i);
    expect(html).not.toMatch(/<div[\s>]/i);
  });

  it('never turns a file extension into a language name', () => {
    expect(languageForPath(`report.${p}`)).toBeNull();
    expect(languageForPath(`src/${p}`)).toBeNull();
  });
});

describe('the checker itself', () => {
  it('rejects markup a payload would create if it were not escaped', () => {
    expect(() => expectInert(`<p>${IMG}</p>`, IMG)).toThrow();
    expect(() => expectInert(`<p>${SCRIPT}</p>`, SCRIPT)).toThrow();
    expect(() => expectInert('<p onclick="x">&lt;img src=x onerror=alert(1)></p>', IMG)).toThrow();
    expect(() => expectInert('<a href="javascript:alert(1)">&lt;img src=x onerror=alert(1)></a>', IMG)).toThrow();
    expect(() => expectInert('<iframe></iframe>&lt;img src=x onerror=alert(1)>', IMG)).toThrow();
  });

  it('accepts the payload as escaped text, including inside attribute values', () => {
    expect(() => expectInert('<p title="&lt;img src=x onerror=alert(1)>">&lt;img src=x onerror=alert(1)></p>', IMG, 2)).not.toThrow();
  });
});

describe('links', () => {
  it.each(['javascript:alert(1)', 'file:///etc/passwd', 'smb://host/share', 'vscode://file/x', 'data:text/html,<b>x</b>'])(
    'gives %s no href',
    (url) => {
      const html = renderHtml(LinkRow, { link: link(IMG, url) });
      expect(html).not.toMatch(/href=/i);
      expectInert(html, IMG, 1);
    },
  );

  it('gives http and https links an href', () => {
    const html = renderHtml(LinkRow, { link: link('Design notes', 'https://www.notion.so/acme/Design-notes-1a2b') });
    expect(html).toContain('href="https://www.notion.so/acme/Design-notes-1a2b"');
  });
});

describe('language names', () => {
  it('ignores Object property names as extensions', () => {
    for (const ext of ['constructor', '__proto__', 'toString', 'hasOwnProperty']) {
      expect(languageForPath(`x.${ext}`)).toBeNull();
    }
  });

  it('shows only built-in language labels in the diff', () => {
    const html = renderHtml(DiffView, { files: [diffFile('plain'), diffFile('x', { new_path: `a.${IMG}` })] });
    expect(html).toContain('Rust');
    expectInert(html, IMG, 1);
  });
});
