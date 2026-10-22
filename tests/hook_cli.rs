//! Design §20.11: the hook prints nothing and exits 0 in every case, including when
//! the service is down.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run_hook(home: &std::path::Path, stdin: &[u8]) -> (std::process::Output, Duration) {
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_blether"))
        .arg("hook")
        .env("BLETHERNEUK_HOME", home)
        .env("ITERM_SESSION_ID", "w0t0p0:6F3A1E2B-1C2D-4E5F-8A9B-0C1D2E3F4A5B")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A large input may stop being read at 1 MB; that's fine.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    (out, start.elapsed())
}

#[test]
fn silent_and_successful_whatever_happens() {
    let dir = tempfile::tempdir().unwrap();
    let event = br#"{"hook_event_name":"UserPromptSubmit","session_id":"22222222-2222-4222-8222-222222222222","prompt":"hi"}"#;
    let big = vec![b'x'; 3 * 1024 * 1024];
    for input in [
        &event[..],
        b"",
        b"not json",
        b"{\"hook_event_name\": 5}",
        &big[..],
    ] {
        let (out, took) = run_hook(dir.path(), input);
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.is_empty(), "stdout: {:?}", String::from_utf8_lossy(&out.stdout));
        assert!(out.stderr.is_empty(), "stderr: {:?}", String::from_utf8_lossy(&out.stderr));
        assert!(took < Duration::from_secs(2), "took {took:?}");
    }
}
