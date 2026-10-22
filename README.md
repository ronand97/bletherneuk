# bletherneuk

"Blether" is Scots for a chat, and a "neuk" is a nook or corner. bletherneuk is the corner where your Claude Code chats are kept in order.

bletherneuk is a macOS companion for people who run [Claude Code](https://code.claude.com) in [iTerm2](https://iterm2.com). It groups Claude Code chats into **work blocks**. A work block is one piece of work, usually tracked by a pull request, a Jira ticket or a Notion page. For each block it shows:

- the chats that belong to it, with live status, each one click from being resumed;
- its PR (with live status and checks), ticket and other links;
- the docs and outputs (HTML reports, CSVs, maps) its chats produced, including ones in temporary folders that macOS deletes;
- a **Changes** view of everything that changed in the code since you last looked, turn by turn.

It runs on top of iTerm2 rather than replacing it. The command is `blether`.

> **Status:** early. It's built from source, and the author's own Mac is the only one it has run on so far.

## How it fits together

- **`blether serve`** is a small Rust service that launchd keeps running. It reads Claude Code's transcripts, groups chats into blocks, polls `gh` for PR status and serves the pages on `127.0.0.1`.
- **`blether hook`** is registered as a Claude Code hook. It tells the service what each chat is doing, in a few milliseconds, and never prints anything.
- **The iTerm2 bridge** (`bletherneuk_bridge.py`) runs inside iTerm2. It adds a **bletherneuk** panel to the toolbelt, and opens windows, tabs and browser panes for the service.
- **The pages**: a home board in its own iTerm2 window, the toolbelt panel that follows the focused tab, and a Changes page that opens in a split pane next to the chat.

The design, with the reasons behind each choice, is in [`docs/designs/2026-10-22-bletherneuk-design.md`](docs/designs/2026-10-22-bletherneuk-design.md). The exact protocols are in [`docs/protocol.md`](docs/protocol.md).

## Requirements

- macOS on Apple silicon or Intel.
- iTerm2 3.7.6 or later, with **Settings → General → Magic → Enable Python API** switched on.
- Claude Code.
- The GitHub CLI (`gh`), logged in, for PR status. Optional: without it, PR badges say "unknown".
- To build: Rust (stable), Node.js 22 or later, and Python 3.10 or later for the bridge tests.

### Tested with

| Component | Version |
|---|---|
| macOS | 26.6.2 |
| iTerm2 | 3.7.6 |
| iTerm2 browser plugin | 1.0 |
| `iterm2` Python package | 2.30 |
| Claude Code | 2.1.307 |

Browser panes rely on an undocumented iTerm2 behaviour (design §5). If it stops working, or iTerm2 is older than 3.7.6, pages open in your default browser instead. `blether doctor` says which is in use.

## Install

```sh
make install
```

This builds the pages and the binary, copies `blether` to `~/.local/bin`, and runs `blether install`, which:

1. adds one `"<path>/blether" hook` entry per Claude Code hook event to `~/.claude/settings.json`, after saving a backup next to it (`settings.json.bletherneuk-backup-<time>`, mode 0600). Your other hooks are left as they are;
2. copies the bridge into iTerm2's `Scripts/AutoLaunch` folder;
3. writes and loads a launchd agent, `~/Library/LaunchAgents/dev.bletherneuk.serve.plist`;
4. writes starter `~/.config/bletherneuk/config.toml` and `theme.toml` files if you don't have them.

Run `blether install --dry-run` to see the changes without making them, and `blether doctor` afterwards to check everything.

### First run

- **iTerm2's Python runtime.** The first time the bridge starts, iTerm2 may offer to download its Python runtime. Accept it.
- **The panel.** Open the toolbelt (**View → Show Toolbelt**) and tick **bletherneuk** in **Toolbelt → bletherneuk** if it isn't shown.
- **A key for the board.** In **Settings → Keys → Key Bindings**, add a binding with the action **Invoke Script Function** and the function `bletherneuk_open_board()`.
- **Browser panes.** iTerm2 asks once whether its browser may open `127.0.0.1`. Allow it.

## Using it

- **Board:** `blether board`, or your key binding. Chats waiting on you are at the top. Click a block to open its window.
- **Panel:** follows the focused tab. It shows the block's worktrees, links, chats, docs and outputs, and has buttons for a new chat (in this worktree, the main checkout, or a new worktree), "keep awake until done", rename and add file.
- **Changes:** from the panel's "files changed since your last review" line. Pick a range (since your last review, one turn, or the whole branch) and mark it reviewed when you're done.
- **Links:** paste a URL into the panel's "+ link" box, or run `blether link <url> [--title "…"]` in a chat's tab. Claude can run it too; such links are marked "added by Claude".
- **Files:** the panel's "Add file" button, or `blether file <path>` in a chat's tab.
- **Wrong block?** Every chat says why it was placed where it is. Use its "Move to…" menu; manual moves always win.
- `blether rescan` re-reads every transcript. Everything you entered by hand is kept.

## Configuration

Every key is optional. Saved changes apply straight away.

`~/.config/bletherneuk/config.toml`:

```toml
[tickets]
projects = ["ABC"]                                          # ticket-key prefixes to recognise
url_template = "https://example.atlassian.net/browse/{key}"

[outputs]
extensions = ["html", "csv", "geojson", "parquet", "png", "svg", "xlsx", "pdf"]

[keep_awake]
grace_seconds = 120
max_hours = 12
```

`~/.config/bletherneuk/theme.toml`:

```toml
base = "soft-dark"        # or "soft-light"
match_iterm2 = true       # take background, text and the 16 ANSI colours from the current iTerm2 profile

[font]
ui = "Inter"
code = "JetBrains Mono"
size = 13

[colors]                  # anything set here wins
accent = "#8aadf4"
```

The full list of settings is in the design, sections 15 and 17.

## Security

bletherneuk runs as you, with no sandbox, so it is built never to turn "a chat wrote a file" or "a chat printed some text" into "code ran". In short:

- No `git` command ever runs inside your repos. Everything goes through libgit2, which never runs hooks, fsmonitor, or filter, textconv or diff programs. Snapshots live in a separate store and never enter your repo.
- The web server listens on `127.0.0.1` only. It checks `Host` and Fetch Metadata on every request, and needs a session cookie plus a per-start write key in page memory for every change. It sends a strict Content-Security-Policy and no CORS headers.
- Files open only by ID, as documents in their default app, and never with a terminal or script runner. Links open only if they are `http` or `https`.
- The Unix socket is private to your user, and the hook never prints anything.

Design section 20 has the details and the threat model.

## Uninstall

```sh
blether uninstall          # keeps your data
blether uninstall --purge  # also deletes ~/Library/Application Support/bletherneuk
```

`uninstall` removes only the hook entries, bridge and launchd agent that `install` added.

## Development

```sh
make test    # Rust, page and bridge tests
make lint    # rustfmt, clippy and svelte-check
make build   # pages, then the release binary with the pages built in
```

Secret scanning runs before each commit through [pre-commit](https://pre-commit.com) and [gitleaks](https://github.com/gitleaks/gitleaks): `brew install pre-commit gitleaks && pre-commit install`. CI runs the same scan, `cargo deny` and every test.

Test fixtures are written by hand. Never copy real transcripts into the repo.

## Licence

MIT. See [LICENSE](LICENSE).
