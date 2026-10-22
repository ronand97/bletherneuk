# bletherneuk — design

- **Date:** 2026-10-22
- **Status:** Draft, awaiting review
- **Command:** `blether`

"Blether" is Scots for a chat, and a "neuk" is a nook or corner. bletherneuk is the corner where your Claude Code chats are kept in order.

## 1. What this is

bletherneuk is a macOS companion for people who run [Claude Code](https://code.claude.com) in [iTerm2](https://iterm2.com). It groups Claude Code chats into **work blocks**. A work block is one piece of work, usually tracked by a pull request, a Jira ticket or a Notion page. For each block it shows:

- the chats that belong to it, with live status, each one click from being resumed;
- its PR (with live status and checks), ticket and other links;
- the docs (specs, handovers) and outputs (HTML reports, CSVs, maps) its chats produced;
- a **Changes** view of everything that changed in the code since you last looked.

It runs on top of iTerm2 rather than replacing it. A background service builds the picture. A small iTerm2 script adds a panel to iTerm2's right-hand toolbelt and opens windows, tabs and browser panes. Claude Code hooks report what each chat is doing.

## 2. The problem

- One piece of work usually spans several Claude chats over days. Finding the right one means scrolling the `claude --resume` picker and remembering which folder it ran in.
- Chats produce specs, reports, CSVs and maps. These end up spread across docs folders, git worktrees and temporary scratch folders that macOS deletes.
- Agents commit as they go. A branch can be well over a hundred commits ahead of `main`. Working out "what changed since I last looked" from commits or from the uncommitted diff doesn't work.
- With several agents running at once, it is hard to see which one is waiting for you.

## 3. Goals

1. One board shows every active work block and every chat waiting for you. One click jumps to it.
2. Every chat in a block is listed and can be resumed in one click, in the right folder.
3. Every doc and output a block's chats produced is listed and opens in one click, including files from temporary folders.
4. A block's changes can be reviewed per turn or since your last review, in a pane next to the chat.
5. Fonts, colours and sizes are fully customisable.
6. It never slows Claude Code down, never loses anything you entered by hand, and is safe to leave running all day.

## 4. Scope

**In the first version**

- Index service, iTerm2 bridge, Claude Code hook and command line (sections 6–16).
- Home board, toolbelt panel and Changes page.
- Clue-based grouping with manual moves.
- Branch and worktree tracking, including starting a chat in a new worktree.
- "Keep the Mac awake until this chat is done".
- PR status through the GitHub CLI (`gh`).
- Jira, Notion and other links shown as plain links.
- Theme file, including colours taken from the current iTerm2 profile.

**Not in the first version**

- **Background suggestions.** A scheduled, non-interactive Claude run (`claude -p`) that reads recent chats and suggests CLAUDE.md rules, skills or hooks. Plain warning badges for stale PRs or failing checks would sit alongside it.
- **Live Jira and Notion status.** This needs API tokens: an Atlassian scoped token with `read:jira-work`, and a Notion internal integration. Links are stored so this can be added later without a redesign.
- **AI-assisted grouping** of chats left in Unsorted.
- **Drag-and-drop** to move chats. A "Move to…" menu does the job.
- **A standalone desktop app** with its own terminal. The index service is written so that it can be reused by one (for example a Tauri app).
- **Terminals other than iTerm2, and platforms other than macOS.** Only the bridge is iTerm2-specific.

## 5. Tested with

| Component | Version |
|---|---|
| macOS | 26.6.2 |
| iTerm2 | 3.7.6 |
| iTerm2 browser plugin | 1.0 |
| `iterm2` Python package | 2.30 |
| Claude Code | 2.1.307 |

`blether doctor` warns when iTerm2 is older than 3.7.6. It names the features that may not work (mainly browser panes) and says whether the browser fallback is in use.

**Browser panes rely on an undocumented iTerm2 behaviour.** iTerm2's [web browser docs](https://iterm2.com/documentation-web.html) say there are "no browser-specific APIs yet". But iTerm2 opens a browser session when a new session's profile has `"Custom Command": "Browser"` and an `"Initial URL"`. iTerm2's own test suite opens browser windows this way ([`tests/browser_eval_smoke.py`](https://github.com/gnachman/iTerm2/blob/master/tests/browser_eval_smoke.py)). A throwaway test on 2026-10-22 confirmed that a script can open both a browser tab and a browser split pane this way, and that both load a local page. If a future iTerm2 version changes this, the bridge falls back to the default browser (section 14).

## 6. Architecture

```
  Claude Code ──hook events──▶ blether hook ─┐
                                             │  Unix socket
  ~/.claude/projects/**/*.jsonl ──watch──────┤
                                             ▼
                                   ┌────────────────────┐
  gh (GitHub CLI) ◀──── polls ─────│   blether serve    │── SQLite, snapshots,
                                   │  (index service)   │   copies of temp files
                                   └────────────────────┘
                                     ▲               │ HTTP + live updates
                         Unix socket │               ▼ (127.0.0.1 only)
                       ┌─────────────┴──┐    ┌──────────────────────────────┐
  iTerm2 ◀─Python API──│  iTerm2 bridge │    │ board · panel · Changes page │
                       └────────────────┘    │ (toolbelt, browser panes, or │
                                             │  the default browser)        │
                                             └──────────────────────────────┘
```

| Part | What it is | Runs |
|---|---|---|
| Index service | `blether serve`, Rust | Always, as a launchd user agent (`dev.bletherneuk.serve`) |
| iTerm2 bridge | `bletherneuk_bridge.py`, Python, about 200 lines | Whenever iTerm2 runs, from iTerm2's AutoLaunch folder |
| Hook | `blether hook`, the same Rust binary | Once per Claude Code hook event, for a few milliseconds |
| Command line | `blether <command>` | On demand |
| Web pages | Svelte + TypeScript, built into the binary | In iTerm2's toolbelt, iTerm2 browser panes or the default browser |

**Why these choices**

- **Rust for the service.** It runs all day, so a small, fast process matters. It installs as one binary with the pages built in. A future desktop app would most likely use Tauri, which is Rust, so the service carries over. Go would also work but has no such path. Node would put one language across service and pages but is a heavier always-on process.
- **Python for the bridge.** iTerm2's API only has an official Python client. The bridge stays thin: it translates requests into iTerm2 API calls and reports iTerm2 events, with no logic of its own.
- **Svelte for the pages.** It is small, escapes text by default and builds to static files that the binary embeds.
- **libgit2 for all git work.** Through the `git2` crate it reads repos and takes and diffs snapshots. libgit2 never runs hooks, fsmonitor commands, or external filter, textconv or diff programs. The `git` command line can run all of those from a repo's own config (section 20.4).
- **Main libraries:** `tokio` (async runtime), `axum` (HTTP), `rusqlite` with bundled SQLite (storage), `notify` (FSEvents file watching), `git2` (git), `serde`/`serde_json` (transcripts and messages), Shiki (syntax highlighting in pages).

**How the parts talk**

- The **hook, bridge and command line** talk to the service over one Unix socket, `~/Library/Application Support/bletherneuk/run/blether.sock`. Messages are newline-delimited JSON.
- **Pages** talk to the service over HTTP on `127.0.0.1` at a random port. Live updates arrive as server-sent events (SSE). Pages never learn the port or keys by themselves. The bridge or the command line asks the service for a one-time page URL over the socket, then opens it (section 20.2).

## 7. What the service stores

All state lives in SQLite at `~/Library/Application Support/bletherneuk/index.db`. Each change to the schema is a numbered migration.

| Table | Holds |
|---|---|
| `chats` | Claude session ID, title, folders seen, recorded branch, repo root, first and last activity, live status, iTerm2 session ID if open, block ID, `group_reason`, hidden flag |
| `blocks` | ID, title, title source, repo, archived flag, created and last-active times |
| `block_keys` | The keys that map to a block: `pr:<owner>/<repo>#<n>`, `ticket:<KEY-123>`, `branch:<repo-root>@<branch>` |
| `links` | Block ID, kind (`github_pr`, `jira`, `notion`, `other`), URL, title, status (PRs only), source (`auto` or `manual`) |
| `files` | Block ID, chat ID, path, kind (`doc` or `output`), how it was found, size, modified time, path of the saved copy if any, hidden flag |
| `turns` | Chat ID, prompt text, start and end times, start and end snapshot IDs, and the files changed and lines added and removed (saved when the turn ends, so they outlive deleted snapshots) |
| `snapshots` | ID, repo, worktree path, git tree ID, time, reason (`turn_start`, `turn_end`, `review`) |
| `review_markers` | Block ID, worktree, snapshot ID, time |
| `overrides` | Your manual changes: chat moves, block renames, hidden files, added files, and the block a chat was started from |
| `worktrees` | Repo root, path, name, current branch (or detached commit), uncommitted file count, commits ahead of and behind the default branch, last read time, removed flag |
| `keep_awake` | Chat ID, or block ID with the chats it covers, plus start time, end time and why it ended |
| `ingest_offsets` | Transcript path, inode, byte offset read so far |
| `bridge_windows` | iTerm2 window ID ↔ block ID, and which chats were open in it |

Almost everything can be rebuilt by rescanning transcripts (`blether rescan`). The exceptions are links you added, `overrides` and `review_markers`. Once a day, and before every migration, these are written to `~/Library/Application Support/bletherneuk/backups/manual-<date>.json`. The last 30 backups are kept.

## 8. Reading transcripts

Claude Code writes one JSONL transcript per chat at `~/.claude/projects/<encoded-folder>/<session-id>.jsonl`. Subagent transcripts sit in a folder named after the session.

- **Watching.** The service watches `~/.claude/projects` with FSEvents. On a change it reads from the saved byte offset. If the file shrank or its inode changed, it reads it again from the start.
- **What it reads from each line:**
  - `type`, `sessionId`, `cwd`, `gitBranch`, `timestamp`.
  - The title, in this order: `custom-title` (`customTitle`), then the latest `ai-title` (`aiTitle`), then the first user prompt cut to 80 characters.
  - User prompts: `type: "user"` lines whose content is text, not a tool result.
  - Assistant `tool_use` blocks: `file_path` for `Write`, `Edit`, `MultiEdit` and `NotebookEdit`, and `command` for `Bash`.
  - `tool_result` text, scanning only the first 256 KB of each result, for file paths.
- **Subagent transcripts** are read for file writes, which are credited to the parent chat. Subagents are not listed as chats.
- **Robustness.** Unknown `type` values are ignored. Lines that fail to parse are skipped and counted. Lines over 8 MB are skipped without parsing. `blether doctor` shows the counts. The transcript format is not a public API and changes between Claude Code versions, so the parser only depends on the fields above. Every field is optional.
- **Hidden chats.** A chat is hidden by default when it has no branch (or `HEAD`) and its folder is inside a temporary directory (`/private/tmp`, `/tmp`, `/private/var/folders`). These are mostly scheduled or automated runs. Hidden chats can still be found with the archive search.

## 9. Grouping chats into blocks

Each chat is placed by the first clue that matches.

1. **Manual move.** You moved it with "Move to…". This always wins and is never undone by later clues.
2. **PR reference** in the chat's title or first prompt: `PR #123`, `PR 123` or `pr123` (case-insensitive), or a URL like `github.com/<owner>/<repo>/pull/123`. Without a URL, the PR belongs to the GitHub repo of the chat's folder (from the `origin` remote).
3. **Ticket key** in the title, the first prompt or the branch name. Only prefixes listed in `tickets.projects` in the config count, for example `["ABC"]`. A generic pattern would also match `UTF-8`, `SHA-256` and `ISO-8601`. With no prefixes configured, this clue is off. In branch names, `_` is accepted in place of `-` (`feat/abc_123_retry` → `ABC-123`).
4. **Worktree drift.** The chat wrote files inside a different git worktree from the folder it started in. It takes that worktree's branch. If it wrote into several, the one with the most writes wins. Claude Code records the branch a chat starts on and never updates it, so without this clue such chats land in the wrong place.
5. **Recorded branch**, unless it is the repo's default branch or `HEAD`. The default branch is read from `refs/remotes/origin/HEAD`, falling back to `main` and then `master`.
6. Otherwise the chat goes to **Unsorted**.

Every chat stores a `group_reason`, such as "title mentions PR #42" or "wrote files in worktree feature-x". The panel shows it next to the chat, so wrong guesses are easy to spot and move.

**Creating and joining blocks**

- A block is created the first time a new key turns up: a branch, PR or ticket.
- Keys join one block when they are linked. A PR's head branch (from `gh`) joins its `pr` and `branch` keys. A ticket key in a PR's title or head branch joins the `ticket` key too. If joining would bring two existing blocks together, they merge. Links, files, overrides and review markers are combined.
- **Block title**, in this order: your rename, the PR title, the ticket key, the branch name.
- **Archive.** A block is archived when its PR is merged or closed and it has had no chat activity for 14 days (`archive.idle_days`). Any new activity in one of its chats brings it back.

Clues are re-evaluated whenever new information arrives, such as a new transcript line or a PR's head branch from `gh`. Manual moves are never re-evaluated.

**Chats started from a block.** When you start a chat from a block's panel, the bridge tags its iTerm2 session with the block. The first hook event from that session assigns the chat to that block, and the assignment is stored like a manual move. This covers new chats in a new worktree, whose branch doesn't exist anywhere else yet.

### Branches and worktrees

Many chats run in git worktrees, often Claude Code's own `.claude/worktrees/<name>` folders created by `claude --worktree`. Transcripts only record the branch a chat started on, so the service reads worktrees directly from git through libgit2.

- **Discovery.** For every repo that chats ran in, the service lists the main checkout and every linked worktree. For each it records:
  - the path and name (the folder name);
  - the current branch, or "detached at `<short commit>`";
  - the number of uncommitted files;
  - commits ahead of and behind the default branch.
- **Keeping it current.**
  - The service watches each worktree's `HEAD` and index files, and re-reads the branch when they change.
  - It counts uncommitted files at most every 30 seconds per worktree, and also when a block gains focus. Each count has a time budget (section 20.9).
  - A worktree that has disappeared is marked removed. It isn't deleted from the database.
- **Which worktree a chat is in:** the one that contains the chat's most recent folder.
- **Grouping vs display.**
  - Grouping uses the branch the chat actually worked on, from the transcript and the worktree-drift clue. If a worktree later switches branch, old chats stay with the work they belong to.
  - The display shows the worktree's *current* branch, and adds "now on `<branch>`" when it differs from the chat's.
- **A block's worktrees** are those whose current branch is one of the block's branch keys, plus any worktree its chats ran in.
- **Removed worktrees.** Chats whose worktree has been removed are marked "worktree removed". The build will check whether `claude --resume` finds such a chat when run from the main checkout. If it does, the resume button runs it there. If not, the button is disabled with an explanation.

## 10. Docs and outputs

- **Docs** are `.md` files that a block's chats wrote or edited. These are left out: anything under `~/.claude/` (Claude's memory, skills and settings), and files named `CLAUDE.md` or `AGENTS.md`.
- **Outputs** are files with these extensions (configurable): `html`, `csv`, `geojson`, `parquet`, `png`, `svg`, `xlsx`, `pdf`. A file counts as an output when either:
  - a chat wrote it with a tool, or
  - its path appears in a `Bash` command or in tool output, and the file exists on disk. Absolute paths and `~` paths are taken as they are. Relative paths are resolved against the chat's folder.
