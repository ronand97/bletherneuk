//! The iTerm2 bridge's connection: requests to it, and events from it (protocol §1.2).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use super::{Focus, Session, Shared, UiEvent};
use crate::status;
use crate::theme::ItermColors;

/// Browser panes need at least this iTerm2 version (design §5).
pub const MIN_ITERM: (u32, u32, u32) = (3, 7, 6);

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct Reply {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BridgeError {
    #[error("iTerm2 not connected")]
    NotConnected,
    #[error("iTerm2 didn't answer")]
    Timeout,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    next_id: Arc<AtomicU64>,
    pub iterm_version: String,
    conn: u64,
}

pub fn version_at_least(v: &str, min: (u32, u32, u32)) -> bool {
    let mut parts = v
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(0));
    let got = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    got >= min
}

impl Handle {
    pub fn browser_panes(&self) -> bool {
        version_at_least(&self.iterm_version, MIN_ITERM)
    }
}

/// Sends a request to the bridge and waits for its reply.
pub async fn request(app: &Shared, mut op: Value) -> Result<Reply, BridgeError> {
    let handle = app
        .bridge
        .lock()
        .expect("bridge lock")
        .clone()
        .ok_or(BridgeError::NotConnected)?;
    let id = handle.next_id.fetch_add(1, Ordering::Relaxed);
    op["type"] = json!("request");
    op["id"] = json!(id);
    let (tx, rx) = oneshot::channel();
    handle.pending.lock().expect("pending lock").insert(id, tx);
    if handle.tx.send(format!("{op}\n")).is_err() {
        handle.pending.lock().expect("pending lock").remove(&id);
        return Err(BridgeError::NotConnected);
    }
    match tokio::time::timeout(Duration::from_secs(15), rx).await {
        Ok(Ok(reply)) if reply.ok => Ok(reply),
        Ok(Ok(reply)) => Err(BridgeError::Failed(
            reply.error.unwrap_or_else(|| "iTerm2 refused".into()),
        )),
        Ok(Err(_)) => Err(BridgeError::NotConnected),
        Err(_) => {
            handle.pending.lock().expect("pending lock").remove(&id);
            Err(BridgeError::Timeout)
        }
    }
}

/// Runs one bridge connection until it closes.
pub async fn run<R, W>(app: Shared, hello: Value, mut reader: R, mut writer: W)
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let handle = Handle {
        tx,
        pending: Arc::new(Mutex::new(HashMap::new())),
        next_id: Arc::new(AtomicU64::new(1)),
        iterm_version: hello
            .get("iterm_version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        conn,
    };
    let pending = handle.pending.clone();
    *app.bridge.lock().expect("bridge lock") = Some(handle);

    let writer_task = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let _ = writer.flush().await;
        }
    });

    apply_hello(&app, &hello);
    app.emit(UiEvent::Bridge { connected: true });
    register_panel(&app);

    while let Ok(Some(line)) = crate::socket::read_line(&mut reader).await {
        let Ok(msg) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if msg.get("type").and_then(Value::as_str) == Some("reply") {
            if let Some(id) = msg.get("id").and_then(Value::as_u64)
                && let Some(tx) = pending.lock().expect("pending lock").remove(&id)
            {
                let reply: Reply = serde_json::from_value(msg).unwrap_or_default();
                let _ = tx.send(reply);
            }
            continue;
        }
        on_event(&app, &msg);
    }

    let mut current = app.bridge.lock().expect("bridge lock");
    if current.as_ref().is_some_and(|h| h.conn == conn) {
        *current = None;
        drop(current);
        app.sessions.lock().expect("sessions lock").clear();
        app.emit(UiEvent::Bridge { connected: false });
    }
    pending.lock().expect("pending lock").clear();
    writer_task.abort();
}

/// Asks the bridge to (re)register the toolbelt panel with a fresh one-time URL.
pub fn register_panel(app: &Shared) {
    *app.panel_refresh.lock().expect("panel lock") = Some(std::time::Instant::now());
    let url = app.page_url("/panel");
    let app = app.clone();
    tokio::spawn(async move {
        let _ = request(&app, json!({"op": "panel", "url": url})).await;
    });
}

/// Re-registers the panel when one couldn't sign in, at most once every 10 seconds.
pub fn panel_needs_auth(app: &Shared) {
    if !app.bridge_connected() {
        return;
    }
    let recent = app
        .panel_refresh
        .lock()
        .expect("panel lock")
        .is_some_and(|t| t.elapsed() < Duration::from_secs(10));
    if !recent {
        register_panel(app);
    }
}

fn opaque_id(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?;
    (!s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_graphic()))
        .then(|| s.to_string())
}

fn session_uuid(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?;
    crate::ingest::is_uuid(s).then(|| s.to_ascii_uppercase())
}

