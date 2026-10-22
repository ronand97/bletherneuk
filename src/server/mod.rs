//! The local web server for the pages (design §20.2, protocol §2).

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::service::{Shared, UiEvent, actions, bridge, views};

#[derive(rust_embed::Embed)]
#[folder = "web/dist"]
#[allow_missing = true]
struct Assets;

pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
    connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

pub async fn run(app: Shared, listener: tokio::net::TcpListener) {
    if let Err(e) = axum::serve(listener, router(app)).await {
        tracing::error!(error = %e, "web server stopped");
    }
}

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/auth", get(auth))
        .route("/", get(page))
        .route("/panel", get(page))
        .route("/changes", get(page))
        .route("/assets/{*path}", get(asset))
        .route("/api/board", get(api_board))
        .route("/api/search", get(api_search))
        .route("/api/theme", get(api_theme))
        .route("/api/events", get(api_events))
        .route("/api/blocks/{id}", get(api_block))
        .route("/api/blocks/{id}/timeline", get(api_timeline))
        .route("/api/blocks/{id}/diff", get(api_diff))
        .route("/api/chats/{id}/open", post(chat_open))
        .route("/api/chats/{id}/move", post(chat_move))
        .route("/api/chats/{id}/keep-awake", post(chat_keep_awake))
        .route("/api/blocks/{id}/open", post(block_open))
        .route("/api/blocks/{id}/new-chat", post(block_new_chat))
        .route("/api/blocks/{id}/rename", post(block_rename))
        .route("/api/blocks/{id}/keep-awake", post(block_keep_awake))
        .route("/api/blocks/{id}/links", post(block_add_link))
        .route("/api/blocks/{id}/add-file", post(block_add_file))
        .route("/api/blocks/{id}/review", post(block_review))
        .route("/api/blocks/{id}/changes", post(block_changes))
        .route("/api/board/open", post(board_open))
        .route("/api/links/{id}/open", post(link_open))
        .route("/api/links/{id}/delete", post(link_delete))
        .route("/api/files/{id}/open", post(file_open))
        .route("/api/files/{id}/reveal", post(file_reveal))
        .route("/api/files/{id}/hide", post(file_hide))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

fn cookie_name(app: &Shared) -> String {
    format!("bn_{}", app.port())
}

/// Compares secrets without leaking where they differ through timing.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn has_session(app: &Shared, headers: &HeaderMap) -> bool {
    let name = cookie_name(app);
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .any(|(k, v)| k == name && same_secret(v.as_bytes(), app.secrets.session.as_bytes()))
}

fn is_page(path: &str) -> bool {
    matches!(path, "/" | "/panel" | "/changes" | "/auth")
}

fn deny(status: StatusCode, msg: &'static str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        msg,
    )
        .into_response()
}

/// Every request passes through here first (design §20.2).
async fn guard(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let headers = req.headers().clone();

    let want_host = format!("127.0.0.1:{}", app.port());
    let mut resp = 'check: {
        if headers.get(header::HOST).and_then(|h| h.to_str().ok()) != Some(want_host.as_str()) {
            break 'check deny(StatusCode::FORBIDDEN, "wrong host");
        }
        if method != Method::GET && method != Method::HEAD && method != Method::POST {
            break 'check deny(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        }
        // /auth is reached from the default browser through a local redirect file,
        // which browsers report as cross-site. Its single-use code is the credential.
        if path != "/auth"
            && matches!(
                headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()),
                Some("cross-site") | Some("same-site")
            )
        {
            break 'check deny(StatusCode::FORBIDDEN, "cross-site requests are refused");
        }
        let public = path.starts_with("/assets/") || path == "/auth";
        if !public && !has_session(&app, &headers) {
            if is_page(&path) {
                bridge::panel_needs_auth(&app);
                break 'check connecting();
            }
            break 'check (
                StatusCode::UNAUTHORIZED,
                Json(json!({"ok": false, "error": "not signed in"})),
            )
                .into_response();
        }
        if method == Method::POST {
            let origin_ok = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok())
                == Some(app.origin().as_str());
            let key_ok = headers
                .get("x-bletherneuk-key")
                .is_some_and(|k| same_secret(k.as_bytes(), app.secrets.write_key.as_bytes()));
            if !origin_ok || !key_ok {
                break 'check deny(StatusCode::FORBIDDEN, "refused");
            }
        }
        next.run(req).await
    };

    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    if !path.starts_with("/assets/") {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    h.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    resp
}

/// Shown when a page has no session yet. It reloads itself while the bridge
/// re-registers the panel (protocol §2.1).
fn connecting() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html(
            "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"3\">\
             <title>bletherneuk</title><p>Connecting to bletherneuk…</p>\
             <p>If this stays, run <code>blether board</code> in a terminal.</p>",
        ),
    )
        .into_response()
}

