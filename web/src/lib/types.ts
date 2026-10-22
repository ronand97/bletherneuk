// Shapes from docs/protocol.md, sections 2 and 3.
// Every field the protocol marks "or null" may also be missing, so those are optional here.

export type PageKind = 'board' | 'panel' | 'changes';

export type BlockId = number;
export type ChatId = string;

export type ChatStatus = 'idle' | 'working' | 'needs_you' | 'done' | 'closed';

export type ChecksState = 'passing' | 'failing' | 'pending' | 'none';

export interface ChatSummary {
  id: ChatId;
  title: string;
  status: ChatStatus;
  background?: boolean;
  block?: BlockId | null;
  block_title?: string | null;
  last_activity: number;
  status_since?: number | null;
  worktree?: string | null;
  branch?: string | null;
  now_on?: string | null;
  group_reason?: string | null;
  keep_awake?: boolean;
  can_resume?: boolean;
  resume_note?: string | null;
  hidden?: boolean;
  open_in_iterm?: boolean;
}

export interface PrBadge {
  number: number;
  url: string;
  state?: 'OPEN' | 'MERGED' | 'CLOSED' | null;
  draft?: boolean;
  checks?: ChecksState | null;
}

export interface ChatCounts {
  working: number;
  needs_you: number;
  done: number;
  idle: number;
  closed: number;
}

export interface BlockCard {
  id: BlockId;
  title: string;
  repo_name?: string | null;
  archived: boolean;
  last_active: number;
  pr?: PrBadge | null;
  ticket?: string | null;
  worktrees: string[];
  chats: ChatCounts;
  docs: number;
  outputs: number;
}

export interface Focus {
  block?: BlockId | null;
  chat?: ChatId | null;
}

export interface Board {
  now: number;
  iterm_connected: boolean;
  keep_awake: { holding: boolean; chats: number };
  needs_you: ChatSummary[];
  blocks: BlockCard[];
  unsorted: ChatSummary[];
  focus?: Focus | null;
}

export interface SearchResult {
  blocks: BlockCard[];
  chats: ChatSummary[];
}

export interface Worktree {
  id: number;
  name: string;
  path: string;
  is_main: boolean;
  branch?: string | null;
  detached_at?: string | null;
  uncommitted?: number | null;
  uncommitted_many?: boolean;
  ahead?: number | null;
  behind?: number | null;
  removed?: boolean;
}

export interface LinkStatus {
  state?: string | null;
  draft?: boolean;
  checks?: ChecksState | null;
  review?: string | null;
}

export type LinkKind = 'github_pr' | 'jira' | 'notion' | 'other';

export interface Link {
  id: number;
  kind: LinkKind;
  url: string;
  host?: string | null;
  title: string;
  status?: LinkStatus | null;
  source: 'auto' | 'manual';
  added_by_claude?: boolean;
}

export interface FileItem {
  id: number;
  name: string;
  folder: string;
  kind: 'doc' | 'output';
  found_by: 'tool' | 'command' | 'output' | 'manual';
  size?: number | null;
  modified?: number | null;
  missing?: boolean;
  copy?: boolean;
  openable?: boolean;
  hidden?: boolean;
}

export interface BlockInfo {
  id: BlockId;
  title: string;
  title_source: 'rename' | 'pr' | 'ticket' | 'branch';
  repo?: string | null;
  repo_name?: string | null;
  archived: boolean;
  last_active: number;
}

export interface BlockDetail {
  block: BlockInfo;
  worktrees: Worktree[];
  links: Link[];
  chats: ChatSummary[];
  docs: FileItem[];
  outputs: FileItem[];
  changes_since_review?: { worktree: number; files: number } | null;
  keep_awake: { block: boolean };
  new_chat_name: string;
}

export interface Turn {
  id: number;
  chat: ChatId;
  chat_title: string;
  prompt?: string | null;
  started_at: number;
  ended_at?: number | null;
  worktree?: number | null;
  files_changed?: number | null;
  lines_added?: number | null;
  lines_removed?: number | null;
  parallel: boolean;
  missed?: string | null;
  snapshot_deleted: boolean;
}

export interface Review {
  id: number;
  worktree: number;
  created_at: number;
}

export interface Timeline {
  turns: Turn[];
  reviews: Review[];
}

export type RangeKind = 'review' | 'branch' | 'turn';

export interface DiffLine {
  kind: '+' | '-' | ' ';
  old?: number | null;
  new?: number | null;
  text: string;
}

export interface Hunk {
  header: string;
  lines: DiffLine[];
}

export type FileStatus = 'added' | 'deleted' | 'modified' | 'renamed' | 'copied' | 'typechange';

export interface FileDiff {
  old_path?: string | null;
  new_path?: string | null;
  status: FileStatus;
  binary: boolean;
  additions: number;
  deletions: number;
  collapsed?: 'lockfile' | 'minified' | 'large' | null;
  truncated: boolean;
  unavailable: boolean;
  hunks: Hunk[];
}

export interface NotInDiffItem {
  path: string;
  reason: string;
}

export interface DiffResponse {
  worktree?: number | null;
  range: { kind: RangeKind; from_label: string; to_label: string };
  /** The snapshot shown as "now"; Mark reviewed records exactly this. */
  to_tree?: string | null;
  files: FileDiff[];
  totals: { files: number; additions: number; deletions: number };
  not_in_diff: NotInDiffItem[];
  parallel: boolean;
  unavailable?: string | null;
}

export interface ThemeResponse {
  vars: Record<string, string>;
}

export interface ChangedEvent {
  blocks?: BlockId[];
  board?: boolean;
}

export interface BridgeEvent {
  connected: boolean;
}

export type ActionResult = { ok: true } | { ok: false; error: string };

/** A block a chat can be moved to, as shown in a "Move to…" menu. */
export interface MoveTarget {
  id: BlockId;
  title: string;
}