pub(crate) fn apply_hello(app: &Shared, hello: &Value) {
    if let Some(c) = hello.get("colors") {
        set_colors(app, c);
    }
    let mut sessions = app.sessions.lock().expect("sessions lock");
    sessions.clear();
    let mut chat_links = Vec::new();
    if let Some(list) = hello.get("sessions").and_then(Value::as_array) {
        for s in list {
            let Some(id) = session_uuid(s.get("session")) else {
                continue;
            };
            // The block reported here is the window's: a hint for focus, never a reason
            // to file a chat under it.
            sessions.insert(
                id.clone(),
                Session {
                    window: opaque_id(s.get("window")),
                    tab: opaque_id(s.get("tab")),
                    block: None,
                    window_block: s.get("block").and_then(Value::as_i64),
                },
            );
            if let Some(chat) = s.get("chat").and_then(Value::as_str)
                && crate::ingest::is_uuid(chat)
            {
                chat_links.push((chat.to_string(), id));
            }
        }
    }
    drop(sessions);
    if !chat_links.is_empty() {
        let db = app.db.lock().expect("db lock");
        for (chat, session) in chat_links {
            let _ = db.conn().execute(
                "UPDATE chats SET iterm_session = ?2 WHERE id = ?1 AND iterm_session IS NULL",
                rusqlite::params![chat, session],
            );
        }
    }
    if let Some(f) = hello.get("focus") {
        set_focus(
            app,
            session_uuid(f.get("session")),
            opaque_id(f.get("window")),
        );
    }
}

fn set_colors(app: &Shared, v: &Value) {
    if let Ok(c) = serde_json::from_value::<ItermColors>(v.clone())
        && c.is_valid()
    {
        *app.iterm_colors.write().expect("colors lock") = Some(c);
        app.emit(UiEvent::Theme(app.theme_vars()));
    }
}

pub fn on_event(app: &Shared, msg: &Value) {
    match msg.get("type").and_then(Value::as_str) {
        Some("focus") => set_focus(
            app,
            session_uuid(msg.get("session")),
            opaque_id(msg.get("window")),
        ),
        Some("session_opened") => {
            if let Some(id) = session_uuid(msg.get("session")) {
                app.sessions.lock().expect("sessions lock").insert(
                    id,
                    Session {
                        window: opaque_id(msg.get("window")),
                        tab: opaque_id(msg.get("tab")),
                        block: msg.get("block").and_then(Value::as_i64),
                        window_block: msg.get("block").and_then(Value::as_i64),
                    },
                );
                app.changed([]);
            }
        }
        Some("session_closed") => {
            if let Some(id) = session_uuid(msg.get("session")) {
                app.sessions.lock().expect("sessions lock").remove(&id);
                app.changed([]);
            }
        }
        Some("window_closed") => {
            let (Some(window), Some(block)) = (
                opaque_id(msg.get("window")),
                msg.get("block").and_then(Value::as_i64),
            ) else {
                return;
            };
            let chats: Vec<String> = msg
                .get("chats")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .filter(|c| crate::ingest::is_uuid(c))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let db = app.db.lock().expect("db lock");
            let _ = db.conn().execute(
                "INSERT INTO bridge_windows(window_id, block_id, chats) VALUES (?1, ?2, ?3)
                 ON CONFLICT(window_id) DO UPDATE SET block_id = excluded.block_id, chats = excluded.chats",
                rusqlite::params![window, block, serde_json::to_string(&chats).unwrap_or_default()],
            );
        }
        Some("colors") => {
            if let Some(c) = msg.get("colors") {
                set_colors(app, c);
            }
        }
        Some("panel_failed") => tracing::warn!("the iTerm2 toolbelt panel could not be registered"),
        _ => {}
    }
}

/// Works out the focused tab's chat and block, and tells the pages.
pub fn set_focus(app: &Shared, session: Option<String>, window: Option<String>) {
    let tagged = session
        .as_ref()
        .and_then(|s| app.sessions.lock().expect("sessions lock").get(s).cloned());
    let (chat, block, became_idle) = {
        let db = app.db.lock().expect("db lock");
        let row: Option<(String, Option<i64>, String, bool)> = session.as_ref().and_then(|s| {
            db.conn()
                .query_row(
                    "SELECT id, block_id, status, background FROM chats WHERE iterm_session = ?1
                     ORDER BY COALESCE(last_event_at, last_activity, 0) DESC LIMIT 1",
                    [s],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .ok()
        });
        let mut became_idle = None;
        if let Some((chat, _, st, bg)) = &row {
            let prev = status::State {
                status: status::Status::parse(st),
                background: *bg,
            };
            if let Some(next) = status::on_focus(prev) {
                let _ = db.conn().execute(
                    "UPDATE chats SET status = ?2, status_since = ?3 WHERE id = ?1",
                    rusqlite::params![chat, next.status.as_str(), crate::util::now_ms()],
                );
                became_idle = Some(chat.clone());
            }
        }
        let chat = row.as_ref().map(|r| r.0.clone());
        let block = row
            .and_then(|r| r.1)
            .or_else(|| tagged.as_ref().and_then(|t| t.block.or(t.window_block)));
        (chat, block, became_idle)
    };
    let focus = Focus {
        session,
        window,
        block,
        chat: chat.clone(),
    };
    let changed = {
        let mut f = app.focus.lock().expect("focus lock");
        let changed = (f.block, &f.chat) != (focus.block, &focus.chat);
        *f = focus;
        changed
    };
    if became_idle.is_some() {
        app.changed(block);
    }
    if changed {
        app.emit(UiEvent::Focus { block, chat });
        if let Some(b) = block {
            super::tasks::on_block_focus(app.clone(), b);
        }
    }
}