fn allowed_next(next: &str) -> Option<String> {
    if next == "/" || next == "/panel" {
        return Some(next.to_string());
    }
    let block = next.strip_prefix("/changes?block=")?;
    block
        .parse::<i64>()
        .ok()
        .map(|b| format!("/changes?block={b}"))
}

/// A same-origin hop to `next`, so the `SameSite=Strict` cookie is sent with it.
fn hop(next: &str) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"0;url={}\">\
         <title>bletherneuk</title>",
        actions::html_escape(next)
    )
}

#[derive(Deserialize)]
struct AuthQuery {
    code: Option<String>,
    next: Option<String>,
}

async fn auth(
    State(app): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<AuthQuery>,
) -> Response {
    if let Some(next) = q.code.as_deref().and_then(|c| app.redeem(c)) {
        let cookie = format!(
            "{}={}; HttpOnly; SameSite=Strict; Path=/",
            cookie_name(&app),
            app.secrets.session
        );
        return (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie)],
            Html(hop(&next)),
        )
            .into_response();
    }
    if has_session(&app, &headers) {
        let next = q
            .next
            .as_deref()
            .and_then(allowed_next)
            .unwrap_or_else(|| "/".into());
        return Html(hop(&next)).into_response();
    }
    bridge::panel_needs_auth(&app);
    connecting()
}

#[derive(Deserialize)]
struct PageQuery {
    block: Option<i64>,
}

async fn page(
    State(app): State<Shared>,
    req_path: axum::extract::OriginalUri,
    Query(q): Query<PageQuery>,
) -> Response {
    let name = match req_path.path() {
        "/" => "board",
        "/panel" => "panel",
        _ => {
            if q.block.is_none() {
                return deny(StatusCode::NOT_FOUND, "no block");
            }
            "changes"
        }
    };
    let Some(index) = Assets::get("index.html") else {
        return Html(
            "<!doctype html><meta charset=\"utf-8\"><title>bletherneuk</title>\
             <p>The pages haven't been built. Run <code>make build</code>.</p>",
        )
        .into_response();
    };
    let html = String::from_utf8_lossy(&index.data);
    let meta = format!(
        "<meta name=\"bn-page\" content=\"{name}\"><meta name=\"bn-key\" content=\"{}\">",
        app.secrets.write_key
    );
    let html = html.replacen("<head>", &format!("<head>{meta}"), 1);
    Html(html).into_response()
}

async fn asset(Path(path): Path<String>) -> Response {
    let full = format!("assets/{path}");
    let Some(file) = Assets::get(&full) else {
        return deny(StatusCode::NOT_FOUND, "not found");
    };
    let ctype = match path.rsplit('.').next() {
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("json") | Some("map") => "application/json",
        _ => "application/octet-stream",
    };
    (
        [
            (header::CONTENT_TYPE, ctype),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

async fn not_found() -> Response {
    deny(StatusCode::NOT_FOUND, "not found")
}

/// Runs a view on a blocking thread (they read the database and git).
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("view task")
}

fn json_or_404(v: Option<Value>) -> Response {
    match v {
        Some(v) => Json(v).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"ok": false, "error": "not found"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct BoardQuery {
    archived: Option<u8>,
}

async fn api_board(State(app): State<Shared>, Query(q): Query<BoardQuery>) -> Response {
    let archived = q.archived == Some(1);
    Json(blocking(move || views::board(&app, archived)).await).into_response()
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

async fn api_search(State(app): State<Shared>, Query(q): Query<SearchQuery>) -> Response {
    let text = q.q.unwrap_or_default();
    let text = crate::util::truncate_chars(&text, 200);
    Json(blocking(move || views::search(&app, &text)).await).into_response()
}

async fn api_theme(State(app): State<Shared>) -> Response {
    Json(json!({"vars": app.theme_vars()})).into_response()
}

async fn api_block(State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    json_or_404(blocking(move || views::block(&app, id)).await)
}

async fn api_timeline(State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    json_or_404(blocking(move || views::timeline(&app, id)).await)
}

#[derive(Deserialize)]
struct DiffQuery {
    worktree: Option<i64>,
    range: Option<String>,
    turn: Option<i64>,
}

async fn api_diff(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Query(q): Query<DiffQuery>,
) -> Response {
    let range = q.range.unwrap_or_else(|| "review".into());
    let range = match range.as_str() {
        "review" | "branch" | "turn" => range,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "unknown range"})),
            )
                .into_response();
        }
    };
    match blocking(move || views::diff(&app, id, q.worktree, &range, q.turn)).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({"ok": false, "error": e})),
        )
            .into_response(),
    }
}

