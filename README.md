# bletherneuk

"Blether" is Scots for a chat, and a "neuk" is a nook or corner. bletherneuk is the corner where your Claude Code chats are kept in order.

bletherneuk is a macOS companion for people who run [Claude Code](https://code.claude.com) in [iTerm2](https://iterm2.com). It groups Claude Code chats into **work blocks**. A work block is one piece of work, usually tracked by a pull request, a Jira ticket or a Notion page. For each block it shows:

- the chats that belong to it, with live status, each one click from being resumed;
- its PR (with live status and checks), ticket and other links;
- the docs and outputs (HTML reports, CSVs, maps) its chats produced;
- a **Changes** view of everything that changed in the code since you last looked.

It runs on top of iTerm2 rather than replacing it. The command is `blether`.

> **Status:** under construction. Nothing here is ready to use yet.

## Requirements

- macOS on Apple silicon or Intel.
- iTerm2 with **Settings → General → Magic → Enable Python API** switched on.
- Claude Code.
- The GitHub CLI (`gh`), logged in, for PR status. Optional.
- To build from source: Rust (stable), Node.js 22 or later, and Python 3.10 or later for the bridge tests.

### Tested with

| Component | Version |
|---|---|
| macOS | 26.6.2 |
| iTerm2 | 3.7.6 |
| iTerm2 browser plugin | 1.0 |
| `iterm2` Python package | 2.30 |
| Claude Code | 2.1.307 |

Browser panes rely on an undocumented iTerm2 behaviour. If it stops working, pages open in your default browser instead. `blether doctor` reports which one is in use.

## Install

From source:

```sh
make install
```

This builds the pages and the binary, copies `blether` to `~/.local/bin`, and runs `blether install`. See [`docs/designs/2026-10-22-bletherneuk-design.md`](docs/designs/2026-10-22-bletherneuk-design.md) section 18 for exactly what `blether install` changes, and `blether install --dry-run` to preview it.

## Configuration

Personal settings live in `~/.config/bletherneuk/config.toml` and `~/.config/bletherneuk/theme.toml`. Every key is optional. See the design, sections 15 and 17.

## Development

```sh
make test   # Rust, page and bridge tests
make lint   # rustfmt, clippy, svelte-check
```

Secret scanning runs before each commit through [pre-commit](https://pre-commit.com) and [gitleaks](https://github.com/gitleaks/gitleaks): `brew install pre-commit gitleaks && pre-commit install`.

Test fixtures are written by hand. Never copy real transcripts into the repo.

## Licence

MIT. See [LICENSE](LICENSE).
