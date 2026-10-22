//! `blether hook`: the Claude Code hook entry point (design §12 and §20.7).
//!
//! It reads the event JSON from stdin (at most 1 MB), keeps only the fields bletherneuk
//! uses, adds the iTerm2 session and the time, and sends one line to the service's
//! socket with a hard 50 ms limit. It never writes to stdout or stderr, and always
//! exits 0: Claude Code adds some hooks' stdout to the chat, and exit code 2 blocks.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::status::Event;

pub const MAX_STDIN: u64 = 1024 * 1024;
pub const DEADLINE: Duration = Duration::from_millis(50);

/// The UUID part of `ITERM_SESSION_ID` (`w0t1p0:<UUID>`), which matches the iTerm2
/// API's session ID.
pub fn iterm_session_uuid(var: &str) -> Option<String> {
    let id = var.rsplit(':').next()?;
    crate::ingest::is_uuid(id).then(|| id.to_ascii_uppercase())
}

/// Builds the line sent to the service, or `None` if the input isn't a hook event.
pub fn message(stdin: &[u8], iterm_session_id: Option<&str>, now_ms: i64) -> Option<String> {
    let v: Value = serde_json::from_slice(stdin).ok()?;
    let event = Event::from_payload(&v)?;
    let msg = json!({
        "type": "hook",
        "event": event,
        "iterm_session": iterm_session_id.and_then(iterm_session_uuid),
        "time": now_ms,
    });
    Some(format!("{msg}\n"))
}

/// Sends `line` to the socket, giving up at `deadline`. The work runs on a thread so a
/// stuck connect can't hold the hook past the deadline; the process exits anyway.
pub fn send(socket: &Path, line: String, deadline: Duration) -> bool {
    let socket = socket.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ok = (|| -> std::io::Result<()> {
            let mut s = UnixStream::connect(&socket)?;
            s.set_write_timeout(Some(deadline))?;
            s.write_all(line.as_bytes())?;
            s.flush()
        })()
        .is_ok();
        let _ = tx.send(ok);
    });
    rx.recv_timeout(deadline).unwrap_or(false)
}

/// The whole hook. Every failure is swallowed.
pub fn run() {
    // A panic must not print to stderr either.
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(|| {
        let mut buf = Vec::new();
        let _ = std::io::stdin().take(MAX_STDIN).read_to_end(&mut buf);
        let session = std::env::var("ITERM_SESSION_ID").ok();
        let Some(line) = message(&buf, session.as_deref(), crate::util::now_ms()) else {
            return;
        };
        let Ok(paths) = crate::paths::Paths::current() else {
            return;
        };
        send(&paths.socket(), line, DEADLINE);
    });
    let _ = result;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    const UUID: &str = "6F3A1E2B-1C2D-4E5F-8A9B-0C1D2E3F4A5B";

    #[test]
    fn iterm_session_ids() {
        assert_eq!(
            iterm_session_uuid(&format!("w0t1p2:{UUID}")).as_deref(),
            Some(UUID)
        );
        assert_eq!(
            iterm_session_uuid(&format!("w0t1p2:{}", UUID.to_lowercase())).as_deref(),
            Some(UUID)
        );
        assert_eq!(iterm_session_uuid("w0t1p2:not-a-uuid"), None);
    }

    #[test]
    fn builds_a_reduced_message() {
        let input = br#"{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"hi","extra":"x"}"#;
        let line = message(input, Some(&format!("w0t0p0:{UUID}")), 5).unwrap();
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["type"], "hook");
        assert_eq!(v["event"]["name"], "UserPromptSubmit");
        assert_eq!(v["event"]["prompt"], "hi");
        assert_eq!(v["iterm_session"], UUID);
        assert_eq!(v["time"], 5);
        assert!(!line.contains("extra"));
        assert!(message(b"not json", None, 0).is_none());
        assert!(message(br#"{"session_id":"s"}"#, None, 0).is_none());
    }

    #[test]
    fn delivers_to_a_listening_socket() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let t = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(s).read_line(&mut line).unwrap();
            line
        });
        assert!(send(
            &sock,
            "{\"a\":1}\n".into(),
            Duration::from_millis(500)
        ));
        assert_eq!(t.join().unwrap(), "{\"a\":1}\n");
    }

    #[test]
    fn gives_up_quickly_when_the_service_is_down() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        assert!(!send(
            &dir.path().join("missing.sock"),
            "x\n".into(),
            DEADLINE
        ));
        assert!(start.elapsed() < Duration::from_millis(200));
    }
}
