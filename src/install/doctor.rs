//! `blether doctor`: checks the setup and reports what the service sees.

use std::io::Write;
use std::time::Duration;

use serde_json::{Value, json};

use crate::paths::{LAUNCHD_LABEL, Paths};
use crate::service::{App, bridge};

/// What the running service reports about itself (socket request `doctor`).
pub fn service_report(app: &App) -> Value {
    let db = app.db.lock().expect("db lock");
    let c = db.conn();
    let count = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let meta = |k: &str| {
        db.meta(k)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let bridge = app.bridge.lock().expect("bridge lock").clone();
    let holding = app.awake.lock().expect("awake lock").is_holding();
    let covered = app.awake.lock().expect("awake lock").covered_chats().len();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "port": app.port(),
        "started_at": app.started_at,
        "chats": count("SELECT COUNT(*) FROM chats"),
        "hidden_chats": count("SELECT COUNT(*) FROM chats WHERE hidden = 1"),
        "blocks": count("SELECT COUNT(*) FROM blocks"),
        "unsorted": count("SELECT COUNT(*) FROM chats WHERE block_id IS NULL AND hidden = 0"),
        "snapshots": count("SELECT COUNT(*) FROM snapshots WHERE tree_id IS NOT NULL"),
        "skipped_lines": {"malformed": meta("ingest.malformed"), "oversized": meta("ingest.oversized")},
        "warnings": app.warnings.read().expect("warnings lock").clone(),
        "bridge": bridge.as_ref().map(|b| json!({
            "connected": true,
            "iterm_version": b.iterm_version,
            "browser_panes": b.browser_panes(),
        })).unwrap_or(json!({"connected": false})),
        "keep_awake": {"holding": holding, "chats": covered},
        "gh": app.gh.as_ref().map(|p| p.to_string_lossy().into_owned()),
    })
}

struct Report<'a> {
    out: &'a mut dyn Write,
    problems: u32,
}

impl Report<'_> {
    fn ok(&mut self, msg: &str) {
        let _ = writeln!(self.out, "  ok    {msg}");
    }
    fn warn(&mut self, msg: &str) {
        let _ = writeln!(self.out, "  warn  {msg}");
    }
    fn bad(&mut self, msg: &str) {
        self.problems += 1;
        let _ = writeln!(self.out, "  FAIL  {msg}");
    }
}