- **Ignored:** anything preceded by `http://` or `https://`, and paths inside `.venv`, `venv`, `node_modules`, `site-packages`, `.git`, `~/.claude` or `~/Library`.
- **Code files** are not listed. The PR shows them.
- **Copies of temporary files.** When a doc or output is found inside a temporary directory, the service copies it into `~/Library/Application Support/bletherneuk/copies/<block-id>/`. The limits are 200 MB per file (`outputs.copy_max_mb`) and 5 GB in total (`outputs.copies_max_total_gb`). When the total limit is reached, copies from archived blocks are removed first, then the oldest. When the original is gone, the panel opens the copy and marks it as a copy. Section 20.5 sets which files may be copied.
- **Opening.** Docs and outputs open with the default app for their extension. Section 20.5 covers how this is done safely.
- **Manual changes.** In the panel you can add a file by hand or hide one. Files added by hand with an extension outside the lists above are only shown in Finder, not opened.

## 11. Links and PR status

URLs are recognised by pattern:

| Pattern | Kind | Title shown |
|---|---|---|
| `https://github.com/<owner>/<repo>/pull/<n>` | `github_pr` | PR title from `gh` |
| `https://<site>.atlassian.net/browse/<KEY-123>` | `jira` | The ticket key, or a title you type |
| `https://www.notion.so/...`, `https://*.notion.site/...` | `notion` | Words from the URL's slug (`Design-notes-1a2b…` → "Design notes"), or a title you type |
| any other `https://` or `http://` URL | `other` | The host and path, or a title you type |

