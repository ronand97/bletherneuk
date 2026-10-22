//! The JSON the pages read (protocol §2.4 and §3).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Value, json};

use super::App;
use crate::group::Key;
use crate::snapshots::{Shadow, diff};

pub fn tilde(app: &App, p: &str) -> String {
    let home = app.paths.home.to_string_lossy();
    match p.strip_prefix(home.as_ref()) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => p.to_string(),
    }
}

fn fmt_time(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%-d %b %H:%M").to_string())
        .unwrap_or_default()
}

const CHAT_SELECT: &str = "
    SELECT c.id, c.title, c.status, c.background, c.block_id, b.title, c.last_activity,
           c.status_since, c.worktree_path, COALESCE(w.repo_root, c.repo_root), w.branch, w.removed,
           c.group_reason,
           c.hidden, c.iterm_session, c.transcript_gone, c.last_cwd,
           COALESCE((SELECT cw.branch FROM chat_worktrees cw
                     WHERE cw.chat_id = c.id AND cw.worktree != COALESCE(c.start_worktree, '')
                       AND cw.branch IS NOT NULL
                     ORDER BY cw.writes DESC LIMIT 1), c.recorded_branch) AS work_branch
    FROM chats c
    LEFT JOIN blocks b ON b.id = c.block_id
    LEFT JOIN worktrees w ON w.path = c.worktree_path";

struct ChatCtx {
    sessions: BTreeSet<String>,
    covered: BTreeSet<String>,
}

fn chat_ctx(app: &App) -> ChatCtx {
    ChatCtx {
        sessions: app
            .sessions
            .lock()
            .expect("sessions lock")
            .keys()
            .cloned()
            .collect(),
        covered: app.awake.lock().expect("awake lock").covered_chats(),
    }
}

fn chat_summary(r: &Row, ctx: &ChatCtx) -> rusqlite::Result<Value> {
    let id: String = r.get(0)?;
    let worktree_path: Option<String> = r.get(8)?;
    let repo_root: Option<String> = r.get(9)?;
    let now_branch: Option<String> = r.get(10)?;
    let removed: Option<bool> = r.get(11)?;
    let branch: Option<String> = r.get(17)?;
    let transcript_gone: bool = r.get(15)?;
    let last_cwd: Option<String> = r.get(16)?;
    let iterm: Option<String> = r.get(14)?;
    let worktree = match (&worktree_path, &repo_root) {
        (Some(w), Some(root)) if w == root => Some("main checkout".to_string()),
        (Some(w), _) => Path::new(w)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        _ => None,
    };
    let now_on = match (&now_branch, &branch) {
        (Some(n), Some(b)) if n != b => Some(n.clone()),
        _ => None,
    };
    let is_dir = |p: &Option<String>| p.as_deref().is_some_and(|p| Path::new(p).is_dir());
    // Matches actions::resume_folder: a chat whose worktree was removed resumes in the
    // main checkout.
    let (can_resume, resume_note) = if transcript_gone {
        (false, Some("transcript deleted"))
    } else if removed == Some(true) {
        if is_dir(&repo_root) {
            (true, Some("worktree removed; resumes in the main checkout"))
        } else {
            (false, Some("worktree removed"))
        }
    } else if is_dir(&last_cwd) {
        (true, None)
    } else {
        (false, Some("folder missing"))
    };
    Ok(json!({
        "id": id,
        "title": r.get::<_, Option<String>>(1)?.unwrap_or_else(|| "Untitled chat".into()),
        "status": r.get::<_, String>(2)?,
        "background": r.get::<_, bool>(3)?,
        "block": r.get::<_, Option<i64>>(4)?,
        "block_title": r.get::<_, Option<String>>(5)?,
        "last_activity": r.get::<_, Option<i64>>(6)?.unwrap_or(0),
        "status_since": r.get::<_, Option<i64>>(7)?,
        "worktree": worktree,
        "branch": branch,
        "now_on": now_on,
        "group_reason": r.get::<_, Option<String>>(12)?.unwrap_or_default(),
        "keep_awake": ctx.covered.contains(&id),
        "can_resume": can_resume,
        "resume_note": resume_note,
        "hidden": r.get::<_, bool>(13)?,
        "open_in_iterm": iterm.is_some_and(|s| ctx.sessions.contains(&s)),
    }))
}

