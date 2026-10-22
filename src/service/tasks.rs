//! Background jobs: watching transcripts and config, refreshing worktrees, polling
//! `gh`, keep-awake, stale statuses, archiving, copies and backups.
//!
//! Lock order, wherever more than one is held: `db`, then `locator`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use rusqlite::params;
use tokio::sync::{Notify, mpsc};

use super::snapshots::Job;
use super::{Shared, UiEvent};
use crate::group;
use crate::ingest::Ingestor;
use crate::status::{self, Status};
use crate::util::now_ms;

pub struct Receivers {
    pub ingest: mpsc::UnboundedReceiver<PathBuf>,
    pub snapshots: mpsc::UnboundedReceiver<Job>,
}

pub fn spawn_all(app: Shared, rx: Receivers) {
    super::snapshots::spawn_worker(app.clone(), rx.snapshots);
    let worktrees_now = app.worktrees_now.clone();
    tokio::spawn(ingest_worker(app.clone(), rx.ingest));
    tokio::spawn(watch_loop(app.clone(), worktrees_now.clone()));
    tokio::spawn(worktree_loop(app.clone(), worktrees_now));
    tokio::spawn(gh_loop(app.clone()));
    tokio::spawn(housekeeping_loop(app.clone()));
    queue_all_transcripts(&app);
}

/// Every transcript under `~/.claude/projects`, newest first.
pub fn queue_all_transcripts(app: &Shared) {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let projects = app.paths.claude_projects();
    let Ok(folders) = std::fs::read_dir(&projects) else {
        return;
    };
    for folder in folders.flatten() {
        let Ok(entries) = std::fs::read_dir(folder.path()) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "jsonl") => {
                    let m = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    found.push((m, p));
                }
                // Subagent transcripts live in a folder named after the session.
                Ok(t) if t.is_dir() => {
                    for sub in walk_jsonl(&p, 3) {
                        found.push((std::time::UNIX_EPOCH, sub));
                    }
                }
                _ => {}
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in found {
        let _ = app.ingest_tx.send(p);
    }
}

fn walk_jsonl(dir: &Path, depth: u32) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "jsonl") => out.push(p),
            Ok(t) if t.is_dir() && depth > 0 => out.extend(walk_jsonl(&p, depth - 1)),
            _ => {}
        }
    }
    out
}

async fn ingest_worker(app: Shared, mut rx: mpsc::UnboundedReceiver<PathBuf>) {
    while let Some(first) = rx.recv().await {
        // Gather a burst of changes, then read each file once.
        let mut batch: Vec<PathBuf> = vec![first];
        tokio::time::sleep(Duration::from_millis(200)).await;
        while let Ok(p) = rx.try_recv() {
            batch.push(p);
        }
        let mut seen = HashSet::new();
        batch.retain(|p| seen.insert(p.clone()));
        let a = app.clone();
        let touched = tokio::task::spawn_blocking(move || {
            let mut touched = BTreeSet::new();
            for (i, p) in batch.iter().enumerate() {
                touched.extend(ingest_one(&a, p));
                // Let pages refresh during a long first scan.
                if i % 50 == 49 {
                    a.changed(std::mem::take(&mut touched));
                }
            }
            touched
        })
        .await
        .unwrap_or_default();
        if !touched.is_empty() {
            // New chats can bring new repos and worktrees.
            app.worktrees_now.notify_one();
        }
        app.changed(touched);
    }
}

/// Reads one transcript and re-places its chat. Returns the blocks that changed.
pub fn ingest_one(app: &super::App, path: &Path) -> BTreeSet<i64> {
    let config = app.config();
    let ing = Ingestor::new(&config, &app.paths);
    let now = now_ms();
    if std::fs::symlink_metadata(path).is_err() {
        let db = app.db.lock().expect("db lock");
        let _ = ing.forget_transcript(db.conn(), path);
        return BTreeSet::new();
    }
    let prev = {
        let db = app.db.lock().expect("db lock");
        crate::ingest::saved_position(db.conn(), path)
            .ok()
            .flatten()
    };
    let read = match ing.read(path, prev) {
        Ok(Some(r)) => r,
        Ok(None) => return BTreeSet::new(),
        Err(e) => {
            tracing::debug!(error = %e, "transcript read failed");
            return BTreeSet::new();
        }
    };
    let mut db = app.db.lock().expect("db lock");
    let Ok((chat, _)) = ing.record(db.conn_mut(), read, now) else {
        return BTreeSet::new();
    };
    let mut locator = app.locator.lock().expect("locator lock");
    let _ = group::refresh_location(db.conn(), &mut locator, &chat);
    drop(locator);
    group::place_chat(db.conn(), &config, &chat, now).unwrap_or_default()
}