- **Adding links.** Paste a URL into the panel's "+ link" box, or run `blether link <url> [--title "…"]`. The command line attaches the link to the block of the iTerm2 tab it runs in (`ITERM_SESSION_ID` → chat → block), or to `--block <id>`. Claude can run it too.
- **Which URLs are accepted.** Only `https://` and `http://` URLs are accepted. This is checked when a link is saved, when it is shown and when it is opened (section 20.5). The panel shows each link's host next to its title. Links added by `blether link` from inside a Claude chat are marked "added by Claude".
- **PR status.** For each GitHub repo that chats ran in, the service runs `gh pr list -R <owner>/<repo> --state all --limit 200 --json number,title,headRefName,isDraft,state,reviewDecision,statusCheckRollup,url` every 5 minutes. It runs from a neutral folder, never from inside a repo (section 20.4). It also refreshes one repo when one of its blocks gains focus, at most once a minute. Checks are summarised as passing, failing or pending.
- **Ticket links from keys.** When `tickets.url_template` is set (for example `https://example.atlassian.net/browse/{key}`), each ticket key a block picks up becomes a Jira link automatically.

## 12. Live status

`blether install` registers `blether hook` for these Claude Code hook events: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop` and `SessionEnd`.

| Event | Chat status |
|---|---|
| `SessionStart` | idle |
| `UserPromptSubmit`, `PreToolUse`, `PostToolUse` | working |
| `Notification` (permission request or question) | needs you |
| `Stop` | done, which becomes idle once the bridge reports you've focused the tab |
| `SessionEnd` | closed |

- **Linking chats to tabs.** Every event carries the Claude session ID. The hook adds `ITERM_SESSION_ID` from its environment, since Claude runs inside that iTerm2 session. Its UUID part matches the iTerm2 API's session ID. This links a chat to its tab from the first event, including for chats started by hand.
- **What the hook does.** It reads the event JSON from stdin (up to 1 MB). It adds `ITERM_SESSION_ID` and the time. It sends one line to the socket, with a 50 ms limit on connecting and writing. It always exits 0. **It never writes to stdout.** For some events, Claude Code adds a hook's stdout to the chat's context.
- **Stale status.** If a chat has been "working" for 10 minutes with no events and its iTerm2 session no longer exists, it is marked closed.
- **Background work.** A chat can keep working after a turn ends, through background shell tasks or subagents it launched. iTerm2's integration shows that recent Claude Code versions report this. During the build, the status machine will treat that background work as "working", using whichever events Claude Code sends for it.
- **Living alongside other hooks.** iTerm2's own `cc-status` hook and any other hooks stay as they are.

### Keep awake until done

This replaces running `caffeinate` by hand and guessing how long a chat will take.

- **Turning it on.**
  - A working chat's row in the panel has a "Keep awake until done" button.
  - The block header has "Keep awake until all chats here are done". That covers the chats working at the moment you press it.
- **While on.**
  - While at least one chosen chat is working, the service runs one `/usr/bin/caffeinate -i -w <service PID>` process, with arguments passed as a list. `-i` stops idle sleep, and the screen can still turn off.
  - Because of `-w`, `caffeinate` exits if the service dies, so the Mac can never get stuck awake.
- **When it ends.**
  - A chat counts as done when it has stopped working (finished, needs you, or closed) and stayed stopped for 2 minutes (`keep_awake.grace_seconds`). The pause covers background work that briefly restarts.
  - The request then ends by itself, and `caffeinate` stops when no requests are left.
  - A request also ends after 12 hours (`keep_awake.max_hours`), so a stuck status can't hold the Mac awake indefinitely.
- **What you see.**
  - The board shows "Keeping awake for *N* chats".
  - Chats being kept awake show an icon.
  - `blether doctor` reports whether the service is currently holding the Mac awake.
- **After a restart.** Active requests are stored in the database. If the service restarts, it restores them and starts `caffeinate` again.
- **Limit.** Closing the lid still puts most Macs to sleep. `caffeinate` can't prevent that, and the panel says so next to the button.

## 13. Snapshots and the Changes page

A **snapshot** is a git tree object recording the content of a worktree's code at one moment. It never touches the working files, the branch or the staging area.

**What a snapshot includes**

- **Every file git tracks**, meaning every file in the worktree's index, including newly staged ones, at its current content.
- **New files a chat wrote itself.** These are untracked files that a chat in this worktree created or changed with its `Write`, `Edit`, `MultiEdit` or `NotebookEdit` tools, taken from the transcript. Two kinds are left out: files with an extension on the outputs list (section 10), and files over 1 MB (`snapshots.new_file_max_mb`). Once such a file is in a snapshot, later snapshots keep including it until it is deleted, tracked or ignored. Otherwise it would look deleted in the next diff.
- **Nothing else.** Files that scripts generate (reports, CSVs, maps, caches) are never included, however often they are regenerated. Outputs are listed in the panel instead (section 10).

**When snapshots are taken**

- At **turn start** (`UserPromptSubmit`) and **turn end** (`Stop`), for the worktree containing the chat's folder. At turn end, also for any other worktree the chat wrote into during the turn.
- When you click **Mark reviewed** (`review`).
- The hook only forwards the event. The service takes the snapshot in the background, so Claude is never kept waiting.

**How a snapshot is taken.** Everything goes through libgit2. No `git` command is ever run inside a user's repo (section 20.4).

- **A shadow store per repo.** Snapshot data goes into a bare repo the service owns, at `~/Library/Application Support/bletherneuk/snapshots/<repo-id>.git`. The user repo's object database is added as a read-only alternate, so file contents git already has are not copied again. Snapshot data never enters the user's repo, so it can never be pushed. It adds nothing to the repo's size.
- **Steps:**
  1. The service opens the shadow store, with the worktree set as its working directory.
  2. It loads a temporary copy of the worktree's own index, so tracked files that haven't changed since git last saw them are not hashed again.
  3. It updates the tracked files that have changed, and adds the chat-written new files described above. New contents are written into the shadow store.
  4. It writes a tree and records its ID.
- **Files that are skipped:**
  - **Large tracked files.** A tracked file over 5 MB that has changed (`snapshots.tracked_max_mb`), such as a committed data file, keeps its previous content in the snapshot. The diff lists it as "changed, too large to snapshot".
  - **Secret-looking files.** Files whose names look like secrets (`snapshots.skip_patterns`, section 20.4).
- **Limits per snapshot.** Each snapshot has a budget of new data, file count and time (section 20.9).
- **Limit per repo.** Each repo's shadow store is limited to 2 GB (`snapshots.max_store_gb`). When it goes over:
  1. The service deletes the oldest snapshots taken before their block's latest review marker.
  2. If the store is still over, it deletes the oldest remaining snapshots, but always keeps the latest snapshot of each worktree and every review marker's snapshot.
  3. It prunes unreferenced objects.

  Each turn's change counts (files changed, lines added and removed) are saved in the `turns` table when the turn ends. So the timeline keeps its numbers after a snapshot is deleted. Opening a deleted turn's diff shows "snapshot deleted to save space".
- **Keeping and pruning.** Each snapshot tree is kept alive by a ref in the shadow store (`refs/snap/<snapshot-id>`). Snapshots of archived blocks are deleted after 30 days (`snapshots.keep_days_after_archive`). Unreferenced objects are then pruned from the shadow store.
- **Known limitation.** A snapshot can refer to file contents that live only in the user's repo, through the alternates link. If the user rewrites history and the repo's own `git gc` later removes those contents, older diffs show "content no longer available" for the files affected. Git keeps unreachable objects for two weeks by default, and agents' commits keep most contents reachable.

**The Changes page.** It opens in a browser split pane to the right of the chat, and reuses that pane if it's already open.

- **Timeline** of every turn in every chat of the block, newest first. Each entry shows the first 200 characters of the prompt, the chat title, the time, the files changed and lines added or removed. The numbers come from a libgit2 tree-to-tree diff between the turn's start and end snapshots. Review markers appear in the timeline.
- **Range picker:**
  - since last review (the default): from the block's latest review marker to now;
  - one turn: from its start snapshot to its end snapshot;
  - whole branch: from the merge base with the default branch to now.
- **Parallel chats.** When another chat was active in the same worktree during a turn, the turn shows a note: "other chats were working here; this diff may include their changes".
- **Diff.** The service diffs the two trees with libgit2, with rename detection and no textconv or external diff programs. It sends the result to the page as structured JSON (files, hunks, lines). The page highlights it with Shiki. Lockfiles, minified files and files over 5,000 changed lines start collapsed. Binary files are listed but not shown.
- **Not in this diff.** A list under the diff shows what a snapshot doesn't cover, taken from the chats' tool calls and the snapshot itself. It lists:
  - files the chats wrote outside the worktree (for example `~/.zshrc`), inside `.git/`, or that are ignored;
  - new files left out because of their extension or size;
  - tracked files that changed but were too large to snapshot;
  - files skipped because they look like secrets.

  Files created by scripts aren't listed here. Outputs appear in the panel's Outputs list. "Mark reviewed" only covers what the diff shows, and this list makes the gap visible.
- **Mark reviewed** takes a `review` snapshot and moves the marker to it.

## 14. iTerm2 bridge

The bridge is a basic iTerm2 script. It lives at `~/Library/Application Support/iTerm2/Scripts/AutoLaunch/bletherneuk_bridge.py` and runs on iTerm2's bundled Python runtime, which includes the `iterm2` package. It needs **Settings → General → Magic → Enable Python API** switched on.

**What it reports to the service**

- `hello`: the iTerm2 version, the current profile's colours (for `match_iterm2`), and every session's ID, window and tab.
- `focus`: which session, tab and window is active, using iTerm2's `FocusMonitor`.
- `session_opened` and `session_closed`.

**What it does for the service**

| Request | What happens |
|---|---|
| `open_chat(chat)` | Focuses the chat's tab if it's open. Otherwise it creates a tab in the block's window (creating the window if needed). The tab uses the default profile with `"Custom Directory": "Yes"` and `"Working Directory": <folder>`, so the folder never passes through a shell. The folder is checked first (section 20.6). The bridge then sends `claude --resume <uuid>` followed by a newline. This runs in your normal shell, so your PATH and virtualenvs work. |
| `new_chat(block, where)` | Creates a tab in the block's window and tags its session with the block (section 9). `where` is one of:<br>• the block's worktree, running `claude` there;<br>• the main checkout, running `claude` there;<br>• a new worktree, running `claude --worktree <name>` from the main checkout. The name is suggested from the block title and must match `^[A-Za-z0-9._-]{1,64}$`. |
| `open_block(block)` | Focuses the block's window. If it is closed, it opens a window with the chats that were open when you last closed it, or the most recent chat if none were. |
| `open_page(url, near_session)` | Splits the session vertically with a browser profile (`"Custom Command": "Browser"`, `"Initial URL": <url>`), or reuses the split it opened before. The board instead opens in its own window. |
| `set_titles` | Names windows after blocks and tabs after chats. iTerm2 treats titles as interpolated strings that can call functions, so backslashes are escaped first (section 20.6). |

- **Remembering which window belongs to which block.** The bridge sets the iTerm2 variables `user.bletherneuk_block` on windows and `user.bletherneuk_chat` on sessions, so it can rebuild the mapping after a restart.
- **Fallback.** If creating a browser session fails, or iTerm2 is older than 3.7.6, the bridge asks the service to open the page in the default browser with `open`.
- **Toolbelt panel.** The bridge registers it with `async_register_web_view_tool` under the name "bletherneuk" and the identifier `dev.bletherneuk.panel`.
- **Open-board shortcut.** The bridge registers an `@iterm2.RPC` function, `bletherneuk_open_board()`. You bind it to a key under Settings → Keys → "Invoke Script Function".
- **Reconnecting.** If the service isn't running, the bridge retries with backoff up to every 30 seconds.

## 15. Pages and theming

**Home board** (its own iTerm2 browser window, or the default browser)

- A **Needs you** strip lists chats waiting on you, oldest first. Clicking one focuses its tab.
- When the service is keeping the Mac awake, a line says "Keeping awake for *N* chats".
- **Block cards** are sorted by last activity. Each shows:
  - the title, a PR badge (draft, open or merged, with check status) and the ticket key;
  - worktree names;
  - chat counts by status;
  - doc and output counts;
  - when it was last active.

  Clicking a card runs `open_block`.
- There is an **Unsorted** card, an **Archive** view and a **search** box over block and chat titles.
- Each chat has a **Move to…** menu.

**Toolbelt panel** (follows the focused tab)

- **In a block's tab:**
  - the block's title and repo;
  - **Worktrees:** each one's name, current branch, uncommitted file count and commits ahead of the default branch, for example `retry-backoff · feat/retry-backoff · 3 uncommitted · 12 ahead`;
  - **Tracking:** links, with PR status, plus a "+ link" box;
  - **Chats:** for each one, its status, title, worktree (or "main checkout") and current branch, plus `group_reason`, a "Keep awake until done" button while it is working, and a click to focus or resume;
  - **Docs** and **Outputs:** click to open with the default app, with a marker when a saved copy is used;
  - "*N* files changed since your last review", which opens the Changes page;
  - **Buttons:**
    - New chat, with a menu: this worktree, main checkout, or a new worktree;
    - "Keep awake until all chats here are done";
    - rename;
    - add file and hide file.
- **In any other tab:** a compact board.

**Changes page:** see section 13.

**Theming.** One file, `~/.config/bletherneuk/theme.toml`, holds the theme:

```toml
base = "soft-dark"        # built in: "soft-dark", "soft-light"
match_iterm2 = true       # take background, text and the 16 ANSI colours from the current iTerm2 profile

