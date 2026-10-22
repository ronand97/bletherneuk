//! Taking snapshots for turns and reviews, and keeping shadow stores within limits.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use git2::Oid;
use rusqlite::{OptionalExtension, params};

use super::{App, Shared};
use crate::snapshots::{self, Limits, Shadow, Skipped, diff, prune};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub worktree: PathBuf,
    pub reason: &'static str,
    pub turn: Option<(i64, Edge)>,
    /// Files written during the turn, in case the transcript hasn't been read yet.
    pub extra: Vec<PathBuf>,
}

/// Runs snapshot jobs one at a time, in order, so a turn's end snapshot never
/// overtakes its start snapshot.
pub fn spawn_worker(app: Shared, mut rx: tokio::sync::mpsc::UnboundedReceiver<Job>) {
    std::thread::Builder::new()
        .name("snapshots".into())
        .spawn(move || {
            while let Some(job) = rx.blocking_recv() {
                run_job(&app, &job);
            }
        })
        .expect("spawn snapshot thread");
}

pub fn run_job(app: &App, job: &Job) {
    let result = take(app, &job.worktree, job.reason, &job.extra);
    let Some((turn, edge)) = job.turn else {
        return;
    };
    match (&result, edge) {
        (Ok(id), Edge::Start) => {
            let db = app.db.lock().expect("db lock");
            let _ = db.conn().execute(
                "UPDATE turns SET start_snapshot = ?2 WHERE id = ?1",
                params![turn, id],
            );
        }
        (Ok(id), Edge::End) => {
            {
                let db = app.db.lock().expect("db lock");
                let _ = db.conn().execute(
                    "UPDATE turns SET end_snapshot = ?2 WHERE id = ?1",
                    params![turn, id],
                );
            }
            record_turn_counts(app, turn);
            let db = app.db.lock().expect("db lock");
            mark_parallel(db.conn(), turn);
        }
        (Err(why), _) => {
            let db = app.db.lock().expect("db lock");
            let _ = db.conn().execute(
                "UPDATE turns SET missed = ?2 WHERE id = ?1",
                params![turn, format!("snapshot missed: {why}")],
            );
        }
    }
    let block: Option<i64> = app
        .db
        .lock()
        .expect("db lock")
        .conn()
        .query_row(
            "SELECT c.block_id FROM turns t JOIN chats c ON c.id = t.chat_id WHERE t.id = ?1",
            [turn],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    app.changed(block);
}

fn repo_lock(app: &App, repo: &Path) -> Arc<Mutex<()>> {
    app.snap_locks
        .lock()
        .expect("snap locks")
        .entry(repo.to_path_buf())
        .or_default()
        .clone()
}

/// Files chats wrote inside `worktree`, for the snapshot's new-file rule.
fn chat_files(app: &App, worktree: &Path) -> Vec<PathBuf> {
    let prefix = format!("{}/", worktree.display());
    let db = app.db.lock().expect("db lock");
    let mut stmt = match db
        .conn()
        .prepare("SELECT DISTINCT path FROM chat_writes WHERE substr(path, 1, ?2) = ?1")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(params![prefix, prefix.len() as i64], |r| {
        r.get::<_, String>(0)
    })
    .map(|rows| rows.flatten().map(PathBuf::from).collect())
    .unwrap_or_default()
}

fn latest_tree(app: &App, worktree: &Path) -> Option<Oid> {
    let db = app.db.lock().expect("db lock");
    db.conn()
        .query_row(
            "SELECT tree_id FROM snapshots WHERE worktree_path = ?1 AND tree_id IS NOT NULL
             ORDER BY id DESC LIMIT 1",
            [worktree.to_string_lossy()],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| Oid::from_str(&s).ok())
}

/// Takes and records a snapshot. Returns its ID, or why it was missed.
pub fn take(app: &App, worktree: &Path, reason: &str, extra: &[PathBuf]) -> Result<i64, String> {
    let loc = app
        .locator
        .lock()
        .expect("locator lock")
        .locate(worktree)
        .ok_or("not a git repo")?;
    let lock = repo_lock(app, &loc.repo_root);
    let _guard = lock.lock().expect("repo lock");
    let mut files = chat_files(app, &loc.worktree);
    files.extend(extra.iter().cloned());
    let previous = latest_tree(app, &loc.worktree);
    let limits = Limits::from_config(&app.config());
    let shadow = Shadow::open(&app.paths.snapshots(), &loc.repo_root).map_err(|e| e.to_string())?;
    let taken = shadow
        .take(&loc.worktree, &files, previous, &limits)
        .map_err(|e| e.to_string())?;
    let id = {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        c.execute(
            "INSERT INTO snapshots(repo_root, worktree_path, tree_id, taken_at, reason, skipped)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                loc.repo_root.to_string_lossy(),
                loc.worktree.to_string_lossy(),
                taken.tree.to_string(),
                crate::util::now_ms(),
                reason,
                serde_json::to_string(&taken.skipped).unwrap_or_default()
            ],
        )
        .map_err(|e| e.to_string())?;
        c.last_insert_rowid()
    };
    shadow.keep(id, taken.tree).map_err(|e| e.to_string())?;
    let max = app.config().snapshots.max_store_gb * 1024 * 1024 * 1024;
    if prune::store_size(&shadow.path) > max {
        enforce_limit(app, &shadow, &loc.repo_root, max);
    }
    Ok(id)
}