async fn watch_loop(app: Shared, worktrees_now: Arc<Notify>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    }) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "file watching unavailable");
            return;
        }
    };
    let projects = app.paths.claude_projects();
    let config_dir = app.paths.config_dir();
    let mut watching_projects = false;
    let mut watching_config = false;
    let mut git_dirs: HashSet<PathBuf> = HashSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    loop {
        if !watching_projects && projects.is_dir() {
            watching_projects = watcher.watch(&projects, RecursiveMode::Recursive).is_ok();
        }
        if !watching_config && config_dir.is_dir() {
            watching_config = watcher
                .watch(&config_dir, RecursiveMode::NonRecursive)
                .is_ok();
        }
        // Each worktree's git folder, so branch switches show up straight away.
        for d in known_git_dirs(&app) {
            if !git_dirs.contains(&d) && watcher.watch(&d, RecursiveMode::NonRecursive).is_ok() {
                git_dirs.insert(d);
            }
        }
        tokio::select! {
            ev = rx.recv() => {
                let Some(Ok(ev)) = ev else { continue };
                for p in ev.paths {
                    if p.starts_with(&projects) {
                        if p.extension().is_some_and(|x| x == "jsonl") {
                            let _ = app.ingest_tx.send(p);
                        }
                    } else if p.starts_with(&config_dir) {
                        if p == app.paths.config_file() || p == app.paths.theme_file() {
                            app.reload_config();
                        }
                    } else if p.file_name().is_some_and(|n| n == "HEAD" || n == "index") {
                        worktrees_now.notify_one();
                    }
                }
            }
            _ = tick.tick() => {}
        }
    }
}

fn known_git_dirs(app: &Shared) -> Vec<PathBuf> {
    let paths: Vec<String> = {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .prepare("SELECT path FROM worktrees WHERE removed = 0")
            .and_then(|mut s| s.query_map([], |r| r.get(0)).map(|r| r.flatten().collect()))
            .unwrap_or_default()
    };
    paths
        .iter()
        .filter_map(|p| crate::worktrees::git_dir(Path::new(p)))
        .collect()
}

/// Re-reads every known repo's worktrees, and counts uncommitted files and commits
/// ahead for worktrees of active blocks.
pub fn refresh_worktrees(app: &super::App, count_for: Option<&BTreeSet<String>>) -> bool {
    let roots: Vec<String> = {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .prepare("SELECT root FROM repos")
            .and_then(|mut s| s.query_map([], |r| r.get(0)).map(|r| r.flatten().collect()))
            .unwrap_or_default()
    };
    let budget = Duration::from_secs(2);
    let now = now_ms();
    let mut changed = false;
    for root in roots {
        let list = crate::worktrees::list(Path::new(&root));
        if list.is_empty() {
            continue;
        }
        let default_branch: Option<String> = {
            let db = app.db.lock().expect("db lock");
            db.conn()
                .query_row(
                    "SELECT default_branch FROM repos WHERE root = ?1",
                    [&root],
                    |r| r.get(0),
                )
                .ok()
                .flatten()
        };
        for w in &list {
            let path = w.path.to_string_lossy().into_owned();
            let (branch, detached) = match &w.head {
                crate::worktrees::Head::Branch(b) => (Some(b.clone()), None),
                crate::worktrees::Head::Detached(c) => (None, Some(c.clone())),
                crate::worktrees::Head::Unborn => (None, None),
            };
            let counted = count_for.is_none_or(|set| set.contains(&path)) && !w.removed;
            let (uncommitted, ahead_behind) = if counted {
                let unc = crate::worktrees::uncommitted(&w.path, budget).map(|c| match c {
                    crate::worktrees::Count::Exact(n) => n as i64,
                    crate::worktrees::Count::Many => -1,
                });
                let ab = default_branch
                    .as_deref()
                    .and_then(|d| crate::worktrees::ahead_behind(&w.path, d));
                (unc, ab)
            } else {
                (None, None)
            };
            let db = app.db.lock().expect("db lock");
            let n = db
                .conn()
                .execute(
                    "INSERT INTO worktrees(path, repo_root, name, branch, detached_at, uncommitted,
                                           ahead, behind, last_read, removed)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(path) DO UPDATE SET
                        repo_root = excluded.repo_root, name = excluded.name,
                        branch = excluded.branch, detached_at = excluded.detached_at,
                        uncommitted = COALESCE(excluded.uncommitted, uncommitted),
                        ahead = COALESCE(excluded.ahead, ahead),
                        behind = COALESCE(excluded.behind, behind),
                        last_read = excluded.last_read, removed = excluded.removed
                     WHERE branch IS NOT excluded.branch OR detached_at IS NOT excluded.detached_at
                        OR removed != excluded.removed
                        OR (excluded.uncommitted IS NOT NULL AND uncommitted IS NOT excluded.uncommitted)
                        OR (excluded.ahead IS NOT NULL AND ahead IS NOT excluded.ahead)
                        OR (excluded.behind IS NOT NULL AND behind IS NOT excluded.behind)",
                    params![
                        path,
                        root,
                        w.name,
                        branch,
                        detached,
                        uncommitted,
                        ahead_behind.map(|a| a.0 as i64),
                        ahead_behind.map(|a| a.1 as i64),
                        now,
                        w.removed
                    ],
                )
                .unwrap_or(0);
            changed |= n > 0;
        }
        // Worktrees that git no longer lists at all.
        let listed: Vec<String> = list
            .iter()
            .map(|w| w.path.to_string_lossy().into_owned())
            .collect();
        let db = app.db.lock().expect("db lock");
        let stale: Vec<String> = db
            .conn()
            .prepare("SELECT path FROM worktrees WHERE repo_root = ?1 AND removed = 0")
            .and_then(|mut s| {
                s.query_map([&root], |r| r.get(0))
                    .map(|r| r.flatten().collect())
            })
            .unwrap_or_default();
        for p in stale {
            if !listed.contains(&p) && !Path::new(&p).exists() {
                let _ = db
                    .conn()
                    .execute("UPDATE worktrees SET removed = 1 WHERE path = ?1", [&p]);
                changed = true;
            }
        }
    }
    changed
}