[font]
ui = "Inter"
code = "JetBrains Mono"
size = 13
line_height = 1.5

[colors]                  # anything set here overrides base and match_iterm2
accent = "#8aadf4"
needs_you = "#ed8796"
working = "#eed49f"
done = "#a6da95"
diff_added_bg = "#a6da9526"
diff_removed_bg = "#ed879626"

[layout]
density = "comfortable"   # or "compact"
radius = 6
```

Pages use CSS custom properties. When the file is saved, the service reloads it and pushes it to every open page.

## 16. Command line

| Command | Does |
|---|---|
| `blether serve` | Runs the index service. launchd normally starts it. |
| `blether hook` | Hook entry point (section 12). |
| `blether install [--dry-run]` | Installs everything (section 18). `--dry-run` prints what would change. |
| `blether uninstall` | Removes everything `install` added. Leaves the database unless `--purge` is given. |
| `blether doctor [--live]` | Checks the setup. `--live` also opens and closes a browser pane in iTerm2. |
| `blether link <url> [--title …] [--block …]` | Attaches a link. |
| `blether board` | Opens the home board. |
| `blether rescan` | Re-reads all transcripts. Manual data is kept. |

## 17. Configuration

All personal settings live outside the repo, in `~/.config/bletherneuk/config.toml`. Every key is optional.

```toml
[tickets]
projects = ["ABC"]                                          # ticket-key prefixes to recognise
url_template = "https://example.atlassian.net/browse/{key}"