fn command_ok(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Runs the checks. Returns the number of problems found.
pub fn run(paths: &Paths, live: bool, out: &mut dyn Write) -> u32 {
    let mut r = Report { out, problems: 0 };
    let _ = writeln!(r.out, "bletherneuk {}", env!("CARGO_PKG_VERSION"));

    let _ = writeln!(r.out, "\nInstall");
    let binary = paths.installed_binary();
    if binary.is_file() {
        r.ok(&format!("binary at {}", binary.display()));
    } else {
        r.bad(&format!(
            "no binary at {} (run `make install`)",
            binary.display()
        ));
    }
    let command = super::hook_command(&binary);
    match std::fs::read_to_string(paths.claude_settings())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        Some(v) => {
            let missing: Vec<&str> = super::HOOK_EVENTS
                .iter()
                .copied()
                .filter(|ev| {
                    !v.pointer(&format!("/hooks/{ev}"))
                        .and_then(Value::as_array)
                        .is_some_and(|l| {
                            l.iter().any(|e| {
                                e.get("hooks").and_then(Value::as_array).is_some_and(|hs| {
                                    hs.iter().any(|h| {
                                        h.get("command").and_then(Value::as_str)
                                            == Some(command.as_str())
                                    })
                                })
                            })
                        })
                })
                .collect();
            if missing.is_empty() {
                r.ok("Claude Code hooks registered");
            } else {
                r.bad(&format!(
                    "hooks missing for: {} (run `blether install`)",
                    missing.join(", ")
                ));
            }
        }
        None => r.bad("couldn't read ~/.claude/settings.json"),
    }
    let bridge_file = paths.iterm_autolaunch().join(super::BRIDGE_FILE);
    match std::fs::read_to_string(&bridge_file) {
        Ok(t) if t == super::BRIDGE_SCRIPT => r.ok("iTerm2 bridge installed"),
        Ok(_) => r.warn("the iTerm2 bridge is out of date (run `blether install`)"),
        Err(_) => r.bad(&format!("no iTerm2 bridge at {}", bridge_file.display())),
    }
    if paths.launch_agent().is_file() {
        let uid = unsafe { libc::getuid() };
        if command_ok(
            "/bin/launchctl",
            &["print", &format!("gui/{uid}/{LAUNCHD_LABEL}")],
        )
        .is_some()
        {
            r.ok("background service loaded in launchd");
        } else {
            r.bad("the launchd agent isn't loaded (run `blether install`)");
        }
    } else {
        r.bad("no launchd agent (run `blether install`)");
    }

    let _ = writeln!(r.out, "\niTerm2");
    let version = command_ok(
        "/usr/bin/defaults",
        &[
            "read",
            "/Applications/iTerm.app/Contents/Info",
            "CFBundleShortVersionString",
        ],
    );
    match &version {
        Some(v) if bridge::version_at_least(v, bridge::MIN_ITERM) => r.ok(&format!("iTerm2 {v}")),
        Some(v) => r.warn(&format!(
            "iTerm2 {v} is older than 3.7.6: browser panes may not work, so pages open in your default browser"
        )),
        None => r.bad("iTerm2 not found in /Applications"),
    }
    match command_ok(
        "/usr/bin/defaults",
        &["read", "com.googlecode.iterm2", "EnableAPIServer"],
    )
    .as_deref()
    {
        Some("1") => r.ok("Python API enabled"),
        _ => r.bad("turn on Settings → General → Magic → Enable Python API in iTerm2"),
    }
    let runtime = paths.home.join("Library/Application Support/iTerm2");
    let has_runtime = std::fs::read_dir(&runtime)
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("iterm2env"))
        })
        .unwrap_or(false);
    if has_runtime {
        r.ok("iTerm2 Python runtime installed");
    } else {
        r.warn("iTerm2's Python runtime isn't installed yet; iTerm2 offers to download it when the bridge first starts");
    }

    let _ = writeln!(r.out, "\nService");
    let report = crate::socket::request(paths, &json!({"type": "doctor"}), Duration::from_secs(5))
        .ok()
        .and_then(|v| v.get("report").cloned());
    match &report {
        Some(rep) => {
            r.ok(&format!(
                "running (pid {}, {} chats in {} blocks, {} unsorted)",
                rep["pid"], rep["chats"], rep["blocks"], rep["unsorted"]
            ));
            let skipped = &rep["skipped_lines"];
            if skipped["malformed"].as_i64().unwrap_or(0)
                + skipped["oversized"].as_i64().unwrap_or(0)
                > 0
            {
                r.warn(&format!(
                    "skipped transcript lines: {} unreadable, {} over 8 MB",
                    skipped["malformed"], skipped["oversized"]
                ));
            }
            for w in rep["warnings"].as_array().into_iter().flatten() {
                r.warn(w.as_str().unwrap_or_default());
            }
            if rep["bridge"]["connected"].as_bool() == Some(true) {
                let panes = rep["bridge"]["browser_panes"].as_bool() == Some(true);
                r.ok(&format!(
                    "iTerm2 bridge connected; pages open in {}",
                    if panes {
                        "iTerm2 browser panes"
                    } else {
                        "your default browser (fallback)"
                    }
                ));
            } else {
                r.warn("iTerm2 bridge not connected (is iTerm2 running?)");
            }
            if rep["keep_awake"]["holding"].as_bool() == Some(true) {
                r.ok(&format!(
                    "keeping the Mac awake for {} chats",
                    rep["keep_awake"]["chats"]
                ));
            } else {
                r.ok("not keeping the Mac awake");
            }
            if rep["gh"].is_null() {
                r.warn("gh not found: PR badges show \"unknown\"");
            }
        }
        None => r.bad(
            "the service isn't answering (see ~/Library/Application Support/bletherneuk/logs/)",
        ),
    }
    if let Some(gh) = crate::service::find_on_path("gh") {
        if command_ok(&gh.to_string_lossy(), &["auth", "status"]).is_some() {
            r.ok("gh logged in");
        } else {
            r.warn("gh isn't logged in (run `gh auth login`): PR badges show \"unknown\"");
        }
    }
    let socket = paths.socket();
    if let Ok(meta) = std::fs::metadata(&socket) {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 == 0 {
            r.ok("socket is private");
        } else {
            r.bad(&format!(
                "socket {} is readable by others",
                socket.display()
            ));
        }
    }

    if live {
        let _ = writeln!(r.out, "\nLive check");
        match crate::socket::request(
            paths,
            &json!({"type": "live_check"}),
            Duration::from_secs(30),
        ) {
            Ok(v) if v["ok"].as_bool() == Some(true) => {
                r.ok("opened and closed a browser pane in iTerm2")
            }
            Ok(v) => r.bad(&format!(
                "couldn't open a browser pane: {}",
                v["error"].as_str().unwrap_or("unknown error")
            )),
            Err(e) => r.bad(&e.to_string()),
        }
    }

    let _ = writeln!(r.out);
    if r.problems == 0 {
        let _ = writeln!(r.out, "All good.");
    } else {
        let _ = writeln!(r.out, "{} problem(s) found.", r.problems);
    }
    r.problems
}
