-- Migration 1: the first schema. Times are Unix milliseconds.

CREATE TABLE blocks (
    id           INTEGER PRIMARY KEY,
    title        TEXT NOT NULL,
    title_source TEXT NOT NULL,            -- rename | pr | ticket | branch
    repo         TEXT,                     -- repo root folder
    archived     INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    last_active  INTEGER NOT NULL
);

CREATE TABLE block_keys (
    key      TEXT PRIMARY KEY,             -- pr:<owner>/<repo>#<n> | ticket:<KEY-1> | branch:<repo-root>@<branch>
    block_id INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE
);
CREATE INDEX block_keys_block ON block_keys(block_id);

CREATE TABLE chats (
    id              TEXT PRIMARY KEY,      -- Claude session ID
    title           TEXT,
    title_source    TEXT,                  -- custom | ai | prompt
    first_prompt    TEXT,
    recorded_branch TEXT,
    start_cwd       TEXT,
    last_cwd        TEXT,
    repo_root       TEXT,                  -- repo of the folder the chat started in
    start_worktree  TEXT,                  -- worktree of the folder the chat started in
    worktree_path   TEXT,                  -- worktree of the chat's most recent folder
    first_activity  INTEGER,
    last_activity   INTEGER,
    status          TEXT NOT NULL DEFAULT 'idle',  -- idle | working | needs_you | done | closed
    status_since    INTEGER,
    last_event_at   INTEGER,
    background      INTEGER NOT NULL DEFAULT 0,
    iterm_session   TEXT,
    block_id        INTEGER REFERENCES blocks(id) ON DELETE SET NULL,
    group_reason    TEXT,
    hidden          INTEGER NOT NULL DEFAULT 0,
    transcript_path TEXT,
    transcript_gone INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX chats_block ON chats(block_id);
CREATE INDEX chats_iterm ON chats(iterm_session);

CREATE TABLE chat_folders (
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    path    TEXT NOT NULL,
    PRIMARY KEY (chat_id, path)
);

-- Files a chat wrote with Write, Edit, MultiEdit or NotebookEdit.
CREATE TABLE chat_writes (
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    path    TEXT NOT NULL,
    count   INTEGER NOT NULL,
    last_at INTEGER,
    PRIMARY KEY (chat_id, path)
);

-- Worktrees a chat wrote into, with the branch each was on when first seen there.
-- Keeping that branch means a later branch switch never moves old chats.
CREATE TABLE chat_worktrees (
    chat_id   TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    worktree  TEXT NOT NULL,
    repo_root TEXT NOT NULL,
    branch    TEXT,
    writes    INTEGER NOT NULL,
    PRIMARY KEY (chat_id, worktree)
);

CREATE TABLE links (
    id              INTEGER PRIMARY KEY,
    block_id        INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,         -- github_pr | jira | notion | other
    url             TEXT NOT NULL,
    title           TEXT,
    status          TEXT,                  -- JSON, PRs only
    source          TEXT NOT NULL,         -- auto | manual
    added_by_claude INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    UNIQUE (block_id, url)
);

CREATE TABLE files (
    id         INTEGER PRIMARY KEY,
    chat_id    TEXT REFERENCES chats(id) ON DELETE CASCADE,
    block_id   INTEGER REFERENCES blocks(id) ON DELETE CASCADE,  -- set only for files added by hand
    path       TEXT NOT NULL,
    kind       TEXT NOT NULL,              -- doc | output
    found_by   TEXT NOT NULL,              -- tool | command | output | manual
    size       INTEGER,
    mtime      INTEGER,
    copy_path  TEXT,
    missing    INTEGER NOT NULL DEFAULT 0,
    first_seen INTEGER NOT NULL
);
CREATE UNIQUE INDEX files_chat_path ON files(chat_id, path) WHERE chat_id IS NOT NULL;
CREATE UNIQUE INDEX files_block_path ON files(block_id, path) WHERE block_id IS NOT NULL;

CREATE TABLE snapshots (
    id            INTEGER PRIMARY KEY,
    repo_root     TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    tree_id       TEXT,                    -- NULL once deleted to save space
    taken_at      INTEGER NOT NULL,
    reason        TEXT NOT NULL,           -- turn_start | turn_end | review
    skipped       TEXT                     -- JSON: what the snapshot left out
);
CREATE INDEX snapshots_worktree ON snapshots(worktree_path, taken_at);

CREATE TABLE turns (
    id             INTEGER PRIMARY KEY,
    chat_id        TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    prompt         TEXT,
    started_at     INTEGER NOT NULL,
    ended_at       INTEGER,
    worktree_path  TEXT,
    start_snapshot INTEGER REFERENCES snapshots(id),
    end_snapshot   INTEGER REFERENCES snapshots(id),
    files_changed  INTEGER,
    lines_added    INTEGER,
    lines_removed  INTEGER,
    parallel       INTEGER NOT NULL DEFAULT 0,
    missed         TEXT                    -- why a snapshot was missed
);
CREATE INDEX turns_chat ON turns(chat_id, started_at);

CREATE TABLE review_markers (
    id            INTEGER PRIMARY KEY,
    block_id      INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    worktree_path TEXT NOT NULL,
    snapshot_id   INTEGER NOT NULL REFERENCES snapshots(id),
    created_at    INTEGER NOT NULL
);

-- Manual changes. These are never rebuilt from transcripts.
CREATE TABLE overrides (
    id         INTEGER PRIMARY KEY,
    kind       TEXT NOT NULL,              -- chat_move | chat_started_from | block_rename | file_hide | file_add
    chat_id    TEXT,
    block_id   INTEGER REFERENCES blocks(id) ON DELETE CASCADE,
    path       TEXT,
    value      TEXT,
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX overrides_chat ON overrides(chat_id) WHERE kind IN ('chat_move', 'chat_started_from');
CREATE UNIQUE INDEX overrides_rename ON overrides(block_id) WHERE kind = 'block_rename';

CREATE TABLE worktrees (
    path        TEXT PRIMARY KEY,
    repo_root   TEXT NOT NULL,
    name        TEXT NOT NULL,
    branch      TEXT,
    detached_at TEXT,
    uncommitted INTEGER,                   -- -1 means "many" (over the time budget)
    ahead       INTEGER,
    behind      INTEGER,
    last_read   INTEGER,
    removed     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX worktrees_repo ON worktrees(repo_root);

CREATE TABLE repos (
    root           TEXT PRIMARY KEY,
    github         TEXT,                   -- owner/repo from the origin remote
    default_branch TEXT
);

CREATE TABLE prs (
    github          TEXT NOT NULL,
    number          INTEGER NOT NULL,
    title           TEXT,
    head_branch     TEXT,
    state           TEXT,                  -- OPEN | CLOSED | MERGED
    is_draft        INTEGER NOT NULL DEFAULT 0,
    review_decision TEXT,
    checks          TEXT,                  -- passing | failing | pending | none
    url             TEXT,
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (github, number)
);

CREATE TABLE keep_awake (
    id         INTEGER PRIMARY KEY,
    chat_id    TEXT,
    block_id   INTEGER,
    chats      TEXT NOT NULL,              -- JSON list of chat IDs covered
    started_at INTEGER NOT NULL,
    ended_at   INTEGER,
    end_reason TEXT                        -- done | max_hours | cancelled
);

CREATE TABLE ingest_offsets (
    path       TEXT PRIMARY KEY,
    inode      INTEGER NOT NULL,
    byte_offset INTEGER NOT NULL,
    session_id TEXT
);

CREATE TABLE bridge_windows (
    window_id TEXT PRIMARY KEY,
    block_id  INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
    chats     TEXT NOT NULL                -- JSON list of chat IDs open in the window
);

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
