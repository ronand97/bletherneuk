//! Grouping chats into work blocks (design §9): placing chats, creating, joining and
//! merging blocks, block titles, automatic links and archiving.

pub mod clues;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::config::Config;
use crate::worktrees::{self, Locator};
pub use clues::{Facts, Key, Placement, WroteIn};

const DAY_MS: i64 = 86_400_000;

/// Makes sure a repo has a `repos` row, reading its GitHub remote and default branch.
pub fn ensure_repo(conn: &Connection, root: &Path) -> rusqlite::Result<()> {
    let key = root.to_string_lossy();
    let exists: bool = conn
        .query_row("SELECT 1 FROM repos WHERE root = ?1", [&key], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    if !exists {
        refresh_repo(conn, root)?;
    }
    Ok(())
}

/// Re-reads a repo's GitHub remote and default branch.
pub fn refresh_repo(conn: &Connection, root: &Path) -> rusqlite::Result<()> {
    let info = worktrees::repo_info(root).unwrap_or_default();
    conn.execute(
        "INSERT INTO repos(root, github, default_branch) VALUES (?1, ?2, ?3)
         ON CONFLICT(root) DO UPDATE SET github = excluded.github,
                                         default_branch = excluded.default_branch",
        params![
            root.to_string_lossy(),
            info.github.map(|g| g.to_ascii_lowercase()),
            info.default_branch
        ],
    )?;
    Ok(())
}

/// Works out which repo and worktrees a chat started in, is in now, and wrote into.
pub fn refresh_location(
    conn: &Connection,
    locator: &mut Locator,
    chat_id: &str,
) -> rusqlite::Result<()> {
    let (start, last): (Option<String>, Option<String>) = conn.query_row(
        "SELECT start_cwd, last_cwd FROM chats WHERE id = ?1",
        [chat_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let start_loc = start.as_deref().and_then(|p| locator.locate(Path::new(p)));
    let last_loc = last.as_deref().and_then(|p| locator.locate(Path::new(p)));
    if let Some(l) = &start_loc {
        ensure_repo(conn, &l.repo_root)?;
    }
    conn.execute(
        "UPDATE chats SET repo_root = ?2, start_worktree = ?3, worktree_path = ?4 WHERE id = ?1",
        params![
            chat_id,
            start_loc
                .as_ref()
                .map(|l| l.repo_root.to_string_lossy().into_owned()),
            start_loc
                .as_ref()
                .map(|l| l.worktree.to_string_lossy().into_owned()),
            last_loc
                .as_ref()
                .map(|l| l.worktree.to_string_lossy().into_owned()),
        ],
    )?;

    let writes: Vec<(String, i64)> = {
        let mut stmt = conn.prepare("SELECT path, count FROM chat_writes WHERE chat_id = ?1")?;
        stmt.query_map([chat_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut per_worktree: BTreeMap<String, (String, i64)> = BTreeMap::new();
    for (path, count) in writes {
        if let Some(loc) = locator.locate(Path::new(&path)) {
            let e = per_worktree
                .entry(loc.worktree.to_string_lossy().into_owned())
                .or_insert((loc.repo_root.to_string_lossy().into_owned(), 0));
            e.1 += count;
        }
    }
    for (wt, (root, count)) in per_worktree {
        ensure_repo(conn, Path::new(&root))?;
        let branch =
            worktrees::head_of(Path::new(&wt)).and_then(|h| h.branch().map(str::to_string));
        // The branch is recorded once, when the chat is first seen writing there.
        conn.execute(
            "INSERT INTO chat_worktrees(chat_id, worktree, repo_root, branch, writes)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chat_id, worktree) DO UPDATE SET writes = excluded.writes",
            params![chat_id, wt, root, branch, count],
        )?;
    }
    Ok(())
}

pub fn facts(conn: &Connection, chat_id: &str) -> rusqlite::Result<Facts> {
    let mut f = conn.query_row(
        "SELECT c.title, c.first_prompt, c.recorded_branch, c.repo_root, r.github,
                r.default_branch, c.start_worktree
         FROM chats c LEFT JOIN repos r ON r.root = c.repo_root
         WHERE c.id = ?1",
        [chat_id],
        |r| {
            Ok(Facts {
                title: r.get(0)?,
                first_prompt: r.get(1)?,
                recorded_branch: r.get(2)?,
                repo_root: r.get(3)?,
                github: r.get(4)?,
                default_branch: r.get(5)?,
                start_worktree: r.get(6)?,
                wrote_in: Vec::new(),
            })
        },
    )?;
    let mut stmt = conn.prepare(
        "SELECT w.worktree, w.repo_root, w.branch, r.default_branch, w.writes
         FROM chat_worktrees w LEFT JOIN repos r ON r.root = w.repo_root
         WHERE w.chat_id = ?1",
    )?;
    f.wrote_in = stmt
        .query_map([chat_id], |r| {
            Ok(WroteIn {
                worktree: r.get(0)?,
                repo_root: r.get(1)?,
                branch: r.get(2)?,
                default_branch: r.get(3)?,
                writes: r.get::<_, i64>(4)? as u64,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(f)
}

fn manual_placement(
    conn: &Connection,
    chat_id: &str,
) -> rusqlite::Result<Option<(Option<i64>, String)>> {
    conn.query_row(
        "SELECT block_id, kind FROM overrides
         WHERE chat_id = ?1 AND kind IN ('chat_move', 'chat_started_from')",
        [chat_id],
        |r| {
            let kind: String = r.get(1)?;
            let reason = if kind == "chat_move" {
                "moved by you"
            } else {
                "started from this block"
            };
            Ok((r.get(0)?, reason.to_string()))
        },
    )
    .optional()
}

/// Places one chat by the first clue that matches. Returns the blocks whose contents
/// changed (the old one and the new one).
pub fn place_chat(
    conn: &Connection,
    config: &Config,
    chat_id: &str,
    now: i64,
) -> rusqlite::Result<BTreeSet<i64>> {
    let mut touched = BTreeSet::new();
    let old: Option<i64> =
        conn.query_row("SELECT block_id FROM chats WHERE id = ?1", [chat_id], |r| {
            r.get(0)
        })?;
    touched.extend(old);

    let (block, reason) = match manual_placement(conn, chat_id)? {
        Some((block, reason)) => (block, reason),
        None => {
            let f = facts(conn, chat_id)?;
            let p = clues::evaluate(&f, &config.tickets.projects);
            match p.key {
                Some(key) => {
                    let id = block_for_key(conn, &key, f.repo_root.as_deref(), now)?;
                    let id = link_keys(conn, id)?;
                    (Some(id), p.reason)
                }
                None => (None, p.reason),
            }
        }
    };
    conn.execute(
        "UPDATE chats SET block_id = ?2, group_reason = ?3 WHERE id = ?1",
        params![chat_id, block, reason],
    )?;
    touched.extend(block);
    for b in touched.clone() {
        if block_exists(conn, b)? {
            refresh_block(conn, config, b, now)?;
        } else {
            touched.remove(&b);
        }
    }
    Ok(touched)
}

fn block_exists(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM blocks WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

pub fn block_of_key(conn: &Connection, key: &Key) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT block_id FROM block_keys WHERE key = ?1",
        [key.to_string()],
        |r| r.get(0),
    )
    .optional()
}

/// The block that owns `key`, creating one if the key is new.
pub fn block_for_key(
    conn: &Connection,
    key: &Key,
    repo_hint: Option<&str>,
    now: i64,
) -> rusqlite::Result<i64> {
    if let Some(id) = block_of_key(conn, key)? {
        return Ok(id);
    }
    let (title, source, repo) = match key {
        Key::Pr { repo, number } => (
            format!("{repo}#{number}"),
            "pr",
            repo_hint.map(str::to_string),
        ),
        Key::Ticket(t) => (t.clone(), "ticket", repo_hint.map(str::to_string)),
        Key::Branch { repo_root, branch } => (branch.clone(), "branch", Some(repo_root.clone())),
    };
    conn.execute(
        "INSERT INTO blocks(title, title_source, repo, created_at, last_active)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![title, source, repo, now],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO block_keys(key, block_id) VALUES (?1, ?2)",
        params![key.to_string(), id],
    )?;
    Ok(id)
}

/// Gives `key` to `block`. If another block owns it, the two blocks merge.
/// Returns the block that survives.
pub fn attach_key(conn: &Connection, key: &Key, block: i64) -> rusqlite::Result<i64> {
    match block_of_key(conn, key)? {
        None => {
            conn.execute(
                "INSERT INTO block_keys(key, block_id) VALUES (?1, ?2)",
                params![key.to_string(), block],
            )?;
            Ok(block)
        }
        Some(owner) if owner == block => Ok(block),
        Some(owner) => merge_blocks(conn, owner, block),
    }
}

/// Merges two blocks into the older one. Keys, chats, links, files, overrides, review
/// markers and window records all move across.
pub fn merge_blocks(conn: &Connection, a: i64, b: i64) -> rusqlite::Result<i64> {
    let (keep, gone) = (a.min(b), a.max(b));
    conn.execute(
        "UPDATE block_keys SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE chats SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    // A URL already linked to the surviving block stays as it is.
    conn.execute(
        "UPDATE OR IGNORE links SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE OR IGNORE files SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    // Keep the surviving block's own name if both were renamed.
    conn.execute(
        "DELETE FROM overrides WHERE kind = 'block_rename' AND block_id = ?2
         AND EXISTS (SELECT 1 FROM overrides WHERE kind = 'block_rename' AND block_id = ?1)",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE overrides SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE review_markers SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE bridge_windows SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE keep_awake SET block_id = ?1 WHERE block_id = ?2",
        params![keep, gone],
    )?;
    conn.execute(
        "UPDATE blocks SET last_active = MAX(last_active,
            (SELECT last_active FROM blocks WHERE id = ?2)),
            archived = archived AND (SELECT archived FROM blocks WHERE id = ?2)
         WHERE id = ?1",
        params![keep, gone],
    )?;
    conn.execute("DELETE FROM blocks WHERE id = ?1", [gone])?;
    Ok(keep)
}

fn block_keys(conn: &Connection, block: i64) -> rusqlite::Result<Vec<Key>> {
    let mut stmt = conn.prepare("SELECT key FROM block_keys WHERE block_id = ?1 ORDER BY key")?;
    let keys = stmt
        .query_map([block], |r| r.get::<_, String>(0))?
        .filter_map(|k| k.ok().and_then(|k| Key::parse(&k)))
        .collect();
    Ok(keys)
}

/// Joins keys that belong together (design §9): a PR's head branch joins its `pr` and
/// `branch` keys, and a ticket key in a PR's title or head branch joins too. Repeats
/// until nothing changes, since a join can merge blocks and bring in more keys.
pub fn link_keys(conn: &Connection, mut block: i64) -> rusqlite::Result<i64> {
    let prefixes: Vec<String> = ticket_prefixes(conn)?;
    for _ in 0..16 {
        let before = block_keys(conn, block)?;
        for key in &before {
            let related = related_keys(conn, key, &prefixes)?;
            for k in related {
                block = attach_key(conn, &k, block)?;
            }
        }
        if block_keys(conn, block)? == before {
            break;
        }
    }
    Ok(block)
}

/// Ticket prefixes are config, but `link_keys` runs deep inside other calls; the
/// service stores the current list in `meta` whenever config loads.
fn ticket_prefixes(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'tickets.projects'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

pub fn store_ticket_prefixes(conn: &Connection, config: &Config) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('tickets.projects', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [serde_json::to_string(&config.tickets.projects).unwrap_or_default()],
    )?;
    Ok(())
}

fn related_keys(conn: &Connection, key: &Key, prefixes: &[String]) -> rusqlite::Result<Vec<Key>> {
    let mut out = Vec::new();
    let from_pr = |title: Option<String>,
                   head: Option<String>,
                   github: &str,
                   out: &mut Vec<Key>|
     -> rusqlite::Result<()> {
        if let Some(head) = &head {
            let mut stmt = conn.prepare("SELECT root FROM repos WHERE github = ?1")?;
            let roots: Vec<String> = stmt
                .query_map([github], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for root in roots {
                out.push(Key::Branch {
                    repo_root: root,
                    branch: head.clone(),
                });
            }
        }
        let t = title
            .as_deref()
            .and_then(|t| clues::find_ticket(t, prefixes, false))
            .or_else(|| {
                head.as_deref()
                    .and_then(|h| clues::find_ticket(h, prefixes, true))
            });
        out.extend(t.map(Key::Ticket));
        Ok(())
    };
    match key {
        Key::Pr { repo, number } => {
            let row: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT title, head_branch FROM prs WHERE github = ?1 AND number = ?2",
                    params![repo, *number as i64],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((title, head)) = row {
                from_pr(title, head, repo, &mut out)?;
            }
        }
        Key::Branch { repo_root, branch } => {
            // The most recent PR whose head is this branch, in this repo's GitHub repo.
            let row: Option<(String, i64, Option<String>)> = conn
                .query_row(
                    "SELECT p.github, p.number, p.title FROM prs p
                     JOIN repos r ON r.github = p.github
                     WHERE r.root = ?1 AND p.head_branch = ?2
                     ORDER BY p.number DESC LIMIT 1",
                    params![repo_root, branch],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((github, number, title)) = row {
                out.push(Key::pr(&github, number as u64));
                from_pr(title, Some(branch.clone()), &github, &mut out)?;
            }
        }
        Key::Ticket(_) => {}
    }
    out.retain(|k| k != key);
    Ok(out)
}

/// Recomputes a block's title, repo, last activity, automatic links and archive flag.
pub fn refresh_block(
    conn: &Connection,
    config: &Config,
    block: i64,
    now: i64,
) -> rusqlite::Result<()> {
    let keys = block_keys(conn, block)?;
    let rename: Option<String> = conn
        .query_row(
            "SELECT value FROM overrides WHERE kind = 'block_rename' AND block_id = ?1",
            [block],
            |r| r.get(0),
        )
        .optional()?;

    struct PrRow {
        number: i64,
        title: Option<String>,
        state: Option<String>,
        is_draft: bool,
        review: Option<String>,
        checks: Option<String>,
        url: Option<String>,
        repo: String,
    }
    let mut prs = Vec::new();
    for k in &keys {
        if let Key::Pr { repo, number } = k {
            let row = conn
                .query_row(
                    "SELECT title, state, is_draft, review_decision, checks, url
                     FROM prs WHERE github = ?1 AND number = ?2",
                    params![repo, *number as i64],
                    |r| {
                        Ok(PrRow {
                            number: *number as i64,
                            title: r.get(0)?,
                            state: r.get(1)?,
                            is_draft: r.get(2)?,
                            review: r.get(3)?,
                            checks: r.get(4)?,
                            url: r.get(5)?,
                            repo: repo.clone(),
                        })
                    },
                )
                .optional()?;
            prs.push(row.unwrap_or(PrRow {
                number: *number as i64,
                title: None,
                state: None,
                is_draft: false,
                review: None,
                checks: None,
                url: None,
                repo: repo.clone(),
            }));
        }
    }
    prs.sort_by_key(|p| -p.number);
    let tickets: Vec<&String> = keys
        .iter()
        .filter_map(|k| match k {
            Key::Ticket(t) => Some(t),
            _ => None,
        })
        .collect();
    let branches: Vec<&Key> = keys
        .iter()
        .filter(|k| matches!(k, Key::Branch { .. }))
        .collect();

    let (title, source) = if let Some(r) = rename {
        (r, "rename")
    } else if let Some(t) = prs.iter().find_map(|p| p.title.clone()) {
        (t, "pr")
    } else if let Some(t) = tickets.first() {
        (t.to_string(), "ticket")
    } else if let Some(Key::Branch { branch, .. }) = branches.first() {
        (branch.clone(), "branch")
    } else if let Some(p) = prs.first() {
        (format!("{}#{}", p.repo, p.number), "pr")
    } else {
        let current: String =
            conn.query_row("SELECT title FROM blocks WHERE id = ?1", [block], |r| {
                r.get(0)
            })?;
        (current, "branch")
    };

    let repo: Option<String> = branches
        .iter()
        .find_map(|k| match k {
            Key::Branch { repo_root, .. } => Some(repo_root.clone()),
            _ => None,
        })
        .map(Ok)
        .or_else(|| {
            conn.query_row(
                "SELECT repo_root FROM chats WHERE block_id = ?1 AND repo_root IS NOT NULL
                 GROUP BY repo_root ORDER BY COUNT(*) DESC LIMIT 1",
                [block],
                |r| r.get(0),
            )
            .optional()
            .transpose()
        })
        .transpose()?;

    let last_chat: Option<i64> = conn.query_row(
        "SELECT MAX(last_activity) FROM chats WHERE block_id = ?1",
        [block],
        |r| r.get(0),
    )?;

    // Archived when every PR is merged or closed and the chats have been idle long enough.
    let all_done = !prs.is_empty()
        && prs
            .iter()
            .all(|p| matches!(p.state.as_deref(), Some("MERGED") | Some("CLOSED")));
    let idle_since = config.archive.idle_days as i64 * DAY_MS;

    conn.execute(
        "UPDATE blocks SET title = ?2, title_source = ?3, repo = COALESCE(?4, repo),
                last_active = COALESCE(?5, created_at),
                archived = ?6 AND COALESCE(?5, created_at) < ?7
         WHERE id = ?1",
        params![
            block,
            title,
            source,
            repo,
            last_chat,
            all_done,
            now - idle_since
        ],
    )?;

    // Automatic links.
    for p in &prs {
        let url = p
            .url
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{}/pull/{}", p.repo, p.number));
        let title = p
            .title
            .clone()
            .unwrap_or_else(|| format!("{}#{}", p.repo, p.number));
        let status = p.state.as_ref().map(|state| {
            json!({
                "state": state,
                "draft": p.is_draft,
                "checks": p.checks,
                "review": p.review,
            })
            .to_string()
        });
        upsert_auto_link(
            conn,
            block,
            "github_pr",
            &url,
            &title,
            status.as_deref(),
            now,
        )?;
    }
    if let Some(template) = &config.tickets.url_template {
        for t in &tickets {
            if let Some(url) = crate::links::ticket_url(template, t) {
                upsert_auto_link(conn, block, "jira", &url, t, None, now)?;
            }
        }
    }
    Ok(())
}

/// Adds or refreshes an automatic link. A link you added for the same URL keeps its
/// title; only the PR status is refreshed on it.
fn upsert_auto_link(
    conn: &Connection,
    block: i64,
    kind: &str,
    url: &str,
    title: &str,
    status: Option<&str>,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO links(block_id, kind, url, title, status, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'auto', ?6)
         ON CONFLICT(block_id, url) DO UPDATE SET
            status = COALESCE(excluded.status, status),
            title = CASE WHEN source = 'auto' THEN excluded.title ELSE title END",
        params![block, kind, url, title, status, now],
    )?;
    Ok(())
}

/// Stores PRs fetched from `gh` for one GitHub repo, then joins their keys into the
/// blocks they belong to. Never creates a block. Returns the blocks that changed.
pub fn apply_prs(
    conn: &Connection,
    config: &Config,
    github: &str,
    prs: &[crate::links::gh::Pr],
    now: i64,
) -> rusqlite::Result<BTreeSet<i64>> {
    let github = github.to_ascii_lowercase();
    for p in prs {
        conn.execute(
            "INSERT INTO prs(github, number, title, head_branch, state, is_draft, review_decision,
                             checks, url, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(github, number) DO UPDATE SET
                title = excluded.title, head_branch = excluded.head_branch, state = excluded.state,
                is_draft = excluded.is_draft, review_decision = excluded.review_decision,
                checks = excluded.checks, url = excluded.url, updated_at = excluded.updated_at",
            params![
                github,
                p.number as i64,
                p.title,
                p.head_branch,
                p.state,
                p.is_draft,
                p.review_decision,
                p.checks.as_str(),
                crate::links::validate(&p.url).ok().map(|u| u.to_string()),
                now
            ],
        )?;
    }
    let mut touched = BTreeSet::new();
    for p in prs {
        let pr_key = Key::pr(&github, p.number);
        let mut owner = block_of_key(conn, &pr_key)?;
        if owner.is_none() {
            // A block already on the PR's head branch picks the PR up.
            for k in related_keys(conn, &pr_key, &ticket_prefixes(conn)?)? {
                if let Key::Branch { .. } = k
                    && let Some(b) = block_of_key(conn, &k)?
                {
                    owner = Some(attach_key(conn, &pr_key, b)?);
                    break;
                }
            }
        }
        if let Some(b) = owner {
            touched.insert(link_keys(conn, b)?);
        }
    }
    touched.retain(|b| block_exists(conn, *b).unwrap_or(false));
    for b in &touched {
        refresh_block(conn, config, *b, now)?;
    }
    Ok(touched)
}

/// Moves a chat by hand. `None` puts it in Unsorted. A manual move always wins and is
/// never undone by later clues.
pub fn move_chat(
    conn: &Connection,
    config: &Config,
    chat_id: &str,
    target: Option<i64>,
    now: i64,
) -> rusqlite::Result<BTreeSet<i64>> {
    record_placement(conn, chat_id, target, "chat_move", now)?;
    place_chat(conn, config, chat_id, now)
}

/// A chat started from a block's panel belongs to that block, like a manual move.
pub fn assign_started_from(
    conn: &Connection,
    config: &Config,
    chat_id: &str,
    block: i64,
    now: i64,
) -> rusqlite::Result<BTreeSet<i64>> {
    let existing = manual_placement(conn, chat_id)?;
    if existing.is_none() {
        record_placement(conn, chat_id, Some(block), "chat_started_from", now)?;
    }
    place_chat(conn, config, chat_id, now)
}

fn record_placement(
    conn: &Connection,
    chat_id: &str,
    target: Option<i64>,
    kind: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM overrides WHERE chat_id = ?1 AND kind IN ('chat_move', 'chat_started_from')",
        [chat_id],
    )?;
    conn.execute(
        "INSERT INTO overrides(kind, chat_id, block_id, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![kind, chat_id, target, now],
    )?;
    Ok(())
}

/// Renames a block. `None` or an empty name goes back to the automatic title.
pub fn rename_block(
    conn: &Connection,
    config: &Config,
    block: i64,
    title: Option<&str>,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM overrides WHERE kind = 'block_rename' AND block_id = ?1",
        [block],
    )?;
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        conn.execute(
            "INSERT INTO overrides(kind, block_id, value, created_at) VALUES ('block_rename', ?1, ?2, ?3)",
            params![block, crate::util::truncate_chars(t, 200), now],
        )?;
    }
    refresh_block(conn, config, block, now)
}

/// Re-checks every block's archive flag.
pub fn archive_sweep(conn: &Connection, config: &Config, now: i64) -> rusqlite::Result<()> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM blocks")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    for id in ids {
        refresh_block(conn, config, id, now)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
