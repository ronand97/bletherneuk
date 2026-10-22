//! What the page buttons and the command line do (protocol §2.5 and §1.1).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use super::Shared;
use super::bridge::{self, BridgeError};
use crate::awake::Request;
use crate::util::now_ms;

pub type Outcome = Result<Value, String>;

/// Worktree names. The first character must be a letter or digit, so a name can never
/// be read as a command-line flag (`--something`) or be `.` or `..`.
static WORKTREE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").unwrap());

fn ok() -> Outcome {
    Ok(json!({"ok": true}))
}

fn bridge_err(e: BridgeError) -> String {
    e.to_string()
}

fn chat_row(
    app: &Shared,
    chat: &str,
) -> Result<(Option<i64>, Option<String>, Option<String>, Option<String>), String> {
    let db = app.db.lock().expect("db lock");
    db.conn()
        .query_row(
            "SELECT block_id, last_cwd, title, iterm_session FROM chats WHERE id = ?1",
            [chat],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no such chat".to_string())
}

fn block_title(app: &Shared, block: i64) -> Result<String, String> {
    let db = app.db.lock().expect("db lock");
    db.conn()
        .query_row("SELECT title FROM blocks WHERE id = ?1", [block], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no such block".to_string())
}

/// Focuses the chat's tab, or resumes it in the block's window.
pub async fn open_chat(app: &Shared, chat: &str) -> Outcome {
    let (block, _, title, session) = chat_row(app, chat)?;
    let live = session.filter(|s| app.sessions.lock().expect("sessions lock").contains_key(s));
    let folder = match &live {
        Some(_) => None,
        None => Some(resume_folder(app, chat).map_err(|why| format!("can't resume: {why}"))?),
    };
    let window_title = match block {
        Some(b) => block_title(app, b)?,
        None => "Unsorted".into(),
    };
    let reply = bridge::request(
        app,
        json!({
            "op": "open_chat", "chat": chat, "session": live, "folder": folder,
            "block": block, "window_title": window_title,
            "tab_title": title.unwrap_or_else(|| "Claude".into()),
        }),
    )
    .await
    .map_err(bridge_err)?;
    if let Some(s) = reply.session.filter(|s| crate::ingest::is_uuid(s)) {
        let db = app.db.lock().expect("db lock");
        let _ = db.conn().execute(
            "UPDATE chats SET iterm_session = ?2 WHERE id = ?1",
            params![chat, s.to_ascii_uppercase()],
        );
    }
    ok()
}

/// Where to resume a chat, or why it can't be resumed. A chat whose worktree was removed
/// resumes in the main checkout: `claude --resume <id>` finds a session from any folder
/// (Claude Code 2.1.223 and later) and carries on there.
pub fn resume_folder(app: &Shared, chat: &str) -> Result<String, &'static str> {
    let db = app.db.lock().expect("db lock");
    let (gone, cwd, removed, root): (bool, Option<String>, Option<bool>, Option<String>) = db
        .conn()
        .query_row(
            "SELECT c.transcript_gone, c.last_cwd, w.removed, COALESCE(w.repo_root, c.repo_root)
             FROM chats c LEFT JOIN worktrees w ON w.path = c.worktree_path WHERE c.id = ?1",
            [chat],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| "no such chat")?;
    let is_dir = |p: &Option<String>| p.as_deref().is_some_and(|p| Path::new(p).is_dir());
    if gone {
        Err("transcript deleted")
    } else if removed == Some(true) {
        if is_dir(&root) {
            Ok(root.expect("checked"))
        } else {
            Err("worktree removed")
        }
    } else if is_dir(&cwd) {
        Ok(cwd.expect("checked"))
    } else {
        Err("folder missing")
    }
}

pub fn move_chat(app: &Shared, chat: &str, block: Option<i64>) -> Outcome {
    let config = app.config();
    let touched = {
        let db = app.db.lock().expect("db lock");
        if let Some(b) = block {
            db.conn()
                .query_row("SELECT 1 FROM blocks WHERE id = ?1", [b], |_| Ok(()))
                .map_err(|_| "no such block".to_string())?;
        }
        crate::group::move_chat(db.conn(), &config, chat, block, now_ms())
            .map_err(|e| e.to_string())?
    };
    app.changed(touched);
    ok()
}

pub fn rename_block(app: &Shared, block: i64, title: Option<&str>) -> Outcome {
    let config = app.config();
    block_title(app, block)?;
    {
        let db = app.db.lock().expect("db lock");
        crate::group::rename_block(db.conn(), &config, block, title, now_ms())
            .map_err(|e| e.to_string())?;
    }
    app.changed([block]);
    ok()
}

fn insert_keep_awake(
    app: &Shared,
    chat: Option<&str>,
    block: Option<i64>,
    chats: BTreeSet<String>,
) -> Outcome {
    let now = now_ms();
    let id = {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .execute(
                "INSERT INTO keep_awake(chat_id, block_id, chats, started_at) VALUES (?1, ?2, ?3, ?4)",
                params![chat, block, serde_json::to_string(&chats).unwrap_or_default(), now],
            )
            .map_err(|e| e.to_string())?;
        db.conn().last_insert_rowid()
    };
    app.awake
        .lock()
        .expect("awake lock")
        .add(Request {
            id,
            chat_id: chat.map(str::to_string),
            block_id: block,
            chats,
            started_at: now,
        })
        .map_err(|e| format!("couldn't start caffeinate: {e}"))?;
    ok()
}

fn cancel_keep_awake(app: &Shared, matches: impl Fn(&Request) -> bool) {
    let ids: Vec<i64> = app
        .awake
        .lock()
        .expect("awake lock")
        .active()
        .iter()
        .filter(|r| matches(r))
        .map(|r| r.id)
        .collect();
    let now = now_ms();
    for id in ids {
        app.awake.lock().expect("awake lock").cancel(id);
        let db = app.db.lock().expect("db lock");
        let _ = db.conn().execute(
            "UPDATE keep_awake SET ended_at = ?2, end_reason = 'cancelled' WHERE id = ?1",
            params![id, now],
        );
    }
}

pub fn keep_awake_chat(app: &Shared, chat: &str, on: bool) -> Outcome {
    let (block, ..) = chat_row(app, chat)?;
    if on {
        let status: String = app
            .db
            .lock()
            .expect("db lock")
            .conn()
            .query_row("SELECT status FROM chats WHERE id = ?1", [chat], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if status != "working" {
            return Err("this chat isn't working".into());
        }
        insert_keep_awake(app, Some(chat), None, BTreeSet::from([chat.to_string()]))?;
    } else {
        cancel_keep_awake(app, |r| r.chat_id.as_deref() == Some(chat));
    }
    app.changed(block);
    ok()
}

/// Covers the chats working right now (design §12).
pub fn keep_awake_block(app: &Shared, block: i64, on: bool) -> Outcome {
    block_title(app, block)?;
    if on {
        let chats: BTreeSet<String> = {
            let db = app.db.lock().expect("db lock");
            db.conn()
                .prepare("SELECT id FROM chats WHERE block_id = ?1 AND status = 'working'")
                .and_then(|mut s| {
                    s.query_map([block], |r| r.get(0))
                        .map(|r| r.flatten().collect())
                })
                .map_err(|e| e.to_string())?
        };
        if chats.is_empty() {
            return Err("no chats here are working".into());
        }
        insert_keep_awake(app, None, Some(block), chats)?;
    } else {
        cancel_keep_awake(app, |r| r.block_id == Some(block));
    }
    app.changed([block]);
    ok()
}

/// The chats to bring back when a closed block window is reopened: those open when it
/// was last closed, else the most recent one.
fn chats_for_window(app: &Shared, block: i64) -> Vec<Value> {
    let db = app.db.lock().expect("db lock");
    let c = db.conn();
    let saved: Vec<String> = c
        .query_row(
            "SELECT chats FROM bridge_windows WHERE block_id = ?1 ORDER BY rowid DESC LIMIT 1",
            [block],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut ids: Vec<String> = saved;
    if ids.is_empty()
        && let Ok(id) = c.query_row(
            "SELECT id FROM chats WHERE block_id = ?1 AND hidden = 0 AND transcript_gone = 0
             ORDER BY last_activity DESC LIMIT 1",
            [block],
            |r| r.get::<_, String>(0),
        )
    {
        ids.push(id);
    }
    let titled: Vec<(String, String)> = ids
        .into_iter()
        .filter_map(|id| {
            c.query_row(
                "SELECT COALESCE(title, 'Claude') FROM chats WHERE id = ?1 AND block_id = ?2",
                params![id, block],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .map(|t| (id, t))
        })
        .collect();
    drop(db);
    titled
        .into_iter()
        .filter_map(|(id, title)| {
            let folder = resume_folder(app, &id).ok()?;
            Some(json!({"chat": id, "folder": folder, "tab_title": title}))
        })
        .collect()
}

pub async fn open_block(app: &Shared, block: i64) -> Outcome {
    let title = block_title(app, block)?;
    let chats = chats_for_window(app, block);
    bridge::request(
        app,
        json!({"op": "open_block", "block": block, "window_title": title, "chats": chats}),
    )
    .await
    .map_err(bridge_err)?;
    ok()
}

pub async fn new_chat(
    app: &Shared,
    block: i64,
    place: &str,
    worktree: Option<i64>,
    name: Option<&str>,
) -> Outcome {
    let title = block_title(app, block)?;
    let (folder, worktree_name) = match place {
        "worktree" => {
            let (_, path) = super::views::worktree_path(app, block, worktree)
                .ok_or("this block has no worktree")?;
            (path, None)
        }
        "main" | "new_worktree" => {
            let repo: Option<String> = app
                .db
                .lock()
                .expect("db lock")
                .conn()
                .query_row("SELECT repo FROM blocks WHERE id = ?1", [block], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            let repo = repo.ok_or("this block has no repo")?;
            if place == "new_worktree" {
                let n = name.map(str::trim).unwrap_or_default();
                if !WORKTREE_NAME.is_match(n) {
                    return Err("worktree names start with a letter or digit, then use letters, digits, '.', '_' and '-' (at most 64)".into());
                }
                (repo, Some(n.to_string()))
            } else {
                (repo, None)
            }
        }
        _ => return Err("unknown place".into()),
    };
    if !Path::new(&folder).is_dir() {
        return Err("that folder no longer exists".into());
    }
    let reply = bridge::request(
        app,
        json!({"op": "new_chat", "block": block, "folder": folder,
               "worktree_name": worktree_name, "window_title": title}),
    )
    .await
    .map_err(bridge_err)?;
    if let Some(s) = reply.session.filter(|s| crate::ingest::is_uuid(s)) {
        let mut sessions = app.sessions.lock().expect("sessions lock");
        sessions.entry(s.to_ascii_uppercase()).or_default().block = Some(block);
    }
    ok()
}

/// Attaches a link to a block. Links added from inside a Claude chat are marked.
pub fn add_link(
    app: &Shared,
    block: i64,
    url: &str,
    title: Option<&str>,
    by_claude: bool,
) -> Outcome {
    let r = crate::links::recognise(url).map_err(str::to_string)?;
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| crate::util::truncate_chars(t, 200))
        .unwrap_or(r.title.clone());
    let config = app.config();
    let now = now_ms();
    block_title(app, block)?;
    let (block, link) = {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        c.execute(
            "INSERT INTO links(block_id, kind, url, title, source, added_by_claude, created_at)
             VALUES (?1, ?2, ?3, ?4, 'manual', ?5, ?6)
             ON CONFLICT(block_id, url) DO UPDATE SET title = excluded.title, source = 'manual'",
            params![block, r.kind.as_str(), r.url, title, by_claude, now],
        )
        .map_err(|e| e.to_string())?;
        let link: i64 = c
            .query_row(
                "SELECT id FROM links WHERE block_id = ?1 AND url = ?2",
                params![block, r.url],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        // A PR link ties the PR to this block, so its status shows.
        let mut block = block;
        if let Some((repo, n)) = &r.pr {
            block = crate::group::attach_key(c, &crate::group::Key::pr(repo, *n), block)
                .and_then(|b| crate::group::link_keys(c, b))
                .map_err(|e| e.to_string())?;
        }
        if let Some(t) = &r.ticket {
            block = crate::group::attach_key(c, &crate::group::Key::Ticket(t.clone()), block)
                .map_err(|e| e.to_string())?;
        }
        let _ = crate::group::refresh_block(c, &config, block, now);
        (block, link)
    };
    app.changed([block]);
    Ok(json!({"ok": true, "block": block, "link": link}))
}

pub fn delete_link(app: &Shared, link: i64) -> Outcome {
    let block: i64 = {
        let db = app.db.lock().expect("db lock");
        let (block, source): (i64, String) = db
            .conn()
            .query_row(
                "SELECT block_id, source FROM links WHERE id = ?1",
                [link],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| "no such link".to_string())?;
        if source != "manual" {
            return Err("automatic links come back by themselves".into());
        }
        db.conn()
            .execute("DELETE FROM links WHERE id = ?1", [link])
            .map_err(|e| e.to_string())?;
        block
    };
    app.changed([block]);
    ok()
}

pub fn open_link(app: &Shared, link: i64) -> Outcome {
    let url: String = app
        .db
        .lock()
        .expect("db lock")
        .conn()
        .query_row("SELECT url FROM links WHERE id = ?1", [link], |r| r.get(0))
        .map_err(|_| "no such link".to_string())?;
    crate::open::open_link(&url, app.handlers.as_ref(), app.runner.as_ref())
        .map_err(|e| e.to_string())?;
    ok()
}

/// The path to open for a file: the original, or the saved copy when it's gone.
fn file_target(app: &Shared, file: i64) -> Result<(PathBuf, Option<i64>), String> {
    let (path, copy, block): (String, Option<String>, Option<i64>) = app
        .db
        .lock()
        .expect("db lock")
        .conn()
        .query_row(
            "SELECT f.path, f.copy_path, COALESCE(c.block_id, f.block_id)
             FROM files f LEFT JOIN chats c ON c.id = f.chat_id WHERE f.id = ?1",
            [file],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "no such file".to_string())?;
    let original = PathBuf::from(path);
    if std::fs::symlink_metadata(&original).is_ok() {
        return Ok((original, block));
    }
    match copy.map(PathBuf::from).filter(|c| c.is_file()) {
        Some(c) => Ok((c, block)),
        None => Err("the file is missing".into()),
    }
}

pub fn open_file(app: &Shared, file: i64) -> Outcome {
    let (path, _) = file_target(app, file)?;
    let config = app.config();
    crate::open::open_file(&path, &config, app.handlers.as_ref(), app.runner.as_ref())
        .map_err(|e| e.to_string())?;
    ok()
}

pub fn reveal_file(app: &Shared, file: i64) -> Outcome {
    let (path, _) = file_target(app, file)?;
    crate::open::reveal(&path, app.runner.as_ref()).map_err(|e| e.to_string())?;
    ok()
}

pub fn hide_file(app: &Shared, file: i64, hidden: bool) -> Outcome {
    let (path, block): (String, Option<i64>) = app
        .db
        .lock()
        .expect("db lock")
        .conn()
        .query_row(
            "SELECT f.path, COALESCE(c.block_id, f.block_id) FROM files f
             LEFT JOIN chats c ON c.id = f.chat_id WHERE f.id = ?1",
            [file],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "no such file".to_string())?;
    let block = block.ok_or("this file isn't in a block")?;
    {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        let _ = c.execute(
            "DELETE FROM overrides WHERE kind = 'file_hide' AND block_id = ?1 AND path = ?2",
            params![block, path],
        );
        if hidden {
            c.execute(
                "INSERT INTO overrides(kind, block_id, path, created_at) VALUES ('file_hide', ?1, ?2, ?3)",
                params![block, path, now_ms()],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    app.changed([block]);
    ok()
}

/// Adds a file by hand. The path comes from the native file picker (or the command
/// line), never from a page.
pub fn add_file_path(app: &Shared, block: i64, path: &Path) -> Outcome {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute() || crate::util::has_control_or_bidi(&path.to_string_lossy()) {
        return Err("not a usable path".into());
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| "the file is missing".to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.uid() != unsafe { libc::getuid() } {
        return Err("the file belongs to another user".into());
    }
    let kind = if crate::files::extension(path).as_deref() == Some("md") {
        "doc"
    } else {
        "output"
    };
    let now = now_ms();
    block_title(app, block)?;
    {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        c.execute(
            "INSERT INTO files(block_id, path, kind, found_by, size, mtime, first_seen)
             VALUES (?1, ?2, ?3, 'manual', ?4, ?5, ?6)
             ON CONFLICT(block_id, path) WHERE block_id IS NOT NULL DO NOTHING",
            params![
                block,
                path.to_string_lossy(),
                kind,
                meta.len() as i64,
                meta.mtime() * 1000,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        let _ = c.execute(
            "INSERT INTO overrides(kind, block_id, path, created_at) VALUES ('file_add', ?1, ?2, ?3)",
            params![block, path.to_string_lossy(), now],
        );
    }
    app.changed([block]);
    ok()
}

pub async fn add_file(app: &Shared, block: i64) -> Outcome {
    block_title(app, block)?;
    let a = app.clone();
    let picked = tokio::task::spawn_blocking(move || (a.pick_file)())
        .await
        .map_err(|e| e.to_string())?;
    match picked {
        Some(p) => add_file_path(app, block, &p),
        None => Ok(json!({"ok": true, "cancelled": true})),
    }
}

/// Shows macOS's "choose file" dialog through AppleScript and returns the chosen path.
pub fn choose_file_dialog() -> Option<PathBuf> {
    let out = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "POSIX path of (choose file with prompt \"Add a file to this block\")",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let p = PathBuf::from(s.trim_end_matches('\n'));
    p.is_absolute().then_some(p)
}

/// Moves the block's review marker. With `tree` (from the diff the page showed), the
/// marker records exactly that; otherwise a fresh review snapshot is taken.
pub async fn mark_reviewed(app: &Shared, block: i64, worktree: i64, tree: Option<&str>) -> Outcome {
    let (_, path) =
        super::views::worktree_path(app, block, Some(worktree)).ok_or("no such worktree here")?;
    let a = app.clone();
    let p = PathBuf::from(&path);
    let tree = tree.map(str::to_string);
    let snap = tokio::task::spawn_blocking(move || match tree {
        Some(t) => super::snapshots::record_tree(&a, &p, &t, "review"),
        None => super::snapshots::take(&a, &p, "review", &[]),
    })
    .await
    .map_err(|e| e.to_string())??;
    {
        let db = app.db.lock().expect("db lock");
        db.conn()
            .execute(
                "INSERT INTO review_markers(block_id, worktree_path, snapshot_id, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![block, path, snap, now_ms()],
            )
            .map_err(|e| e.to_string())?;
    }
    app.changed([block]);
    ok()
}

/// Opens a page in iTerm2 (a browser split for Changes, its own window for the board),
/// falling back to the default browser.
pub async fn open_page(
    app: &Shared,
    page: &str,
    block: Option<i64>,
    near: Option<String>,
) -> Outcome {
    let path = super::App::page_path(page, block).ok_or("unknown page")?;
    let use_iterm = app
        .bridge
        .lock()
        .expect("bridge lock")
        .as_ref()
        .is_some_and(|h| h.browser_panes());
    if use_iterm {
        let url = app.page_url(&path);
        match bridge::request(
            app,
            json!({"op": "open_page", "url": url, "kind": page, "near_session": near}),
        )
        .await
        {
            Ok(_) => return Ok(json!({"ok": true, "via": "iterm2"})),
            Err(BridgeError::Failed(e)) if e != "browser_unavailable" => return Err(e),
            Err(_) => {}
        }
    }
    open_in_browser(app, &app.page_url(&path))?;
    Ok(json!({"ok": true, "via": "browser"}))
}

/// Opens a one-time URL in the default browser without putting the code in process
/// arguments: a 0600 redirect file in the private run folder (design §20.2).
pub fn open_in_browser(app: &Shared, url: &str) -> Result<(), String> {
    let name = format!("open-{}.html", super::random_hex(8));
    let path = app.paths.run_dir().join(name);
    crate::paths::ensure_private_dir(&app.paths.run_dir()).map_err(|e| e.to_string())?;
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\
         <meta http-equiv=\"refresh\" content=\"0;url={}\"><title>bletherneuk</title>",
        html_escape(url)
    );
    crate::util::write_private_atomic(&path, html.as_bytes()).map_err(|e| e.to_string())?;
    let browser = app.handlers.browser().ok_or("no default browser")?;
    if crate::open::is_refused_handler(&browser) {
        return Err("the default browser looks like a terminal".into());
    }
    let result = app
        .runner
        .open(&[
            "-b".into(),
            browser,
            "--".into(),
            path.to_string_lossy().into_owned(),
        ])
        .map_err(|e| e.to_string());
    let p = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        let _ = std::fs::remove_file(p);
    });
    result
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The block for `blether link`: `--block`, else the chat running in this iTerm2 tab.
pub fn link_from_cli(
    app: &Shared,
    url: &str,
    title: Option<&str>,
    block: Option<i64>,
    iterm_session: Option<&str>,
    by_claude: bool,
) -> Outcome {
    let block = match block {
        Some(b) => b,
        None => block_for_session(app, iterm_session)?,
    };
    add_link(app, block, url, title, by_claude)
}

/// The block of the chat running in an iTerm2 session.
pub fn block_for_session(app: &Shared, iterm_session: Option<&str>) -> Result<i64, String> {
    let s = iterm_session.ok_or("not in an iTerm2 tab; use --block")?;
    let db = app.db.lock().expect("db lock");
    db.conn()
        .query_row(
            "SELECT block_id FROM chats WHERE iterm_session = ?1 AND block_id IS NOT NULL
             ORDER BY COALESCE(last_event_at, 0) DESC LIMIT 1",
            [s.to_ascii_uppercase()],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|_| "this tab's chat isn't in a block yet; use --block".to_string())
}

/// `blether doctor --live`: opens a browser pane in iTerm2, then closes it.
pub async fn live_check(app: &Shared) -> Outcome {
    let url = app.page_url("/");
    let near = app.focus.lock().expect("focus lock").session.clone();
    let reply = bridge::request(
        app,
        json!({"op": "open_page", "kind": "probe", "url": url, "near_session": near}),
    )
    .await
    .map_err(bridge_err)?;
    let session = reply
        .session
        .ok_or("iTerm2 didn't say which pane it opened")?;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    bridge::request(app, json!({"op": "close_session", "session": session}))
        .await
        .map_err(bridge_err)?;
    ok()
}

pub fn rescan(app: &Shared) -> Outcome {
    // The ingest worker does the reset itself, between batches, so a read in progress
    // can't save an old offset over it.
    app.ingest_tx
        .send(PathBuf::new())
        .map_err(|_| "the service is shutting down".to_string())?;
    ok()
}