/// Worktrees of blocks that aren't archived: the ones worth counting.
fn active_worktrees(app: &super::App) -> BTreeSet<String> {
    let db = app.db.lock().expect("db lock");
    db.conn()
        .prepare(
            "SELECT DISTINCT w.path FROM worktrees w
             WHERE EXISTS (SELECT 1 FROM block_keys k JOIN blocks b ON b.id = k.block_id
                           WHERE b.archived = 0 AND k.key = 'branch:' || w.repo_root || '@' || w.branch)
                OR w.path IN (SELECT c.worktree_path FROM chats c JOIN blocks b ON b.id = c.block_id
                              WHERE b.archived = 0)",
        )
        .and_then(|mut s| s.query_map([], |r| r.get(0)).map(|r| r.flatten().collect()))
        .unwrap_or_default()
}

async fn worktree_loop(app: Shared, now: Arc<Notify>) {
    loop {
        let interval = app.config().worktrees.status_interval_seconds.max(5);
        let a = app.clone();
        let changed = tokio::task::spawn_blocking(move || {
            let active = active_worktrees(&a);
            refresh_worktrees(&a, Some(&active))
        })
        .await
        .unwrap_or(false);
        if changed {
            app.changed([]);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
            _ = now.notified() => {
                // A burst of git writes: wait for it to settle.
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Polls `gh` for one GitHub repo and applies the result.
pub async fn poll_repo(app: &Shared, github: &str) -> Result<(), String> {
    let gh = app.gh.clone().ok_or("gh is not installed")?;
    let neutral = app.paths.data();
    let prs = crate::links::gh::list_prs(&gh, github, &neutral)
        .await
        .map_err(|e| e.to_string())?;
    let config = app.config();
    let touched = {
        let db = app.db.lock().expect("db lock");
        group::apply_prs(db.conn(), &config, github, &prs, now_ms()).map_err(|e| e.to_string())?
    };
    let _ = app
        .db
        .lock()
        .expect("db lock")
        .set_meta(&format!("gh.last.{github}"), &now_ms().to_string());
    app.changed(touched);
    Ok(())
}

async fn gh_loop(app: Shared) {
    // Per repo: consecutive failures, and when to try next.
    let mut backoff: HashMap<String, (u32, i64)> = HashMap::new();
    loop {
        let repos: Vec<String> = {
            let db = app.db.lock().expect("db lock");
            db.conn()
                .prepare("SELECT DISTINCT github FROM repos WHERE github IS NOT NULL")
                .and_then(|mut s| s.query_map([], |r| r.get(0)).map(|r| r.flatten().collect()))
                .unwrap_or_default()
        };
        let now = now_ms();
        for repo in repos {
            if backoff.get(&repo).is_some_and(|(_, next)| *next > now) {
                continue;
            }
            match poll_repo(&app, &repo).await {
                Ok(()) => {
                    backoff.remove(&repo);
                }
                Err(e) => {
                    let fails = backoff.get(&repo).map_or(0, |b| b.0) + 1;
                    let wait = (60_000i64 << fails.min(6)).min(3_600_000);
                    backoff.insert(repo.clone(), (fails, now + wait));
                    tracing::debug!(repo = %repo, error = %e, "gh poll failed");
                }
            }
        }
        let minutes = app.config().github.poll_minutes.max(1);
        tokio::time::sleep(Duration::from_secs(minutes * 60)).await;
    }
}

/// A block gained focus: refresh its PRs (at most once a minute) and its worktrees.
pub fn on_block_focus(app: Shared, block: i64) {
    tokio::spawn(async move {
        let github: Option<String> = {
            let db = app.db.lock().expect("db lock");
            db.conn()
                .query_row(
                    "SELECT r.github FROM blocks b JOIN repos r ON r.root = b.repo WHERE b.id = ?1",
                    [block],
                    |r| r.get(0),
                )
                .ok()
                .flatten()
        };
        if let Some(g) = github {
            let last: i64 = app
                .db
                .lock()
                .expect("db lock")
                .meta(&format!("gh.last.{g}"))
                .ok()
                .flatten()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if now_ms() - last > 60_000 {
                let _ = poll_repo(&app, &g).await;
            }
        }
        let a = app.clone();
        let changed = tokio::task::spawn_blocking(move || {
            let paths: BTreeSet<String> = {
                let db = a.db.lock().expect("db lock");
                db.conn()
                    .prepare("SELECT DISTINCT worktree_path FROM chats WHERE block_id = ?1 AND worktree_path IS NOT NULL")
                    .and_then(|mut s| s.query_map([block], |r| r.get(0)).map(|r| r.flatten().collect()))
                    .unwrap_or_default()
            };
            refresh_worktrees(&a, Some(&paths))
        })
        .await
        .unwrap_or(false);
        if changed {
            app.changed([block]);
        }
    });
}

pub fn restore_keep_awake(app: &Shared) {
    let rows: Vec<crate::awake::Request> = {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .prepare("SELECT id, chat_id, block_id, chats, started_at FROM keep_awake WHERE ended_at IS NULL")
            .and_then(|mut s| {
                s.query_map([], |r| {
                    let chats: String = r.get(3)?;
                    Ok(crate::awake::Request {
                        id: r.get(0)?,
                        chat_id: r.get(1)?,
                        block_id: r.get(2)?,
                        chats: serde_json::from_str(&chats).unwrap_or_default(),
                        started_at: r.get(4)?,
                    })
                })
                .map(|r| r.flatten().collect())
            })
            .unwrap_or_default()
    };
    let mut m = app.awake.lock().expect("awake lock");
    for r in rows {
        let _ = m.add(r);
    }
}

pub fn tick_keep_awake(app: &Shared, now: i64) {
    let covered = app.awake.lock().expect("awake lock").covered_chats();
    if covered.is_empty() {
        return;
    }
    let statuses: HashMap<String, Status> = {
        let db = app.db.lock().expect("db lock");
        covered
            .iter()
            .filter_map(|c| {
                db.conn()
                    .query_row("SELECT status FROM chats WHERE id = ?1", [c], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok()
                    .map(|s| (c.clone(), Status::parse(&s)))
            })
            .collect()
    };
    let ended = app.awake.lock().expect("awake lock").tick(&statuses, now);
    if ended.is_empty() {
        return;
    }
    let db = app.db.lock().expect("db lock");
    for (r, why) in ended {
        let _ = db.conn().execute(
            "UPDATE keep_awake SET ended_at = ?2, end_reason = ?3 WHERE id = ?1",
            params![r.id, now, why.as_str()],
        );
    }
    drop(db);
    app.emit(UiEvent::Changed {
        blocks: Vec::new(),
        board: true,
    });
}

/// Marks chats closed that have been "working" for 10 minutes with no events and no
/// iTerm2 session.
pub fn close_stale(app: &Shared, now: i64) -> BTreeSet<i64> {
    let sessions: HashSet<String> = app
        .sessions
        .lock()
        .expect("sessions lock")
        .keys()
        .cloned()
        .collect();
    let db = app.db.lock().expect("db lock");
    let rows: Vec<(String, Option<i64>, Option<String>, Option<i64>)> = db
        .conn()
        .prepare(
            "SELECT id, last_event_at, iterm_session, block_id FROM chats WHERE status = 'working'",
        )
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .map(|r| r.flatten().collect())
        })
        .unwrap_or_default();
    let mut touched = BTreeSet::new();
    for (id, last, session, block) in rows {
        let state = status::State {
            status: Status::Working,
            background: false,
        };
        let exists = session.as_ref().is_some_and(|s| sessions.contains(s));
        if status::is_stale(state, last.unwrap_or(0), now, exists) {
            let _ = db.conn().execute(
                "UPDATE chats SET status = 'closed', background = 0, status_since = ?2 WHERE id = ?1",
                params![id, now],
            );
            touched.extend(block);
        }
    }
    touched
}

/// Copies docs and outputs found in temporary folders, then keeps the total in bounds.
pub fn copy_temp_files(app: &super::App) -> BTreeSet<i64> {
    let config = app.config();
    let rows: Vec<(i64, String, i64, Option<String>)> = {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .prepare(
                "SELECT f.id, f.path, COALESCE(c.block_id, f.block_id), f.copy_path
                 FROM files f LEFT JOIN chats c ON c.id = f.chat_id
                 WHERE COALESCE(c.block_id, f.block_id) IS NOT NULL",
            )
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                    .map(|r| r.flatten().collect())
            })
            .unwrap_or_default()
    };
    let limits = crate::files::copies::Limits {
        per_file_bytes: config.outputs.copy_max_mb * 1024 * 1024,
        total_bytes: config.outputs.copies_max_total_gb * 1024 * 1024 * 1024,
    };
    let mut touched = BTreeSet::new();
    for (id, path, block, copy) in rows {
        let p = Path::new(&path);
        if !crate::files::in_temp_dir(p, &config) {
            continue;
        }
        let Ok(src) = std::fs::symlink_metadata(p) else {
            continue;
        };
        let fresh = copy
            .as_deref()
            .and_then(|c| std::fs::metadata(c).ok())
            .is_some_and(|m| m.modified().ok() >= src.modified().ok());
        if fresh {
            continue;
        }
        if let Ok(crate::files::copies::Outcome::Copied { path: dest, .. }) =
            crate::files::copies::copy_file(p, &app.paths.copies(), block, id, limits)
        {
            let db = app.db.lock().expect("db lock");
            let _ = db.conn().execute(
                "UPDATE files SET copy_path = ?2 WHERE id = ?1",
                params![id, dest.to_string_lossy()],
            );
            touched.insert(block);
        }
    }
    let archived: HashSet<i64> = {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .prepare("SELECT id FROM blocks WHERE archived = 1")
            .and_then(|mut s| s.query_map([], |r| r.get(0)).map(|r| r.flatten().collect()))
            .unwrap_or_default()
    };
    if let Ok(removed) =
        crate::files::copies::enforce_total(&app.paths.copies(), limits.total_bytes, &archived)
    {
        let db = app.db.lock().expect("db lock");
        for p in removed {
            let _ = db.conn().execute(
                "UPDATE files SET copy_path = NULL WHERE copy_path = ?1",
                [p.to_string_lossy()],
            );
        }
    }
    touched
}

async fn housekeeping_loop(app: Shared) {
    let mut minute = tokio::time::interval(Duration::from_secs(60));
    let mut ticks: u64 = 0;
    loop {
        minute.tick().await;
        ticks += 1;
        let now = now_ms();
        let mut touched = close_stale(&app, now);
        tick_keep_awake(&app, now);
        let a = app.clone();
        let hourly = ticks % 60 == 1;
        touched.extend(
            tokio::task::spawn_blocking(move || {
                let mut t = copy_temp_files(&a);
                if hourly {
                    let config = a.config();
                    {
                        let db = a.db.lock().expect("db lock");
                        let _ = group::archive_sweep(db.conn(), &config, now);
                    }
                    daily(&a, now);
                    t.insert(-1);
                }
                t
            })
            .await
            .unwrap_or_default(),
        );
        touched.remove(&-1);
        if !touched.is_empty() || hourly {
            app.changed(touched);
        }
    }
}

/// Once a day: back up hand-entered data and clear old snapshots of archived blocks.
fn daily(app: &super::App, now: i64) {
    let last: i64 = app
        .db
        .lock()
        .expect("db lock")
        .meta("backup.last")
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if now - last < 86_400_000 {
        return;
    }
    {
        let db = app.db.lock().expect("db lock");
        if db.backup_manual(&app.paths.backups(), now, None).is_ok() {
            let _ = db.set_meta("backup.last", &now.to_string());
        }
    }
    super::snapshots::sweep_archived(app, now);
}