[chats]
temp_dirs = ["/private/tmp", "/tmp", "/private/var/folders"]

[outputs]
extensions = ["html", "csv", "geojson", "parquet", "png", "svg", "xlsx", "pdf"]
copy_max_mb = 200
copies_max_total_gb = 5

[snapshots]
new_file_max_mb = 1
tracked_max_mb = 5
max_store_gb = 2
keep_days_after_archive = 30
skip_patterns = [".env*", "*.pem", "*.key", "*.p12", "*.pfx", "id_rsa*", "id_ed25519*", "*credentials*.json", "*service-account*.json"]
max_new_data_mb = 500
max_files = 200000
max_seconds = 60

[archive]
idle_days = 14

[github]
poll_minutes = 5

[keep_awake]
grace_seconds = 120
max_hours = 12

[worktrees]
status_interval_seconds = 30
```

## 18. Install and uninstall

v1 is built from source. `make install` builds the pages (`npm ci && npm run build` in `web/`), builds the release binary with the pages embedded, copies `blether` to `~/.local/bin`, then runs `blether install`.

`blether install`:

1. **Claude Code settings.**
   - It reads `~/.claude/settings.json`, and refuses to continue if the file isn't valid JSON.
   - It writes a backup next to it (`settings.json.bletherneuk-backup-<timestamp>`, mode 0600, since the file can hold tokens).
   - It adds one hook entry per event. Each entry's command is the quoted absolute path to the installed binary followed by `hook`. That way a `blether` inside a repo's `.venv/bin` or `node_modules/.bin` can never stand in for it.
   - It re-reads the file just before writing, in case something else changed it. It writes the new file to a temporary path and renames it over the original, so a crash can't leave it half-written.
   - Existing hooks are left untouched. `uninstall` removes only entries whose command is that `blether hook` path.
2. **iTerm2 bridge.** It copies the bridge into iTerm2's AutoLaunch folder. It checks that the Python API is enabled, and tells you how to enable it if not. It never turns on iTerm2's "Allow all apps to connect" setting.
3. **Background service.** It writes `~/Library/LaunchAgents/dev.bletherneuk.serve.plist` and loads it with `launchctl bootstrap gui/<uid>`. The plist runs the service at login, restarts it on crash (`KeepAlive`), and sends logs to `~/Library/Application Support/bletherneuk/logs/`. It points at the installed copy in `~/.local/bin`, never at a build folder.
4. **Config.** It writes starter `config.toml` and `theme.toml` files if they don't exist.

## 19. Failure handling

| Failure | Behaviour |
|---|---|
| Transcript line can't be parsed, or has an unknown type | Skipped and counted. Shown by `doctor`. |
| `gh` missing, logged out or rate-limited | PR badges show "unknown". Retries with backoff. Pages never wait on GitHub. |
| iTerm2 closed or the bridge disconnected | The service keeps running. Panel buttons say "iTerm2 not connected". The bridge reconnects on its own. |
| Service not running | The hook gives up within 50 ms and exits 0. Pages show a "reconnecting" overlay and reconnect with backoff. |
| Snapshot fails (not a git repo, repo busy, git error) | The turn shows "snapshot missed". Nothing else is affected. |
| Browser pane can't be created | The page opens in the default browser. |
| File gone and no saved copy | Listed as "missing" and kept until you hide it. |
| Database migration fails | The service refuses to start and points `doctor` at the pre-migration backup. It never runs on a half-migrated database. |

## 20. Security

This section combines the author's own review with an independent adversarial review of the design. Behaviour claims marked "verified" were tested on macOS 26.6 with git 2.50.1.

### 20.1 Threat model

**In scope:**
- websites open in any browser;
- other pages served on `127.0.0.1` from other ports, such as dev servers, notebooks, generated HTML served with `python -m http.server`, or a compromised npm dev dependency;
- untrusted content that reaches transcripts, including a Claude session steered by text it read (prompt injection). Such a session can write files, but its commands still need permission or run in Claude Code's sandbox;
- GitHub data written by other people (PR titles, branch names);
- downloaded repos or archives, including their `.git/` folder;
- other macOS accounts on the same Mac;
- the supply chain of a public repo.

**Out of scope:** malware already running as the user, root, and physical access.

**The main rule.** bletherneuk runs as the user, with no sandbox and no permission prompts. It must never turn "a chat wrote a file" or "a chat printed some text" into "code ran" or "the Mac did something".

### 20.2 Local web server

- **Address.** It binds to `127.0.0.1` only, on a random port. launchd restarts it if it stops. It never quietly moves to a different port.
- **Fresh secrets on every start:** a session secret, used as a cookie, and a separate write key.
- **Opening a page.**
  - The bridge or command line asks for a URL over the Unix socket. It gets `http://127.0.0.1:<port>/auth?code=<code>&next=<page>`, where the code works once and only for 30 seconds.
  - The service swaps the code for the cookie (`HttpOnly; SameSite=Strict; Path=/`) and sends a 303 redirect to a clean URL with no code in it. So browser history, session restore and synced history never hold a working secret.
  - For iTerm2 panes, the URL travels through the iTerm2 API.
  - For the default browser, the service writes a small redirect file (mode 0600) in its private `run/` folder and opens that file. The code therefore never appears in process arguments, which other accounts can see with `ps` (verified).