/// A snapshot of the worktree right now that isn't recorded, for "… to now" diffs.
pub fn now_tree(app: &App, worktree: &Path) -> Result<(Shadow, Oid, Skipped), String> {
    let loc = app
        .locator
        .lock()
        .expect("locator lock")
        .locate(worktree)
        .ok_or("not a git repo")?;
    let lock = repo_lock(app, &loc.repo_root);
    let _guard = lock.lock().expect("repo lock");
    let files = chat_files(app, &loc.worktree);
    let previous = latest_tree(app, &loc.worktree);
    let limits = Limits::from_config(&app.config());
    let shadow = Shadow::open(&app.paths.snapshots(), &loc.repo_root).map_err(|e| e.to_string())?;
    let taken = shadow
        .take(&loc.worktree, &files, previous, &limits)
        .map_err(|e| e.to_string())?;
    Ok((shadow, taken.tree, taken.skipped))
}

pub fn tree_of(app: &App, snapshot: i64) -> Option<(String, Oid)> {
    let db = app.db.lock().expect("db lock");
    db.conn()
        .query_row(
            "SELECT repo_root, tree_id FROM snapshots WHERE id = ?1",
            [snapshot],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .ok()
        .and_then(|(root, t)| Some((root, Oid::from_str(&t?).ok()?)))
}

fn record_turn_counts(app: &App, turn: i64) {
    let ids: Option<(Option<i64>, Option<i64>)> = app
        .db
        .lock()
        .expect("db lock")
        .conn()
        .query_row(
            "SELECT start_snapshot, end_snapshot FROM turns WHERE id = ?1",
            [turn],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((Some(start), Some(end))) = ids else {
        return;
    };
    let (Some((root, a)), Some((_, b))) = (tree_of(app, start), tree_of(app, end)) else {
        return;
    };
    let Ok(shadow) = Shadow::open(&app.paths.snapshots(), Path::new(&root)) else {
        return;
    };
    if let Ok(files) = diff::diff_trees(&shadow.repo, Some(a), b, true) {
        let t = diff::totals(&files);
        let _ = app.db.lock().expect("db lock").conn().execute(
            "UPDATE turns SET files_changed = ?2, lines_added = ?3, lines_removed = ?4 WHERE id = ?1",
            params![turn, t.files as i64, t.additions as i64, t.deletions as i64],
        );
    }
}

/// Notes when another chat was active in the same worktree during a turn.
fn mark_parallel(c: &rusqlite::Connection, turn: i64) {
    let _ = c.execute(
        "UPDATE turns SET parallel = 1 WHERE id = ?1 AND EXISTS (
            SELECT 1 FROM turns o, turns t
            WHERE t.id = ?1 AND o.id != t.id AND o.chat_id != t.chat_id
              AND o.worktree_path = t.worktree_path
              AND o.started_at < COALESCE(t.ended_at, t.started_at)
              AND COALESCE(o.ended_at, t.ended_at, t.started_at) > t.started_at)",
        [turn],
    );
}

/// Deletes one snapshot's data. Its turn keeps its counts.
fn delete_snapshot(app: &App, shadow: &Shadow, id: i64) {
    let _ = shadow.forget(id);
    let _ = app
        .db
        .lock()
        .expect("db lock")
        .conn()
        .execute("UPDATE snapshots SET tree_id = NULL WHERE id = ?1", [id]);
}

/// Brings a shadow store under `max` bytes (design §13): reviewed snapshots first, then
/// the oldest, never the latest of a worktree or a review marker's.
pub fn enforce_limit(app: &App, shadow: &Shadow, repo_root: &Path, max: u64) {
    let candidates: Vec<prune::Candidate> = {
        let db = app.db.lock().expect("db lock");
        let mut stmt = match db.conn().prepare(
            "SELECT s.id, s.taken_at,
                    s.taken_at < COALESCE((SELECT MAX(r.created_at) FROM review_markers r
                                           WHERE r.worktree_path = s.worktree_path), 0),
                    s.id = (SELECT MAX(id) FROM snapshots x WHERE x.worktree_path = s.worktree_path)
                      OR s.id IN (SELECT snapshot_id FROM review_markers)
             FROM snapshots s WHERE s.repo_root = ?1 AND s.tree_id IS NOT NULL",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        stmt.query_map([repo_root.to_string_lossy()], |r| {
            Ok(prune::Candidate {
                id: r.get(0)?,
                taken_at: r.get(1)?,
                reviewed: r.get(2)?,
                protected: r.get(3)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    };
    let order = prune::deletion_order(&candidates);
    for batch in order.chunks(10) {
        for id in batch {
            delete_snapshot(app, shadow, *id);
        }
        let _ = prune::prune_unreferenced(shadow, Duration::from_secs(3600));
        if prune::store_size(&shadow.path) <= max {
            break;
        }
    }
}

/// Deletes snapshots of blocks archived more than `keep_days` ago (design §13).
pub fn sweep_archived(app: &App, now: i64) {
    let keep_ms = app.config().snapshots.keep_days_after_archive as i64 * 86_400_000;
    let rows: Vec<(i64, String)> = {
        let db = app.db.lock().expect("db lock");
        let mut stmt = match db.conn().prepare(
            "SELECT s.id, s.repo_root FROM snapshots s
             WHERE s.tree_id IS NOT NULL AND s.taken_at < ?1
               AND EXISTS (SELECT 1 FROM chats c JOIN blocks b ON b.id = c.block_id
                           WHERE c.worktree_path = s.worktree_path AND b.archived = 1)
               AND NOT EXISTS (SELECT 1 FROM chats c JOIN blocks b ON b.id = c.block_id
                               WHERE c.worktree_path = s.worktree_path AND b.archived = 0)",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        stmt.query_map([now - keep_ms], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    };
    let mut roots = std::collections::BTreeSet::new();
    for (id, root) in rows {
        if let Ok(shadow) = Shadow::open(&app.paths.snapshots(), Path::new(&root)) {
            delete_snapshot(app, &shadow, id);
            roots.insert(root);
        }
    }
    for root in roots {
        if let Ok(shadow) = Shadow::open(&app.paths.snapshots(), Path::new(&root)) {
            let _ = prune::prune_unreferenced(&shadow, Duration::from_secs(3600));
        }
    }
}

pub fn skipped_of(app: &App, snapshot: i64) -> Skipped {
    app.db
        .lock()
        .expect("db lock")
        .conn()
        .query_row(
            "SELECT skipped FROM snapshots WHERE id = ?1",
            [snapshot],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub use snapshots::repo_id;