async fn api_events(
    State(app): State<Shared>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let rx = app.events.subscribe();
    let focus = app.focus.lock().expect("focus lock").clone();
    let initial = [
        UiEvent::Bridge {
            connected: app.bridge_connected(),
        },
        UiEvent::Focus {
            block: focus.block,
            chat: focus.chat,
        },
    ];
    let to_event = |e: &UiEvent| Ok(Event::default().event(e.name()).data(e.data().to_string()));
    let first = futures_util::stream::iter(initial.iter().map(to_event).collect::<Vec<_>>());
    let rest = futures_util::stream::unfold(rx, move |mut rx| async move {
        use tokio::sync::broadcast::error::RecvError;
        let e = match rx.recv().await {
            Ok(e) => e,
            // Missed some: ask pages to refetch everything.
            Err(RecvError::Lagged(_)) => UiEvent::Changed {
                blocks: Vec::new(),
                board: true,
            },
            Err(RecvError::Closed) => return None,
        };
        Some((to_event(&e), rx))
    });
    use futures_util::StreamExt;
    Sse::new(first.chain(rest)).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}

fn outcome(r: actions::Outcome) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => Json(json!({"ok": false, "error": e})).into_response(),
    }
}

fn no_such_chat() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"ok": false, "error": "no such chat"})),
    )
        .into_response()
}

async fn chat_open(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Json(_): Json<Value>,
) -> Response {
    if !crate::ingest::is_uuid(&id) {
        return no_such_chat();
    }
    outcome(actions::open_chat(&app, &id).await)
}

#[derive(Deserialize)]
struct MoveBody {
    block: Option<i64>,
}

async fn chat_move(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<MoveBody>,
) -> Response {
    if !crate::ingest::is_uuid(&id) {
        return no_such_chat();
    }
    outcome(blocking(move || actions::move_chat(&app, &id, b.block)).await)
}

#[derive(Deserialize)]
struct OnBody {
    on: bool,
}

async fn chat_keep_awake(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<OnBody>,
) -> Response {
    if !crate::ingest::is_uuid(&id) {
        return no_such_chat();
    }
    outcome(blocking(move || actions::keep_awake_chat(&app, &id, b.on)).await)
}

async fn block_open(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> Response {
    outcome(actions::open_block(&app, id).await)
}

#[derive(Deserialize)]
struct NewChatBody {
    #[serde(rename = "where")]
    place: String,
    worktree: Option<i64>,
    name: Option<String>,
}

async fn block_new_chat(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(b): Json<NewChatBody>,
) -> Response {
    outcome(actions::new_chat(&app, id, &b.place, b.worktree, b.name.as_deref()).await)
}

#[derive(Deserialize)]
struct RenameBody {
    title: Option<String>,
}

async fn block_rename(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(b): Json<RenameBody>,
) -> Response {
    outcome(blocking(move || actions::rename_block(&app, id, b.title.as_deref())).await)
}

async fn block_keep_awake(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(b): Json<OnBody>,
) -> Response {
    outcome(blocking(move || actions::keep_awake_block(&app, id, b.on)).await)
}

#[derive(Deserialize)]
struct LinkBody {
    url: String,
    title: Option<String>,
}

async fn block_add_link(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(b): Json<LinkBody>,
) -> Response {
    outcome(blocking(move || actions::add_link(&app, id, &b.url, b.title.as_deref(), false)).await)
}

async fn block_add_file(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> Response {
    outcome(actions::add_file(&app, id).await)
}

#[derive(Deserialize)]
struct ReviewBody {
    worktree: i64,
}

async fn block_review(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(b): Json<ReviewBody>,
) -> Response {
    outcome(actions::mark_reviewed(&app, id, b.worktree).await)
}

async fn block_changes(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> Response {
    let near = app.focus.lock().expect("focus lock").session.clone();
    outcome(actions::open_page(&app, "changes", Some(id), near).await)
}

async fn board_open(State(app): State<Shared>, Json(_): Json<Value>) -> Response {
    outcome(actions::open_page(&app, "board", None, None).await)
}

async fn link_open(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> Response {
    outcome(blocking(move || actions::open_link(&app, id)).await)
}

async fn link_delete(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> Response {
    outcome(blocking(move || actions::delete_link(&app, id)).await)
}

async fn file_open(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> Response {
    outcome(blocking(move || actions::open_file(&app, id)).await)
}

async fn file_reveal(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> Response {
    outcome(blocking(move || actions::reveal_file(&app, id)).await)
}

#[derive(Deserialize)]
struct HideBody {
    hidden: bool,
}

async fn file_hide(
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(b): Json<HideBody>,
) -> Response {
    outcome(blocking(move || actions::hide_file(&app, id, b.hidden)).await)
}

#[cfg(test)]
mod tests;