- **Every request** must:
  - have a `Host` header of exactly `127.0.0.1:<port>`, which stops DNS rebinding;
  - not have `Sec-Fetch-Site` set to `cross-site` or `same-site`. A page on another `127.0.0.1` port arrives as `same-site`, because cookies and the browser's same-site check ignore ports (RFC 6265 §8.5);
  - carry the session cookie, except for static script and style files, which hold no data.
- **Changes** (anything other than a read) are `POST` only, with a JSON body. They need an exact `Origin` match and the write key in an `X-Bletherneuk-Key` header. The write key reaches the page inside the authenticated HTML, and the page keeps it only in memory, never in a cookie or storage. A custom header also forces a CORS preflight, which the service never approves.
- **Live updates** use server-sent events with the same checks. There is no WebSocket. The browser applies no CORS checks to a WebSocket, and a WebSocket can't carry a custom header.
- **No CORS headers are ever sent.** Preflight requests are refused.
- **Response headers:**
  - `Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`
  - `X-Content-Type-Options: nosniff`
  - `Referrer-Policy: no-referrer`
  - `Cache-Control: no-store` on HTML and API responses
  - `X-Frame-Options: DENY`
- **The API only takes IDs** of chats, blocks, files and links. It never accepts a filesystem path, a URL to open or a command.
- **Outputs are never served** by the web server (section 20.5).

### 20.3 Rendering text in pages

