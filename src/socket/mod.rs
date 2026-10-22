//! The Unix socket the hook, the command line and the iTerm2 bridge use (protocol §1).
//!
//! The socket's folder is mode 0700 and the socket 0600, and every peer's user ID is
//! checked, so other accounts on the Mac can't talk to the service.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::{UnixListener, UnixStream};

use crate::paths::Paths;
use crate::service::{Shared, actions};

/// Longest accepted line.
pub const MAX_LINE: usize = 1024 * 1024;

/// Creates the socket, refusing to start if another service already answers on it.
pub fn bind(paths: &Paths) -> anyhow::Result<UnixListener> {
    let path = paths.socket();
    let dir = path.parent().expect("socket has a folder");
    crate::paths::ensure_private_dir(dir)?;
    if path.exists() {
        if StdStream::connect(&path).is_ok() {
            anyhow::bail!("another bletherneuk service is already running");
        }
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Reads one line of at most [`MAX_LINE`] bytes. `None` at end of stream.
pub async fn read_line<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return Ok(if line.is_empty() { None } else { Some(line) });
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                line.extend_from_slice(&buf[..i]);
                r.consume(i + 1);
                if line.len() > MAX_LINE {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "line too long",
                    ));
                }
                return Ok(Some(line));
            }
            None => {
                let n = buf.len();
                line.extend_from_slice(buf);
                r.consume(n);
                if line.len() > MAX_LINE {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "line too long",
                    ));
                }
            }
        }
    }
}

pub async fn accept_loop(app: Shared, listener: UnixListener) {
    let me = unsafe { libc::getuid() };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        match stream.peer_cred() {
            Ok(c) if c.uid() == me => {}
            _ => continue, // another account: drop it
        }
        tokio::spawn(handle(app.clone(), stream));
    }
}

async fn handle(app: Shared, stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut reader = TokioBufReader::new(read);
    let Ok(Some(line)) = read_line(&mut reader).await else {
        return;
    };
    let Ok(msg) = serde_json::from_slice::<Value>(&line) else {
        return;
    };
    match msg.get("type").and_then(Value::as_str) {
        Some("hook") => {
            let Some(event) = msg
                .get("event")
                .and_then(|e| serde_json::from_value::<crate::status::Event>(e.clone()).ok())
            else {
                return;
            };
            let session = msg
                .get("iterm_session")
                .and_then(Value::as_str)
                .filter(|s| crate::ingest::is_uuid(s))
                .map(str::to_ascii_uppercase);
            let time = msg
                .get("time")
                .and_then(Value::as_i64)
                .unwrap_or_else(crate::util::now_ms);
            crate::service::hooks::on_hook(&app, event, session, time);
        }
        Some("bridge_hello") => {
            crate::service::bridge::run(app, msg, reader, write).await;
        }
        _ => {
            let reply = one_shot(&app, &msg).await;
            let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
            let _ = write.flush().await;
        }
    }
}

fn reply(r: actions::Outcome) -> Value {
    match r {
        Ok(v) => v,
        Err(e) => json!({"ok": false, "error": e}),
    }
}

async fn one_shot(app: &Shared, msg: &Value) -> Value {
    let s = |k: &str| msg.get(k).and_then(Value::as_str);
    match s("type") {
        Some("page_url") => {
            let page = s("page").unwrap_or("board");
            let block = msg.get("block").and_then(Value::as_i64);
            match crate::service::App::page_path(page, block) {
                Some(path) => json!({"ok": true, "url": app.page_url(&path)}),
                None => json!({"ok": false, "error": "unknown page"}),
            }
        }
        Some("open_page") => {
            let near = s("near_session")
                .filter(|x| crate::ingest::is_uuid(x))
                .map(str::to_ascii_uppercase)
                .or_else(|| app.focus.lock().expect("focus lock").session.clone());
            reply(
                actions::open_page(
                    app,
                    s("page").unwrap_or("board"),
                    msg.get("block").and_then(Value::as_i64),
                    near,
                )
                .await,
            )
        }
        Some("link") => {
            let Some(url) = s("url") else {
                return json!({"ok": false, "error": "no url"});
            };
            let session = s("iterm_session").and_then(crate::hook::iterm_session_uuid);
            reply(actions::link_from_cli(
                app,
                url,
                s("title"),
                msg.get("block").and_then(Value::as_i64),
                session.as_deref(),
                msg.get("by_claude")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ))
        }
        Some("add_file") => {
            let Some(path) = s("path") else {
                return json!({"ok": false, "error": "no path"});
            };
            let session = s("iterm_session").and_then(crate::hook::iterm_session_uuid);
            let block = match msg.get("block").and_then(Value::as_i64) {
                Some(b) => Ok(b),
                None => actions::block_for_session(app, session.as_deref()),
            };
            match block {
                Ok(b) => reply(actions::add_file_path(app, b, Path::new(path))),
                Err(e) => json!({"ok": false, "error": e}),
            }
        }
        Some("live_check") => reply(actions::live_check(app).await),
        Some("rescan") => reply(actions::rescan(app)),
        Some("doctor") => {
            json!({"ok": true, "report": crate::install::doctor::service_report(app)})
        }
        Some("ping") => json!({"ok": true}),
        _ => json!({"ok": false, "error": "unknown request"}),
    }
}

/// Sends one request and waits for its reply (for the command line).
pub fn request(paths: &Paths, msg: &Value, timeout: Duration) -> anyhow::Result<Value> {
    let mut s = StdStream::connect(paths.socket()).map_err(|_| {
        anyhow::anyhow!("the bletherneuk service isn't running (try `blether doctor`)")
    })?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.write_all(format!("{msg}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim_end())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_lines_and_rejects_huge_ones() {
        let data = b"one\ntwo\n".to_vec();
        let mut r = TokioBufReader::new(&data[..]);
        assert_eq!(read_line(&mut r).await.unwrap(), Some(b"one".to_vec()));
        assert_eq!(read_line(&mut r).await.unwrap(), Some(b"two".to_vec()));
        assert_eq!(read_line(&mut r).await.unwrap(), None);

        let big = vec![b'x'; MAX_LINE + 10];
        let mut r = TokioBufReader::new(&big[..]);
        assert!(read_line(&mut r).await.is_err());
    }
}