fn chats_where(
    c: &Connection,
    ctx: &ChatCtx,
    clause: &str,
    p: &[&dyn rusqlite::ToSql],
) -> Vec<Value> {
    let sql = format!("{CHAT_SELECT} WHERE {clause}");
    let Ok(mut stmt) = c.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(p, |r| chat_summary(r, ctx))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn block_keys(c: &Connection, block: i64) -> Vec<Key> {
    let Ok(mut stmt) = c.prepare("SELECT key FROM block_keys WHERE block_id = ?1 ORDER BY key")
    else {
        return Vec::new();
    };
    stmt.query_map([block], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().filter_map(|k| Key::parse(&k)).collect())
        .unwrap_or_default()
}

fn pr_badge(c: &Connection, keys: &[Key]) -> Value {
    let best = keys
        .iter()
        .filter_map(|k| match k {
            Key::Pr { repo, number } => Some((repo, *number)),
            _ => None,
        })
        .max_by_key(|(_, n)| *n);
    let Some((repo, n)) = best else {
        return Value::Null;
    };
    let row: Option<(Option<String>, bool, Option<String>, Option<String>)> = c
        .query_row(
            "SELECT state, is_draft, checks, url FROM prs WHERE github = ?1 AND number = ?2",
            params![repo, n as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (state, draft, checks, url) = row.unwrap_or((None, false, None, None));
    json!({
        "number": n,
        "url": url.unwrap_or_else(|| format!("https://github.com/{repo}/pull/{n}")),
        "state": state,
        "draft": draft,
        "checks": checks,
    })
}

/// A block's worktrees: those on one of its branches, plus any its chats ran in.
/// `?1` is the block ID.
const BLOCK_WORKTREES: &str = "
    FROM worktrees w
    WHERE (EXISTS (SELECT 1 FROM block_keys k
                   WHERE k.block_id = ?1 AND k.key = 'branch:' || w.repo_root || '@' || w.branch)
           OR w.path IN (SELECT worktree_path FROM chats WHERE block_id = ?1 AND worktree_path IS NOT NULL))";
const BLOCK_WORKTREES_ORDER: &str = "ORDER BY w.path = w.repo_root, w.name";

fn block_worktrees(app: &App, c: &Connection, block: i64) -> Vec<Value> {
    let sql = format!(
        "SELECT w.rowid, w.path, w.name, w.branch, w.detached_at, w.uncommitted, w.ahead,
                w.behind, w.removed, w.repo_root {BLOCK_WORKTREES} {BLOCK_WORKTREES_ORDER}"
    );
    let Ok(mut stmt) = c.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map([block], |r| {
        let path: String = r.get(1)?;
        let root: String = r.get(9)?;
        let unc: Option<i64> = r.get(5)?;
        Ok(json!({
            "id": r.get::<_, i64>(0)?,
            "path": tilde(app, &path),
            "name": r.get::<_, String>(2)?,
            "is_main": path == root,
            "branch": r.get::<_, Option<String>>(3)?,
            "detached_at": r.get::<_, Option<String>>(4)?,
            "uncommitted": unc.filter(|n| *n >= 0),
            "uncommitted_many": unc == Some(-1),
            "ahead": r.get::<_, Option<i64>>(6)?,
            "behind": r.get::<_, Option<i64>>(7)?,
            "removed": r.get::<_, bool>(8)?,
        }))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

fn status_counts(c: &Connection, block: i64) -> Value {
    let mut counts: HashMap<String, i64> = ["working", "needs_you", "done", "idle", "closed"]
        .iter()
        .map(|s| (s.to_string(), 0))
        .collect();
    if let Ok(mut stmt) = c.prepare(
        "SELECT status, COUNT(*) FROM chats WHERE block_id = ?1 AND hidden = 0 GROUP BY status",
    ) && let Ok(rows) = stmt.query_map([block], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    }) {
        for (s, n) in rows.flatten() {
            counts.insert(s, n);
        }
    }
    json!(counts)
}

/// Docs and outputs of a block, newest first, one entry per path.
fn block_files(app: &App, c: &Connection, block: i64) -> (Vec<Value>, Vec<Value>) {
    let hidden: BTreeSet<String> = c
        .prepare("SELECT path FROM overrides WHERE kind = 'file_hide' AND block_id = ?1")
        .and_then(|mut s| {
            s.query_map([block], |r| r.get::<_, String>(0))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default();
    let config = app.config();
    let Ok(mut stmt) = c.prepare(
        "SELECT f.id, f.path, f.kind, f.found_by, f.size, f.mtime, f.copy_path
         FROM files f LEFT JOIN chats c ON c.id = f.chat_id
         WHERE c.block_id = ?1 OR f.block_id = ?1
         ORDER BY f.first_seen DESC, f.id DESC",
    ) else {
        return (Vec::new(), Vec::new());
    };
    let rows: Vec<(
        i64,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
    )> = stmt
        .query_map([block], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    let (mut docs, mut outputs) = (Vec::new(), Vec::new());
    for (id, path, kind, found_by, size, mtime, copy) in rows {
        if !seen.insert(path.clone()) {
            continue;
        }
        let p = Path::new(&path);
        let exists = std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file());
        let has_copy = copy.as_deref().is_some_and(|c| Path::new(c).is_file());
        let ext = crate::files::extension(p).unwrap_or_default();
        let item = json!({
            "id": id,
            "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "folder": p.parent().map(|d| tilde(app, &d.to_string_lossy())).unwrap_or_default(),
            "kind": kind,
            "found_by": found_by,
            "size": size,
            "modified": mtime,
            "missing": !exists && !has_copy,
            "copy": !exists && has_copy,
            "openable": crate::open::extension_allowed(&ext, &config),
            "hidden": hidden.contains(&path),
        });
        if kind == "doc" {
            docs.push(item);
        } else {
            outputs.push(item);
        }
    }
    (docs, outputs)
}

fn block_card(app: &App, c: &Connection, id: i64) -> Option<Value> {
    let (title, repo, archived, last_active): (String, Option<String>, bool, i64) = c
        .query_row(
            "SELECT title, repo, archived, last_active FROM blocks WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok()?;
    let keys = block_keys(c, id);
    let ticket = keys.iter().find_map(|k| match k {
        Key::Ticket(t) => Some(t.clone()),
        _ => None,
    });
    let worktrees: Vec<Value> = block_worktrees(app, c, id)
        .into_iter()
        .filter(|w| !w["is_main"].as_bool().unwrap_or(false))
        .map(|w| w["name"].clone())
        .collect();
    let (docs, outputs) = block_files(app, c, id);
    let visible = |v: &Vec<Value>| {
        v.iter()
            .filter(|f| !f["hidden"].as_bool().unwrap_or(false))
            .count()
    };
    Some(json!({
        "id": id,
        "title": title,
        "repo_name": repo.as_deref().and_then(|r| Path::new(r).file_name()).map(|n| n.to_string_lossy().into_owned()),
        "archived": archived,
        "last_active": last_active,
        "pr": pr_badge(c, &keys),
        "ticket": ticket,
        "worktrees": worktrees,
        "chats": status_counts(c, id),
        "docs": visible(&docs),
        "outputs": visible(&outputs),
    }))
}

pub fn board(app: &App, archived: bool) -> Value {
    let ctx = chat_ctx(app);
    let holding = app.awake.lock().expect("awake lock").is_holding();
    let covered = ctx.covered.len();
    let focus = app.focus.lock().expect("focus lock").clone();
    let db = app.db.lock().expect("db lock");
    let c = db.conn();
    let ids: Vec<i64> = c
        .prepare(
            "SELECT b.id FROM blocks b
             WHERE b.archived = ?1
               AND (EXISTS (SELECT 1 FROM chats c WHERE c.block_id = b.id AND c.hidden = 0)
                    OR EXISTS (SELECT 1 FROM links l WHERE l.block_id = b.id AND l.source = 'manual')
                    OR EXISTS (SELECT 1 FROM files f WHERE f.block_id = b.id))
             ORDER BY b.last_active DESC",
        )
        .and_then(|mut s| s.query_map([archived], |r| r.get(0)).map(|r| r.flatten().collect()))
        .unwrap_or_default();
    let blocks: Vec<Value> = ids
        .into_iter()
        .filter_map(|id| block_card(app, c, id))
        .collect();
    let (needs_you, unsorted) = if archived {
        (Vec::new(), Vec::new())
    } else {
        (
            chats_where(
                c,
                &ctx,
                "c.status = 'needs_you' AND c.hidden = 0 ORDER BY c.status_since ASC",
                &[],
            ),
            chats_where(
                c,
                &ctx,
                "c.block_id IS NULL AND c.hidden = 0 ORDER BY c.last_activity DESC LIMIT 200",
                &[],
            ),
        )
    };
    json!({
        "now": crate::util::now_ms(),
        "iterm_connected": app.bridge_connected(),
        "keep_awake": {"holding": holding, "chats": covered},
        "needs_you": needs_you,
        "blocks": blocks,
        "unsorted": unsorted,
        "focus": {"block": focus.block, "chat": focus.chat},
    })
}

pub fn search(app: &App, q: &str) -> Value {
    let q = q.trim();
    if q.is_empty() {
        return json!({"blocks": [], "chats": []});
    }
    let pattern = format!(
        "%{}%",
        q.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let ctx = chat_ctx(app);
    let db = app.db.lock().expect("db lock");
    let c = db.conn();
    let ids: Vec<i64> = c
        .prepare(
            "SELECT id FROM blocks WHERE title LIKE ?1 ESCAPE '\\'
             ORDER BY last_active DESC LIMIT 50",
        )
        .and_then(|mut s| {
            s.query_map([&pattern], |r| r.get(0))
                .map(|r| r.flatten().collect())
        })
        .unwrap_or_default();
    let blocks: Vec<Value> = ids
        .into_iter()
        .filter_map(|id| block_card(app, c, id))
        .collect();
    let chats = chats_where(
        c,
        &ctx,
        "c.title LIKE ?1 ESCAPE '\\' ORDER BY c.last_activity DESC LIMIT 100",
        &[&pattern],
    );
    json!({"blocks": blocks, "chats": chats})
}

/// The worktree a block's latest turn ran in, else its first worktree.
fn primary_worktree(c: &Connection, block: i64) -> Option<(i64, String)> {
    c.query_row(
        "SELECT w.rowid, w.path FROM turns t JOIN chats c ON c.id = t.chat_id
         JOIN worktrees w ON w.path = t.worktree_path
         WHERE c.block_id = ?1 ORDER BY t.started_at DESC LIMIT 1",
        [block],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .ok()
    .flatten()
    .or_else(|| {
        c.query_row(
            &format!("SELECT w.rowid, w.path {BLOCK_WORKTREES} {BLOCK_WORKTREES_ORDER} LIMIT 1"),
            [block],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    })
}

fn latest_review(c: &Connection, block: i64, worktree: &str) -> Option<(i64, i64)> {
    c.query_row(
        "SELECT snapshot_id, created_at FROM review_markers
         WHERE block_id = ?1 AND worktree_path = ?2 ORDER BY created_at DESC LIMIT 1",
        params![block, worktree],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .ok()
    .flatten()
}

fn latest_snapshot(c: &Connection, worktree: &str) -> Option<i64> {
    c.query_row(
        "SELECT id FROM snapshots WHERE worktree_path = ?1 AND tree_id IS NOT NULL
         ORDER BY id DESC LIMIT 1",
        [worktree],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// "N files changed since your last review", from recorded snapshots only.
fn changes_since_review(app: &App, block: i64) -> Value {
    let (wt, marker, latest) = {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        let Some((wid, wt)) = primary_worktree(c, block) else {
            return Value::Null;
        };
        let Some((marker, _)) = latest_review(c, block, &wt) else {
            return Value::Null;
        };
        let Some(latest) = latest_snapshot(c, &wt) else {
            return Value::Null;
        };
        ((wid, wt), marker, latest)
    };
    let (Some((root, a)), Some((_, b))) = (
        super::snapshots::tree_of(app, marker),
        super::snapshots::tree_of(app, latest),
    ) else {
        return Value::Null;
    };
    let Ok(shadow) = Shadow::open(&app.paths.snapshots(), Path::new(&root)) else {
        return Value::Null;
    };
    match diff::diff_trees(&shadow.repo, Some(a), b, true) {
        Ok(files) => json!({"worktree": wt.0, "files": files.len()}),
        Err(_) => Value::Null,
    }
}

fn link_json(r: &Row) -> rusqlite::Result<Value> {
    let url: String = r.get(2)?;
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let status: Option<String> = r.get(4)?;
    Ok(json!({
        "id": r.get::<_, i64>(0)?,
        "kind": r.get::<_, String>(1)?,
        "url": url,
        "host": host,
        "title": r.get::<_, Option<String>>(3)?.unwrap_or_default(),
        "status": status.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
        "source": r.get::<_, String>(5)?,
        "added_by_claude": r.get::<_, bool>(6)?,
    }))
}

pub fn block(app: &App, id: i64) -> Option<Value> {
    let ctx = chat_ctx(app);
    let covered_block = app
        .awake
        .lock()
        .expect("awake lock")
        .active()
        .iter()
        .any(|r| r.block_id == Some(id));
    let detail = {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        let (title, source, repo, archived, last_active): (
            String,
            String,
            Option<String>,
            bool,
            i64,
        ) = c
            .query_row(
                "SELECT title, title_source, repo, archived, last_active FROM blocks WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .ok()?;
        let links: Vec<Value> = c
            .prepare(
                "SELECT id, kind, url, title, status, source, added_by_claude FROM links
                 WHERE block_id = ?1 ORDER BY kind = 'github_pr' DESC, kind = 'jira' DESC, id",
            )
            .and_then(|mut s| s.query_map([id], link_json).map(|r| r.flatten().collect()))
            .unwrap_or_default();
        // Valid links only, in case one was stored before validation tightened.
        let links: Vec<Value> = links
            .into_iter()
            .filter(|l| crate::links::validate(l["url"].as_str().unwrap_or("")).is_ok())
            .collect();
        let chats = chats_where(
            c,
            &ctx,
            "c.block_id = ?1 ORDER BY c.last_activity DESC",
            &[&id],
        );
        let (docs, outputs) = block_files(app, c, id);
        json!({
            "block": {
                "id": id, "title": title, "title_source": source,
                "repo": repo.as_deref().map(|r| tilde(app, r)),
                "repo_name": repo.as_deref().and_then(|r| Path::new(r).file_name()).map(|n| n.to_string_lossy().into_owned()),
                "archived": archived, "last_active": last_active,
            },
            "worktrees": block_worktrees(app, c, id),
            "links": links,
            "chats": chats,
            "docs": docs,
            "outputs": outputs,
            "keep_awake": {"block": covered_block},
            "new_chat_name": slug(&title),
        })
    };
    let mut detail = detail;
    detail["changes_since_review"] = changes_since_review(app, id);
    Some(detail)
}

/// A worktree name suggested from a block title.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    let out: String = out.chars().take(40).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "work".into() } else { out }
}

pub fn timeline(app: &App, id: i64) -> Option<Value> {
    let db = app.db.lock().expect("db lock");
    let c = db.conn();
    c.query_row("SELECT 1 FROM blocks WHERE id = ?1", [id], |_| Ok(()))
        .ok()?;
    let turns: Vec<Value> = c
        .prepare(
            "SELECT t.id, t.chat_id, COALESCE(c.title, 'Untitled chat'), t.prompt, t.started_at,
                    t.ended_at, w.rowid, t.files_changed, t.lines_added, t.lines_removed,
                    t.parallel, t.missed,
                    (t.start_snapshot IS NOT NULL AND (SELECT tree_id FROM snapshots WHERE id = t.start_snapshot) IS NULL)
                    OR (t.end_snapshot IS NOT NULL AND (SELECT tree_id FROM snapshots WHERE id = t.end_snapshot) IS NULL)
             FROM turns t JOIN chats c ON c.id = t.chat_id
             LEFT JOIN worktrees w ON w.path = t.worktree_path
             WHERE c.block_id = ?1 ORDER BY t.started_at DESC LIMIT 500",
        )
        .and_then(|mut s| {
            s.query_map([id], |r| {
                let prompt: Option<String> = r.get(3)?;
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "chat": r.get::<_, String>(1)?,
                    "chat_title": r.get::<_, String>(2)?,
                    "prompt": prompt.map(|p| crate::util::truncate_chars(&p, 200)),
                    "started_at": r.get::<_, i64>(4)?,
                    "ended_at": r.get::<_, Option<i64>>(5)?,
                    "worktree": r.get::<_, Option<i64>>(6)?,
                    "files_changed": r.get::<_, Option<i64>>(7)?,
                    "lines_added": r.get::<_, Option<i64>>(8)?,
                    "lines_removed": r.get::<_, Option<i64>>(9)?,
                    "parallel": r.get::<_, bool>(10)?,
                    "missed": r.get::<_, Option<String>>(11)?,
                    "snapshot_deleted": r.get::<_, bool>(12)?,
                }))
            })
            .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default();
    let reviews: Vec<Value> = c
        .prepare(
            "SELECT r.id, w.rowid, r.created_at FROM review_markers r
             LEFT JOIN worktrees w ON w.path = r.worktree_path
             WHERE r.block_id = ?1 ORDER BY r.created_at DESC",
        )
        .and_then(|mut s| {
            s.query_map([id], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "worktree": r.get::<_, Option<i64>>(1)?,
                    "created_at": r.get::<_, i64>(2)?,
                }))
            })
            .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default();
    Some(json!({"turns": turns, "reviews": reviews}))
}

pub fn worktree_path(app: &App, block: i64, worktree: Option<i64>) -> Option<(i64, String)> {
    let db = app.db.lock().expect("db lock");
    let c = db.conn();
    match worktree {
        Some(w) => {
            // Only a worktree that belongs to this block.
            let sql = format!("SELECT w.rowid, w.path {BLOCK_WORKTREES} AND w.rowid = ?2");
            c.query_row(&sql, params![block, w], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()
                .ok()
                .flatten()
        }
        None => primary_worktree(c, block),
    }
}

fn not_in_diff(
    app: &App,
    block: i64,
    worktree: &str,
    skipped: &crate::snapshots::Skipped,
) -> Vec<Value> {
    let mut out = Vec::new();
    let prefix = format!("{worktree}/");
    {
        let db = app.db.lock().expect("db lock");
        if let Ok(mut stmt) = db.conn().prepare(
            "SELECT DISTINCT w.path FROM chat_writes w JOIN chats c ON c.id = w.chat_id
             WHERE c.block_id = ?1 ORDER BY w.path LIMIT 2000",
        ) && let Ok(rows) = stmt.query_map([block], |r| r.get::<_, String>(0))
        {
            for p in rows.flatten() {
                let reason = if !p.starts_with(&prefix) {
                    "written outside this worktree"
                } else if p[prefix.len()..].starts_with(".git/") || p.contains("/.git/") {
                    "inside .git"
                } else {
                    continue;
                };
                out.push(json!({"path": tilde(app, &p), "reason": reason}));
            }
        }
    }
    for p in &skipped.too_large {
        out.push(json!({"path": p, "reason": "changed, too large to snapshot"}));
    }
    for p in &skipped.secrets {
        out.push(json!({"path": p, "reason": "looks like a secret"}));
    }
    for (p, why) in &skipped.new_left_out {
        out.push(json!({"path": p, "reason": format!("new file left out ({why})")}));
    }
    out.truncate(1000);
    out
}

/// A diff for the Changes page (protocol §2.4).
pub fn diff(
    app: &App,
    block: i64,
    worktree: Option<i64>,
    range: &str,
    turn: Option<i64>,
) -> Result<Value, String> {
    let (wid, wt) = worktree_path(app, block, worktree).ok_or("no worktree for this block")?;
    let wt_path = Path::new(&wt).to_path_buf();
    let default_branch: Option<String> = {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .query_row(
                "SELECT r.default_branch FROM worktrees w JOIN repos r ON r.root = w.repo_root
                 WHERE w.path = ?1",
                [&wt],
                |r| r.get(0),
            )
            .ok()
            .flatten()
    };

    let empty = |label_from: &str, label_to: &str, kind: &str, why: &str| {
        json!({
            "worktree": wid,
            "range": {"kind": kind, "from_label": label_from, "to_label": label_to},
            "files": [], "totals": {"files": 0, "additions": 0, "deletions": 0},
            "not_in_diff": [], "parallel": false, "unavailable": why,
        })
    };

    let (kind, from_label, to_label, shadow, from, to, skipped, parallel) = match range {
        "turn" => {
            let t = turn.ok_or("pick a turn")?;
            let row: Option<(Option<i64>, Option<i64>, bool, Option<String>, i64)> = {
                let db = app.db.lock().expect("db lock");
                db.conn()
                    .query_row(
                        "SELECT t.start_snapshot, t.end_snapshot, t.parallel, t.missed, t.started_at
                         FROM turns t JOIN chats c ON c.id = t.chat_id
                         WHERE t.id = ?1 AND c.block_id = ?2",
                        params![t, block],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
            };
            let (start, end, parallel, missed, started) = row.ok_or("no such turn")?;
            let label = format!("turn at {}", fmt_time(started));
            if let Some(m) = missed {
                return Ok(empty(&label, "end of turn", "turn", &m));
            }
            let (Some(start), Some(end)) = (start, end) else {
                return Ok(empty(
                    &label,
                    "end of turn",
                    "turn",
                    "this turn hasn't finished",
                ));
            };
            let (Some((root, a)), Some((_, b))) = (
                super::snapshots::tree_of(app, start),
                super::snapshots::tree_of(app, end),
            ) else {
                return Ok(empty(
                    &label,
                    "end of turn",
                    "turn",
                    "snapshot deleted to save space",
                ));
            };
            let shadow = Shadow::open(&app.paths.snapshots(), Path::new(&root))
                .map_err(|e| e.to_string())?;
            let skipped = super::snapshots::skipped_of(app, end);
            (
                "turn",
                label,
                "end of turn".to_string(),
                shadow,
                Some(a),
                b,
                skipped,
                parallel,
            )
        }
        _ => {
            let (shadow, now, skipped) = super::snapshots::now_tree(app, &wt_path)?;
            let review = {
                let db = app.db.lock().expect("db lock");
                latest_review(db.conn(), block, &wt)
            };
            let review_tree = review
                .and_then(|(snap, at)| super::snapshots::tree_of(app, snap).map(|(_, t)| (t, at)));
            match (range, review_tree) {
                ("review", Some((t, at))) => (
                    "review",
                    format!("your review at {}", fmt_time(at)),
                    "now".into(),
                    shadow,
                    Some(t),
                    now,
                    skipped,
                    false,
                ),
                _ => {
                    let base = default_branch
                        .as_deref()
                        .and_then(|d| shadow.merge_base_tree(&wt_path, d));
                    let label = match (&base, &default_branch) {
                        (Some(_), Some(d)) => format!("where this branch left {d}"),
                        _ => "the start of the repo".to_string(),
                    };
                    (
                        "branch",
                        label,
                        "now".into(),
                        shadow,
                        base,
                        now,
                        skipped,
                        false,
                    )
                }
            }
        }
    };
    let files = diff::diff_trees(&shadow.repo, from, to, false).map_err(|e| e.to_string())?;
    let totals = diff::totals(&files);
    // For ranges that end "now", the page sends this back to Mark reviewed, so the
    // marker covers exactly what was shown.
    let to_tree = (kind != "turn").then(|| to.to_string());
    Ok(json!({
        "worktree": wid,
        "to_tree": to_tree,
        "range": {"kind": kind, "from_label": from_label, "to_label": to_label},
        "files": files,
        "totals": totals,
        "not_in_diff": not_in_diff(app, block, &wt, &skipped),
        "parallel": parallel,
        "unavailable": Value::Null,
    }))
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs() {
        assert_eq!(
            slug("Add retry backoff (ABC-123)"),
            "add-retry-backoff-abc-123"
        );
        assert_eq!(slug("  !!  "), "work");
        assert!(slug(&"long title ".repeat(10)).len() <= 40);
    }
}