- These fields are untrusted: chat titles (often written by AI), prompts, PR titles, branch names, file names, link titles, and every line of every diff, including the language names used for highlighting.
- They are always shown as text. Svelte escapes text by default.
- `{@html}` is banned except in the diff view. There it only takes Shiki's output, which escapes its input. Tests feed script payloads through code lines and language names to prove it.
- Markdown is never rendered. Docs open in your editor.
- A link becomes an `href` only if it's `http` or `https`. Clicking it asks the service to open it (section 20.5), so the toolbelt panel never navigates away.

### 20.4 Running git and gh

- **No `git` command ever runs inside a user's repo.** Verified with git 2.50.1:
  - `git status` and `git ls-files -o` run the repo's `core.fsmonitor` command;
  - `git add` runs `filter.*.clean` commands;
  - `git diff` between trees runs `textconv` programs;
  - `git update-ref` runs the `reference-transaction` hook.

  A steered chat could add any of these to `.git/config`, or work inside a downloaded folder that already has one. A snapshot would then run it outside Claude Code's sandbox and permission prompts.
- **libgit2 instead.** All reads, snapshots and diffs go through libgit2, which never runs hooks, fsmonitor, or external filter, textconv or diff programs. Its check that the repo is owned by the current user stays on (`safe.directory` is never overridden). Repos owned by another account are skipped.
- **Snapshots live in a shadow store** outside the repo (section 13). They can't be pushed by `git push --mirror` or a `refs/*` refspec, and they aren't shared with other worktrees.
- **Secret-looking files are never snapshotted.** These match `snapshots.skip_patterns`: `.env*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `id_rsa*`, `id_ed25519*`, `*credentials*.json`, `*service-account*.json`. The Changes page lists them as skipped. Snapshots also never include ignored files or files that scripts generate (section 13).
- **`gh` runs from a neutral folder** (the service's own data folder) with `-R <owner>/<repo>`, because `gh` runs `git` in its current folder.
  - `owner/repo` must match `^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$`, and PR numbers must be digits.
  - Arguments are passed as a list, never through a shell.

### 20.5 Opening files and links

- **Only known files.** Only files the index already knows, by ID, can be opened. The extension must be on the docs or outputs list.
- **Checks just before opening:**
  - it's a regular file and not a symlink (`lstat`);
  - it's owned by the user;
  - its real extension is still on the list.
- **Opened as a document, never launched.** The file opens with `open -b <bundle ID> -- <path>`, where the bundle ID is the default app for that extension, looked up through LaunchServices. If the file were swapped for an app after the checks, the editor or browser would be handed it as a document rather than macOS launching it.
- **Refused handlers.** It won't open a file if the default app for its extension is a terminal, a script runner or an installer: Terminal, iTerm2, Script Editor, Python Launcher, Installer or Automator.
- **Dangerous types are never on the list:** files with no extension, `.command`, `.terminal`, `.tool`, `.app`, `.pkg`, `.fileloc`, `.webloc`, `.jar`, `.py`, `.sh`. Files added by hand with other extensions are only revealed in Finder (`open -R`).
- **Copies from temporary folders:**
  - only paths named in this user's own transcripts are copied, because `/private/tmp` is writable by every account;
  - the file must be owned by the user and be a regular file, and is opened with `O_NOFOLLOW | O_NONBLOCK`;
  - size caps apply (section 10);
  - copies are stored with mode 0600.
- **Links:**
  - only `http` and `https`, checked when saved, shown and opened;
  - opened with `open -b <default browser bundle ID> -- <url>`, so `file:`, `smb:`, `javascript:` and app-specific schemes such as `vscode:` can never be triggered;
  - the host is always shown;
  - links added by `blether link` while the `CLAUDECODE` environment variable is set are marked "added by Claude";
  - links you added or edited by hand always win over automatic ones.

### 20.6 iTerm2 bridge and socket

- **Folders** reach iTerm2 only as the profile's `Working Directory`. They are never typed into a shell. Typed quoting isn't safe: fish treats `\'` as an escape inside single quotes, and control characters in folder names can edit the line before quoting is read. Before use, a folder must:
  - be absolute;
  - exist and be a directory;
  - be owned by the user;
  - contain no control or bidirectional-text characters.
- **Typed text** is only ever `claude --resume <uuid>`, `claude` or `claude --worktree <name>`.
  - The UUID is checked with `re.fullmatch`. `re.match` with `^…$` accepts a trailing newline (verified).
  - Worktree names must match `^[A-Za-z0-9._-]{1,64}$`.
- **Titles.** iTerm2 treats window and tab titles as interpolated strings, and `\(...)` in one can call functions such as `iterm2.get_string`, which shows a native text prompt. Backslashes are escaped before any title is set. A test checks that `\(iterm2.alert(...))` appears literally.
- **Bridge functions.** RPC functions the bridge registers take no free-form arguments.
- **The Unix socket:**
  - sits in a folder with mode 0700, and the socket itself has mode 0600 (macOS enforces socket permissions, verified);
  - the service checks every peer's user ID with `getpeereid()` and drops other users;
  - socket paths are limited to 104 bytes, so if the default path is too long the service uses `$TMPDIR/bletherneuk/` instead.

### 20.7 Hook

- **It prints nothing and always exits 0.** Claude Code adds `UserPromptSubmit` and `SessionStart` stdout to the chat's context. A `PreToolUse` reply can approve a tool. Exit code 2 blocks one.
- **It talks only over the Unix socket**, with a hard 50 ms limit, and reads at most 1 MB from stdin.
- **It is registered by quoted absolute path** (section 18).

### 20.8 Data at rest and logs

- **Permissions.** Folders under `~/Library/Application Support/bletherneuk/` are mode 0700. The database, copies, snapshots, backups and `settings.json` backups are mode 0600.
- **Logs never contain** secrets, codes, page URLs, prompts or hook payloads. The panel never writes them to the browser console, because iTerm2 copies toolbelt console output into its debug log.
- **Prompt text** is deleted from the database when Claude Code deletes the chat's transcript. The chat's title and metadata are kept.

### 20.9 Resource limits

- **Transcripts:**
  - only regular files are read, opened with `O_NOFOLLOW | O_NONBLOCK`, so a pipe or symlink named `*.jsonl` can't hang or flood the service;
  - reading resumes from the saved offset;
  - lines over 8 MB are skipped;
  - only the first 256 KB of each tool output is scanned.
- **Snapshots** have a budget: 500 MB of new data, 200,000 files and 60 seconds (`snapshots.max_*`). Over budget, the turn shows "snapshot missed".
- **Worktree status counts** have a 2-second budget each. Over budget, the count shows as "many".
- **Copies of temporary files** are capped at 200 MB each and 5 GB in total.

