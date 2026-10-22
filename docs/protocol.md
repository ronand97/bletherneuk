# bletherneuk protocols

How the parts of bletherneuk talk to each other. The design ([`designs/2026-10-22-bletherneuk-design.md`](designs/2026-10-22-bletherneuk-design.md)) explains why; this page is the exact contract.

- [1. Unix socket](#1-unix-socket): hook, command line and iTerm2 bridge ↔ service.
- [2. HTTP](#2-http): pages ↔ service.
- [3. Shared shapes](#3-shared-shapes).

All text is UTF-8. All times are Unix milliseconds. Every field marked "or null" may also be missing.

## 1. Unix socket

Path: `~/Library/Application Support/bletherneuk/run/blether.sock`, or `$TMPDIR/bletherneuk/blether.sock` when that path is too long for a socket. The folder is mode 0700 and the socket 0600. The service drops any peer whose user ID differs from its own.

Messages are newline-delimited JSON: one object per line, at most 1 MB per line. Every message has a `type`.

### 1.1 One-shot requests

The client sends one line and reads one reply line, then closes. Replies are `{"ok": true, ...}` or `{"ok": false, "error": "<text>"}`.

| Request | Reply |
|---|---|
| `{"type": "hook", "event": Event, "iterm_session": "<UUID>" or null, "time": ms}` | none (the hook doesn't wait) |
| `{"type": "page_url", "page": "board" \| "panel" \| "changes", "block": id or null}` | `{"ok": true, "url": "http://127.0.0.1:<port>/auth?code=<code>&next=<path>"}` |
| `{"type": "open_page", "page": "board" \| "changes", "block": id or null, "near_session": "<UUID>" or null}` | `{"ok": true, "via": "iterm2" \| "browser"}` |
| `{"type": "link", "url": "...", "title": "..." or null, "block": id or null, "iterm_session": "<UUID>" or null, "by_claude": bool}` | `{"ok": true, "block": id, "link": id}` |
| `{"type": "rescan"}` | `{"ok": true}` |
| `{"type": "doctor"}` | `{"ok": true, "report": {...}}` |

`Event` is the hook event reduced to the fields bletherneuk uses: `name`, `session_id`, and optionally `transcript_path`, `cwd`, `prompt` (at most 2,000 characters), `notification_type`, `tool_name`, `file_path`, `agent_id`, `background_tasks` (a count), `source`, `reason`.

### 1.2 The iTerm2 bridge

The bridge keeps one connection open. Either side may send at any time.

**Bridge → service, first line:**

```json
{"type": "bridge_hello", "bridge_version": "0.1.0", "iterm_version": "3.7.6",
 "colors": {"background": "#rrggbb", "foreground": "#rrggbb", "ansi": ["#rrggbb", "... 16 in all"]} or null,
 "sessions": [{"session": "<UUID>", "window": "<window id>", "tab": "<tab id>",
               "block": id or null, "chat": "<chat UUID>" or null}],
 "focus": {"session": "<UUID>" or null, "window": "<window id>" or null}}
```

`block` and `chat` come from the iTerm2 variables `user.bletherneuk_block` (on windows) and `user.bletherneuk_chat` (on sessions), so the mapping survives restarts.

**Bridge → service, events:**

| Event | Sent when |
|---|---|
| `{"type": "focus", "session": "<UUID>" or null, "window": "<window id>" or null}` | The active session or window changes. |
| `{"type": "session_opened", "session": "<UUID>", "window": "<window id>", "tab": "<tab id>", "block": id or null}` | A session appears. `block` is set for sessions the bridge created for a block. |
| `{"type": "session_closed", "session": "<UUID>"}` | A session ends. |
| `{"type": "window_closed", "window": "<window id>", "block": id or null, "chats": ["<chat UUID>", ...]}` | A window closes. `chats` lists the chats that were open in it. |
| `{"type": "colors", "colors": {...}}` | The current profile's colours change. |
| `{"type": "reply", "id": n, "ok": bool, "error": "..." or null, "session": "<UUID>" or null}` | Answer to a request. |
| `{"type": "panel_failed"}` | The toolbelt panel couldn't be registered. |

**Service → bridge, requests.** Each has a unique `id`. The bridge answers each with a `reply`. If a request fails because a browser session can't be created, the reply has `"ok": false, "error": "browser_unavailable"`, and the service opens the page in the default browser instead.

| Request | The bridge |
|---|---|
| `{"type": "request", "id": n, "op": "panel", "url": "..."}` | Registers (or re-registers) the toolbelt panel: name "bletherneuk", identifier `dev.bletherneuk.panel`, `reveal_if_already_registered` false. |
| `{"type": "request", "id": n, "op": "open_chat", "chat": "<UUID>", "session": "<UUID>" or null, "folder": "/abs/path", "block": id, "window_title": "...", "tab_title": "..."}` | If `session` is open, focuses it. Otherwise creates a tab in the block's window (creating the window if needed), with the default profile plus `"Custom Directory": "Yes"` and `"Working Directory": folder`, then types `claude --resume <UUID>` and a newline. Sets `user.bletherneuk_chat` on the session. Replies with the session ID. |
| `{"type": "request", "id": n, "op": "new_chat", "block": id, "folder": "/abs/path", "worktree_name": "name" or null, "window_title": "..."}` | Creates a tab in the block's window as above. Types `claude`, or `claude --worktree <name>` when `worktree_name` is set. Tags the session with the block. Replies with the session ID. |
| `{"type": "request", "id": n, "op": "open_block", "block": id, "window_title": "...", "chats": [{"chat": "<UUID>", "folder": "/abs/path", "tab_title": "..."}]}` | Focuses the block's window. If it isn't open, opens one with a tab per chat, each resumed as in `open_chat`. |
| `{"type": "request", "id": n, "op": "open_page", "url": "...", "kind": "changes" \| "board", "near_session": "<UUID>" or null}` | `changes`: splits `near_session` vertically with a browser session (`"Custom Command": "Browser"`, `"Initial URL": url`), or reuses the split it opened before. `board`: opens or reuses its own window. |
| `{"type": "request", "id": n, "op": "set_titles", "windows": [{"block": id, "title": "..."}], "sessions": [{"session": "<UUID>", "title": "..."}]}` | Names windows and tabs. Escapes backslashes first. |
| `{"type": "request", "id": n, "op": "ping"}` | Replies. |

The bridge refuses, without touching iTerm2:
- a `chat` that isn't a UUID (checked with `re.fullmatch`);
- a `folder` that isn't absolute, doesn't exist, isn't a directory, isn't owned by the user, or contains control or bidirectional-text characters;
- a `worktree_name` that doesn't match `^[A-Za-z0-9._-]{1,64}$`;
- a `url` that isn't `http://127.0.0.1:<port>/...`.

## 2. HTTP

The service listens on `127.0.0.1` at a random port.

### 2.1 Getting in

1. The bridge or command line asks for a page URL over the socket (`page_url` or `open_page`).
2. The URL is `/auth?code=<code>&next=<path>`. A code works once, for 30 seconds.
3. `/auth` sets the session cookie (`HttpOnly; SameSite=Strict; Path=/`) and answers `303` to `next`, which is never shown with the code.
4. A request to `/auth` that already carries a valid session cookie is redirected to `next` whatever its code. Every toolbelt panel shares one cookie store, so panels in later windows get in this way.
5. If a page or `/auth` is requested without a valid cookie, the service shows a "connecting" page. That page reloads itself every few seconds. At most once every 10 seconds, the service also sends the bridge a fresh `panel` URL.

`next` must be one of `/`, `/panel` or `/changes?block=<id>`.

### 2.2 Rules for every request

- `Host` must be exactly `127.0.0.1:<port>`.
- `Sec-Fetch-Site` must not be `cross-site` or `same-site`.
- The session cookie is required, except for `/assets/*` and `/auth`.
- Anything that changes state is a `POST` with a JSON body. It must have an exact `Origin: http://127.0.0.1:<port>` and the write key in `X-Bletherneuk-Key`.
- No CORS headers are ever sent. `OPTIONS` requests are refused.
- Responses carry `Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` and `X-Frame-Options: DENY`. HTML and API responses also carry `Cache-Control: no-store`.

### 2.3 Pages

`GET /`, `GET /panel` and `GET /changes?block=<id>` all return the same HTML page. The service adds two `<meta>` tags to its `<head>`:

```html
<meta name="bn-page" content="board|panel|changes">
<meta name="bn-key" content="<write key>">
```

The page reads both on start, removes the `bn-key` tag, and keeps the key only in memory.

Under the CSP, pages can't use inline scripts, inline `<style>` or `style="..."` attributes. Style changes made from script through the CSSOM (`element.style.setProperty`) are allowed. That's how the theme and Svelte's `style:` directive work.

### 2.4 Reading (GET, JSON)

**`GET /api/board?archived=0|1`**

```json
{
  "now": 0,
  "iterm_connected": true,
  "keep_awake": {"holding": true, "chats": 2},
  "needs_you": [ChatSummary, "...oldest first"],
  "blocks": [BlockCard, "...most recently active first"],
  "unsorted": [ChatSummary, "...most recently active first"],
  "focus": {"block": 3, "chat": "<UUID>"}
}
```

With `archived=1`, `blocks` holds only archived blocks and `needs_you` and `unsorted` are empty. Hidden chats appear nowhere on the board. They're only found through search.

**`GET /api/search?q=<text>`** returns `{"blocks": [BlockCard], "chats": [ChatSummary]}`. It matches block and chat titles case-insensitively, including archived blocks and hidden chats.

**`GET /api/blocks/<id>`**

```json
{
  "block": {"id": 3, "title": "...", "title_source": "rename|pr|ticket|branch", "repo": "/abs/path" or null,
            "repo_name": "shop" or null, "archived": false, "last_active": 0},
  "worktrees": [Worktree],
  "links": [Link],
  "chats": [ChatSummary],
  "docs": [FileItem],
  "outputs": [FileItem],
  "changes_since_review": {"worktree": 7, "files": 12} or null,
  "keep_awake": {"block": true},
  "new_chat_name": "retry-backoff"
}
```

`new_chat_name` is a suggested name for a new worktree, made from the block title.

**`GET /api/blocks/<id>/timeline`**

```json
{
  "turns": [{"id": 1, "chat": "<UUID>", "chat_title": "...", "prompt": "<first 200 characters>" or null,
             "started_at": 0, "ended_at": 0 or null, "worktree": 7 or null,
             "files_changed": 3 or null, "lines_added": 10 or null, "lines_removed": 2 or null,
             "parallel": false, "missed": "<reason>" or null, "snapshot_deleted": false}],
  "reviews": [{"id": 1, "worktree": 7, "created_at": 0}]
}
```

Both lists are newest first.

**`GET /api/blocks/<id>/diff?worktree=<id>&range=review|branch|turn&turn=<id>`**

```json
{
  "range": {"kind": "review|branch|turn", "from_label": "...", "to_label": "..."},
  "files": [FileDiff],
  "totals": {"files": 3, "additions": 10, "deletions": 2},
  "not_in_diff": [{"path": "~/x/.zshrc", "reason": "..."}],
  "parallel": false,
  "unavailable": "snapshot deleted to save space" or null
}
```

`worktree` defaults to the worktree with the most recent turn. `range` defaults to `review`. With no review yet, `review` falls back to `branch`.

**`GET /api/theme`** returns `{"vars": {"--background": "#24273a", ...}}`. Pages set each variable on `document.documentElement` with `style.setProperty`.

**`GET /api/events`** is a server-sent event stream. Events:

| Event | Data |
|---|---|
| `changed` | `{"blocks": [id, ...], "board": true}`: refetch what you show if it's affected. `board` means board-level lists changed. |
| `theme` | `{"vars": {...}}` |
| `focus` | `{"block": id or null, "chat": "<UUID>" or null}`: the focused iTerm2 tab's block and chat. |
| `bridge` | `{"connected": bool}` |

The stream sends a comment every 20 seconds to keep the connection open. On disconnect, pages show a "reconnecting" overlay and reconnect with backoff.

### 2.5 Actions (POST, JSON)

Every action returns `{"ok": true}` or `{"ok": false, "error": "<text>"}`. Actions that need iTerm2 return `{"ok": false, "error": "iTerm2 not connected"}` when the bridge is down.

| Action | Body | Does |
|---|---|---|
| `POST /api/chats/<id>/open` | `{}` | Focuses the chat's tab, or resumes it in the block's window. |
| `POST /api/chats/<id>/move` | `{"block": id or null}` | Moves the chat by hand. `null` is Unsorted. |
| `POST /api/chats/<id>/keep-awake` | `{"on": bool}` | Starts or cancels "keep awake until done". |
| `POST /api/blocks/<id>/open` | `{}` | Focuses or opens the block's window. |
| `POST /api/blocks/<id>/new-chat` | `{"where": "worktree" \| "main" \| "new_worktree", "worktree": id or null, "name": "..." or null}` | Starts a chat. `worktree` picks one for `"worktree"`. `name` is needed for `"new_worktree"`. |
| `POST /api/blocks/<id>/rename` | `{"title": "..." or null}` | Renames. `null` or empty goes back to the automatic title. |
| `POST /api/blocks/<id>/keep-awake` | `{"on": bool}` | Covers the chats working right now. |
| `POST /api/blocks/<id>/links` | `{"url": "...", "title": "..." or null}` | Adds a link. Only `http` and `https`. |
| `POST /api/blocks/<id>/add-file` | `{}` | Shows a native file picker, then adds the chosen file. |
| `POST /api/blocks/<id>/review` | `{"worktree": id}` | Marks reviewed. |
| `POST /api/blocks/<id>/changes` | `{}` | Opens the Changes page next to the focused chat. |
| `POST /api/board/open` | `{}` | Opens the board in its own window. |
| `POST /api/links/<id>/open` | `{}` | Opens in the default browser. |
| `POST /api/links/<id>/delete` | `{}` | Removes a link. |
| `POST /api/files/<id>/open` | `{}` | Opens with the default app (design §20.5). |
| `POST /api/files/<id>/reveal` | `{}` | Shows in Finder. |
| `POST /api/files/<id>/hide` | `{"hidden": bool}` | Hides or unhides. |

## 3. Shared shapes

**ChatSummary**

```json
{"id": "<UUID>", "title": "...", "status": "idle|working|needs_you|done|closed", "background": false,
 "block": 3 or null, "block_title": "..." or null, "last_activity": 0, "status_since": 0 or null,
 "worktree": "retry" or "main checkout" or null, "branch": "feat/retry" or null,
 "now_on": "feat/other" or null, "group_reason": "...", "keep_awake": false,
 "can_resume": true, "resume_note": "worktree removed" or null, "hidden": false, "open_in_iterm": true}
```

`branch` is the branch the chat worked on. `now_on` is set when its worktree is now on a different branch.

**BlockCard**

```json
{"id": 3, "title": "...", "repo_name": "shop" or null, "archived": false, "last_active": 0,
 "pr": PrBadge or null, "ticket": "ABC-123" or null, "worktrees": ["retry"],
 "chats": {"working": 1, "needs_you": 0, "done": 2, "idle": 0, "closed": 3},
 "docs": 2, "outputs": 5}
```

**PrBadge**: `{"number": 42, "url": "https://github.com/...", "state": "OPEN|MERGED|CLOSED" or null, "draft": false, "checks": "passing|failing|pending|none" or null}`. `state` is null when `gh` hasn't reported it ("unknown").

**Worktree**: `{"id": 7, "name": "retry", "path": "~/repos/shop/.claude/worktrees/retry", "is_main": false, "branch": "feat/retry" or null, "detached_at": "a1b2c3d" or null, "uncommitted": 3 or null, "uncommitted_many": false, "ahead": 12 or null, "behind": 0 or null, "removed": false}`. `path` is for display only.

**Link**: `{"id": 1, "kind": "github_pr|jira|notion|other", "url": "https://...", "host": "github.com", "title": "...", "status": {"state": "OPEN", "draft": false, "checks": "passing", "review": "APPROVED"} or null, "source": "auto|manual", "added_by_claude": false}`.

**FileItem**: `{"id": 9, "name": "report.html", "folder": "~/repos/shop/out", "kind": "doc|output", "found_by": "tool|command|output|manual", "size": 1234 or null, "modified": 0 or null, "missing": false, "copy": false, "openable": true, "hidden": false}`. `copy` means the original is gone and the saved copy opens instead. `openable` is false for files added by hand with an extension outside the lists; those can only be revealed in Finder.

**FileDiff**: `{"old_path": "..." or null, "new_path": "..." or null, "status": "added|deleted|modified|renamed|copied|typechange", "binary": false, "additions": 3, "deletions": 1, "collapsed": "lockfile|minified|large" or null, "truncated": false, "unavailable": false, "hunks": [{"header": "@@ -1,3 +1,4 @@", "lines": [{"kind": "+|-| ", "old": 1 or null, "new": 1 or null, "text": "..."}]}]}`.
