//! Claude Code hook events: live status, turns and the snapshots around them (design §12, §13).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};

use super::Shared;
use super::snapshots::{Edge, Job};
use crate::status::{self, Event, State, Status, TurnEdge};

pub fn on_hook(app: &Shared, ev: Event, iterm_session: Option<String>, time: i64) {
    if !crate::ingest::is_uuid(&ev.session_id) {
        return;
    }
    let chat = ev.session_id.clone();
    let config = app.config();

    // A fresh transcript line is waiting; read it soon.
    if let Some(t) = &ev.transcript_path {
        let p = PathBuf::from(t);
        if p.starts_with(app.paths.claude_projects()) {
            let _ = app.ingest_tx.send(p);
        }
    }

    let worktree = ev
        .cwd
        .as_deref()
        .and_then(|c| {
            app.locator
                .lock()
                .expect("locator lock")
                .locate(Path::new(c))
        })
        .map(|l| l.worktree);

    let mut jobs = Vec::new();
    let mut touched: BTreeSet<i64> = BTreeSet::new();
    // A turn that ended: (turn, its worktree, files it wrote). Snapshots are planned
    // after the database lock is released, since finding worktrees reads git.
    let mut ending: Option<(i64, Option<PathBuf>, BTreeSet<PathBuf>)> = None;
    {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        let _ = c.execute("INSERT OR IGNORE INTO chats(id) VALUES (?1)", [&chat]);
        let _ = c.execute(
            "UPDATE chats SET last_event_at = ?2,
                              iterm_session = COALESCE(?3, iterm_session),
                              last_activity = MAX(COALESCE(last_activity, 0), ?2)
             WHERE id = ?1",
            params![chat, time, iterm_session],
        );

        // Status.
        let prev: Option<State> = c
            .query_row(
                "SELECT status, background, status_since FROM chats WHERE id = ?1",
                [&chat],
                |r| {
                    let status: String = r.get(0)?;
                    let background: bool = r.get(1)?;
                    let since: Option<i64> = r.get(2)?;
                    Ok(since.map(|_| State {
                        status: Status::parse(&status),
                        background,
                    }))
                },
            )
            .ok()
            .flatten();
        if let Some(next) = status::transition(prev, &ev) {
            let _ = c.execute(
                "UPDATE chats SET status = ?2, background = ?3, status_since = ?4 WHERE id = ?1",
                params![chat, next.status.as_str(), next.background, time],
            );
        }

        // A chat started from a block's panel joins that block (design §9).
        if let Some(s) = &iterm_session {
            let tagged = app
                .sessions
                .lock()
                .expect("sessions lock")
                .get(s)
                .and_then(|s| s.block);
            if let Some(block) = tagged {
                let placed: bool = c
                    .query_row(
                        "SELECT 1 FROM overrides WHERE chat_id = ?1
                         AND kind IN ('chat_move', 'chat_started_from')",
                        [&chat],
                        |_| Ok(true),
                    )
                    .optional()
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                if !placed
                    && let Ok(t) = crate::group::assign_started_from(c, &config, &chat, block, time)
                {
                    touched.extend(t);
                }
            }
        }

        // Turns.
        let open_turn: Option<(i64, Option<String>)> = c
            .query_row(
                "SELECT id, worktree_path FROM turns WHERE chat_id = ?1 AND ended_at IS NULL
                 ORDER BY started_at DESC LIMIT 1",
                [&chat],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        if let (Some(p), Some((turn, _))) = (&ev.file_path, &open_turn) {
            let path = crate::files::resolve(p, ev.cwd.as_deref().map(Path::new), &app.paths.home);
            if let Some(path) = path {
                app.turn_writes
                    .lock()
                    .expect("turn writes lock")
                    .entry(*turn)
                    .or_default()
                    .insert(path);
            }
        }
        match status::turn_edge(&ev) {
            TurnEdge::Start => {
                if let Some((old, _)) = open_turn {
                    let _ = c.execute(
                        "UPDATE turns SET ended_at = ?2 WHERE id = ?1",
                        params![old, time],
                    );
                    app.turn_writes
                        .lock()
                        .expect("turn writes lock")
                        .remove(&old);
                }
                let wt = worktree.as_ref().map(|w| w.to_string_lossy().into_owned());
                if c.execute(
                    "INSERT INTO turns(chat_id, prompt, started_at, worktree_path) VALUES (?1, ?2, ?3, ?4)",
                    params![chat, ev.prompt, time, wt],
                )
                .is_ok()
                {
                    let turn = c.last_insert_rowid();
                    if let Some(w) = &worktree {
                        jobs.push(Job {
                            worktree: w.clone(),
                            reason: "turn_start",
                            turn: Some((turn, Edge::Start)),
                            extra: Vec::new(),
                        });
                    }
                }
            }
            TurnEdge::End => {
                if let Some((turn, wt)) = open_turn {
                    let _ = c.execute(
                        "UPDATE turns SET ended_at = ?2 WHERE id = ?1",
                        params![turn, time],
                    );
                    let written = app
                        .turn_writes
                        .lock()
                        .expect("turn writes lock")
                        .remove(&turn)
                        .unwrap_or_default();
                    let main = wt.map(PathBuf::from).or_else(|| worktree.clone());
                    ending = Some((turn, main, written));
                }
            }
            TurnEdge::None => {}
        }
        let block: Option<i64> = c
            .query_row("SELECT block_id FROM chats WHERE id = ?1", [&chat], |r| {
                r.get(0)
            })
            .ok()
            .flatten();
        touched.extend(block);
    }

    if let Some((turn, main, written)) = ending {
        // Other worktrees this turn wrote into get a snapshot too.
        let mut others: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for p in &written {
            if let Some(loc) = app.locator.lock().expect("locator lock").locate(p) {
                others.entry(loc.worktree).or_default().push(p.clone());
            }
        }
        if let Some(m) = &main {
            let extra = others.remove(m).unwrap_or_default();
            jobs.push(Job {
                worktree: m.clone(),
                reason: "turn_end",
                turn: Some((turn, Edge::End)),
                extra,
            });
        }
        for (w, extra) in others {
            jobs.push(Job {
                worktree: w,
                reason: "turn_end",
                turn: None,
                extra,
            });
        }
    }
    for job in jobs {
        let _ = app.snap_tx.send(job);
    }
    super::tasks::tick_keep_awake(app, time);
    app.changed(touched);
}

/// You interrupted a chat with Esc. Claude Code sends no `Stop` then, so the transcript's
/// "[Request interrupted by user]" line ends the turn, unless a hook event has arrived
/// since (a new prompt, say).
pub fn on_interrupt(app: &super::App, chat: &str, at: i64) {
    let mut job = None;
    {
        let db = app.db.lock().expect("db lock");
        let c = db.conn();
        let quiet: bool = c
            .query_row(
                "SELECT COALESCE(last_event_at, 0) <= ?2 FROM chats WHERE id = ?1",
                params![chat, at],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !quiet {
            return;
        }
        let _ = c.execute(
            "UPDATE chats SET status = 'done', background = 0, status_since = ?2
             WHERE id = ?1 AND status = 'working'",
            params![chat, at],
        );
        let open: Option<(i64, Option<String>)> = c
            .query_row(
                "SELECT id, worktree_path FROM turns
                 WHERE chat_id = ?1 AND ended_at IS NULL AND started_at <= ?2
                 ORDER BY started_at DESC LIMIT 1",
                params![chat, at],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        if let Some((turn, wt)) = open {
            let _ = c.execute(
                "UPDATE turns SET ended_at = ?2 WHERE id = ?1",
                params![turn, at],
            );
            let written = app
                .turn_writes
                .lock()
                .expect("turn writes lock")
                .remove(&turn)
                .unwrap_or_default();
            if let Some(w) = wt {
                job = Some(Job {
                    worktree: PathBuf::from(w),
                    reason: "turn_end",
                    turn: Some((turn, Edge::End)),
                    extra: written.into_iter().collect(),
                });
            }
        }
    }
    if let Some(j) = job {
        let _ = app.snap_tx.send(j);
    }
}