### 20.10 Supply chain and the public repo

- **Rust:** `Cargo.lock` is committed. `cargo-deny` checks advisories, licences and sources in CI.
- **Pages:** `package-lock.json` is committed and installs use `npm ci --ignore-scripts`. Dependencies are kept to a minimum. Install-time scripts are how the 2025 "Shai-Hulud" npm worm spread, and any page dependency runs with access to the session.
- **Bridge:** it uses no packages beyond `iterm2`, which ships with iTerm2's Python runtime. Test dependencies are pinned with hashes.
- **GitHub Actions:**
  - actions are pinned to commit SHAs (after the 2025 `tj-actions` compromise);
  - `permissions: contents: read`;
  - no `pull_request_target`, and no secrets in jobs that run pull-request code;
  - Dependabot watches cargo, npm and actions.
- **Secrets and data:** `gitleaks` runs before each commit and in CI. Test fixtures are written by hand. Real transcripts can contain an employer's data that `gitleaks` would not flag.

### 20.11 Security tests

These run in CI (section 21):

- **Web server:**
  - requests with no cookie, a wrong `Host`, or `Sec-Fetch-Site: same-site` or `cross-site` are refused;
  - a valid cookie sent from another `127.0.0.1` port is refused;
  - a `POST` without the write key, or with a wrong `Origin`, is refused;
  - a preflight request gets no CORS headers;
  - an auth code can't be used twice or after 30 seconds;
  - a redirect after auth never carries the code.
- **Pages:** script payloads in every rendered field, including diff code lines and language names, render as text.
- **Opening:**
  - links with `javascript:`, `file:`, `smb:` or `vscode:` are refused at every step;
  - a symlink named `report.html`, a file not owned by the user, and a disallowed extension are all refused;
  - the handler check refuses Terminal.
- **Git:** a repo whose config sets `core.fsmonitor`, a clean filter, `textconv` and a `reference-transaction` hook, all pointing at a script that writes a marker file. After a snapshot and a diff, the marker file must not exist.
- **Bridge:**
  - a folder containing control characters is refused;
  - a UUID with a trailing newline is refused;
  - a title containing `\(` is escaped.
- **Hook:** stdout is empty and the exit code is 0 in every case, including when the service is down.
- **Install:** the `settings.json` backup has mode 0600, the hook path is absolute and quoted, and other hooks are untouched.

## 21. Testing

Tests pass explicit values. They never rely on defaults from the config.

- **Rust unit tests**, using made-up transcripts in `tests/fixtures/transcripts/` that cover every line type used plus malformed and oversized lines:
  - reading transcripts, including resuming from an offset and handling a truncated file;
  - each grouping clue, which one wins when several match, and block joining and merging;
  - classifying docs and outputs, and finding paths in commands and output;
  - recognising links;
  - reading config and theme files;
  - the hook status state machine, including background work;
  - keep-awake requests: starting, the grace period, the 12-hour cap, block-level requests, and restoring after a restart. These use a fake clock and a fake `caffeinate` launcher.
- **Integration tests:**
  - snapshots, diff ranges and pruning, in temporary git repos and worktrees, covering:
    - what a snapshot includes: tracked files, chat-written new files, and new files left out for their extension or size;
    - a chat-written file staying in later snapshots;
    - large tracked files keeping their previous content;
    - the per-repo store limit deleting reviewed snapshots first while the timeline keeps its counts;
  - worktree discovery, branch switches, detached `HEAD`, removed worktrees, uncommitted counts, and commits ahead and behind, in temporary repos;
  - a chat started from a block in a new worktree being assigned to that block;
  - PR status, against a fake `gh` placed first on `PATH` that returns canned JSON;
  - the security tests in section 20.11;
  - `install` and `uninstall` against a temporary home folder, including a `settings.json` with other hooks in it.
- **Bridge tests** with `pytest` and a fake iTerm2 connection. They check that each request makes the right iTerm2 calls, that folders only travel through profile settings, that chat IDs that aren't UUIDs are rejected, and the fallback path.
- **Page tests** with Vitest for the diff view and timeline logic. Styling is not tested.
- **Manual check:** `blether doctor --live` on a real machine.
- **CI:** GitHub Actions on a macOS runner runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo deny check`, `pytest`, the Vitest suite and a `gitleaks` scan.

## 22. Repository layout

```
bletherneuk/
├── README.md              # what it is, requirements (section 5), install, configuration
├── LICENSE                # MIT
├── Cargo.toml
├── Makefile
├── src/
│   ├── main.rs            # command line
│   ├── ingest/            # transcript reading
│   ├── group/             # grouping clues and blocks
│   ├── worktrees/         # worktree discovery and status
│   ├── awake/             # keep-awake requests and caffeinate
│   ├── files/             # docs, outputs, copies
│   ├── links/             # link recognition, gh polling
│   ├── status/            # hook events, status state machine
│   ├── snapshots/         # snapshot store, diffs
│   ├── server/            # HTTP, server-sent events, page serving
│   ├── socket/            # Unix socket protocol
│   ├── store/             # SQLite, migrations, backups
│   └── install/           # install, uninstall, doctor
├── web/                   # Svelte pages
├── bridge/
│   ├── bletherneuk_bridge.py
│   └── tests/
├── tests/
│   ├── fixtures/          # made-up transcripts only
│   └── *.rs               # integration tests
├── docs/designs/
├── .github/workflows/ci.yml
└── .pre-commit-config.yaml   # gitleaks
```

## 23. Keeping the public repo clean

- No personal or company data anywhere in the repo: no real ticket keys, PR titles, branch names, sites, emails or transcripts. Examples use `ABC-123`, `#42` and `example.atlassian.net`.
- Test fixtures are written by hand and never copied from real transcripts.
- Personal settings live in `~/.config/bletherneuk/`. Data lives in `~/Library/Application Support/bletherneuk/`. Neither is ever inside the repo.
- `gitleaks` runs as a pre-commit hook and in CI.
- Commits use the author's GitHub no-reply email.

## 24. Prior art

- [weaver](https://github.com/robbowland/weaver) runs several coding agents side by side in tmux, with a board of every thread and a per-turn diff view in Neovim. The per-turn snapshot idea comes from there.
- [iTerm2's Claude Code integration](https://iterm2.com/claude-code-integration.html) shows each session's status in the toolbelt through a Claude Code hook and iTerm2's Python API.
