//! `blether install` and `blether uninstall` (design §18).

pub mod doctor;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};

use crate::paths::{LAUNCHD_LABEL, Paths};

/// Claude Code hook events `blether hook` is registered for. Besides the events in the
/// design, `PermissionRequest`, `StopFailure` and `SubagentStop` give clearer
/// "needs you", "done" and background-work signals.
pub const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Notification",
    "SubagentStop",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

pub const BRIDGE_SCRIPT: &str = include_str!("../../bridge/bletherneuk_bridge.py");
pub const BRIDGE_FILE: &str = "bletherneuk_bridge.py";

/// The hook command: the quoted absolute path to the installed binary, then `hook`.
/// Quoting means a `blether` in a repo's `.venv/bin` can never stand in for it.
pub fn hook_command(binary: &Path) -> String {
    let p = binary.to_string_lossy();
    let escaped: String = p
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' | '$' | '`' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("\"{escaped}\" hook")
}

fn is_ours(entry_hook: &Value, command: &str) -> bool {
    entry_hook.get("command").and_then(Value::as_str) == Some(command)
}

/// Adds one hook entry per event, leaving existing hooks alone. Returns the events
/// that gained an entry.
pub fn add_hooks(settings: &mut Value, command: &str) -> anyhow::Result<Vec<String>> {
    let obj = settings
        .as_object_mut()
        .context("settings.json is not a JSON object")?;
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .context("\"hooks\" in settings.json is not an object")?;
    let mut added = Vec::new();
    for ev in HOOK_EVENTS {
        let list = hooks.entry(ev.to_string()).or_insert_with(|| json!([]));
        let list = list
            .as_array_mut()
            .with_context(|| format!("hooks.{ev} is not a list"))?;
        let present = list.iter().any(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hs| hs.iter().any(|h| is_ours(h, command)))
        });
        if !present {
            list.push(json!({"hooks": [{"type": "command", "command": command}]}));
            added.push(ev.to_string());
        }
    }
    Ok(added)
}

/// Removes only entries whose command is exactly ours. Returns how many were removed.
pub fn remove_hooks(settings: &mut Value, command: &str) -> usize {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut removed = 0;
    let mut emptied = Vec::new();
    for (ev, list) in hooks.iter_mut() {
        let Some(list) = list.as_array_mut() else {
            continue;
        };
        let before_entries = list.len();
        for entry in list.iter_mut() {
            if let Some(hs) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hs.len();
                hs.retain(|h| !is_ours(h, command));
                removed += before - hs.len();
            }
        }
        list.retain(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|hs| !hs.is_empty())
        });
        if list.is_empty() && before_entries > 0 {
            emptied.push(ev.clone());
        }
    }
    for ev in emptied {
        hooks.shift_remove(&ev);
    }
    removed
}

/// Rewrites `settings.json` safely: refuses invalid JSON, backs up first (mode 0600),
/// re-reads just before writing in case something else changed it, and replaces the
/// file through a rename.
pub fn edit_settings(
    path: &Path,
    now_ms: i64,
    edit: impl Fn(&mut Value) -> anyhow::Result<bool>,
) -> anyhow::Result<Option<PathBuf>> {
    let read = || -> anyhow::Result<(Option<String>, Value)> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let v: Value = serde_json::from_str(&text).with_context(|| {
                    format!("{} isn't valid JSON, so it was left alone", path.display())
                })?;
                Ok((Some(text), v))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, json!({}))),
            Err(e) => Err(e.into()),
        }
    };
    let (original, mut value) = read()?;
    if !edit(&mut value)? {
        return Ok(None);
    }
    let mut backup = None;
    if let Some(text) = &original {
        let b = path.with_file_name(format!(
            "settings.json.bletherneuk-backup-{}",
            chrono::DateTime::from_timestamp_millis(now_ms)
                .unwrap_or_default()
                .format("%Y%m%d-%H%M%S")
        ));
        crate::util::write_private_atomic(&b, text.as_bytes())?;
        backup = Some(b);
    }
    // Something else may have written in the meantime: apply the edit to what's there now.
    let (latest, mut fresh) = read()?;
    if latest != original {
        value = std::mem::take(&mut fresh);
        edit(&mut value)?;
    }
    let mode = std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    let mut text = serde_json::to_string_pretty(&value)?;
    text.push('\n');
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::util::write_private_atomic(path, text.as_bytes())?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(backup)
}

/// Runs `launchctl`. Tests swap in a recorder.
pub trait Launchctl {
    fn run(&self, args: &[String]) -> anyhow::Result<()>;
}

pub struct SystemLaunchctl;

impl Launchctl for SystemLaunchctl {
    fn run(&self, args: &[String]) -> anyhow::Result<()> {
        let out = std::process::Command::new("/bin/launchctl")
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "launchctl {} failed: {}",
            args.first().map(String::as_str).unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd agent that runs the service at login and restarts it if it stops.
pub fn launch_agent_plist(paths: &Paths, binary: &Path, path_env: &str) -> String {
    let logs = paths.logs();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
  <key>StandardOutPath</key>
  <string>{out}</string>
  <key>StandardErrorPath</key>
  <string>{err}</string>
</dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        bin = xml_escape(&binary.to_string_lossy()),
        path = xml_escape(path_env),
        out = xml_escape(&logs.join("serve.log").to_string_lossy()),
        err = xml_escape(&logs.join("serve.err.log").to_string_lossy()),
    )
}

/// PATH for the service: the usual places, plus wherever `gh` is now.
pub fn service_path_env() -> String {
    let mut dirs: Vec<String> = Vec::new();
    if let Some(gh) = crate::service::find_on_path("gh")
        && let Some(d) = gh.parent()
    {
        dirs.push(d.to_string_lossy().into_owned());
    }
    for d in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        if !dirs.iter().any(|x| x == d) {
            dirs.push(d.into());
        }
    }
    dirs.join(":")
}

pub struct Installer<'a> {
    pub paths: &'a Paths,
    pub binary: PathBuf,
    pub dry_run: bool,
    pub launchctl: &'a dyn Launchctl,
    pub uid: u32,
    pub path_env: String,
}

impl Installer<'_> {
    fn say(&self, out: &mut dyn Write, msg: &str) {
        let prefix = if self.dry_run { "would: " } else { "" };
        let _ = writeln!(out, "{prefix}{msg}");
    }

    pub fn install(&self, out: &mut dyn Write, now_ms: i64) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.binary.is_absolute(),
            "the binary path must be absolute"
        );
        let command = hook_command(&self.binary);

        // 1. Claude Code settings.
        let settings = self.paths.claude_settings();
        if self.dry_run {
            let text = std::fs::read_to_string(&settings).unwrap_or_else(|_| "{}".into());
            let mut v: Value = serde_json::from_str(&text)
                .with_context(|| format!("{} isn't valid JSON", settings.display()))?;
            let added = add_hooks(&mut v, &command)?;
            self.say(
                out,
                &format!(
                    "add `{command}` to {} for: {}",
                    settings.display(),
                    if added.is_empty() {
                        "nothing (already there)".into()
                    } else {
                        added.join(", ")
                    }
                ),
            );
        } else {
            let backup = edit_settings(&settings, now_ms, |v| {
                Ok(!add_hooks(v, &command)?.is_empty())
            })?;
            match backup {
                Some(b) => self.say(
                    out,
                    &format!(
                        "added hooks to {} (backup: {})",
                        settings.display(),
                        b.display()
                    ),
                ),
                None => self.say(out, &format!("hooks already in {}", settings.display())),
            }
        }

        // 2. The iTerm2 bridge.
        let autolaunch = self.paths.iterm_autolaunch();
        let target = autolaunch.join(BRIDGE_FILE);
        self.say(
            out,
            &format!("copy the iTerm2 bridge to {}", target.display()),
        );
        if !self.dry_run {
            std::fs::create_dir_all(&autolaunch)?;
            std::fs::write(&target, BRIDGE_SCRIPT)?;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))?;
        }

        // 3. The background service.
        let plist = self.paths.launch_agent();
        self.say(
            out,
            &format!("write {} and load it with launchctl", plist.display()),
        );
        if !self.dry_run {
            crate::paths::ensure_private_dir(&self.paths.data())?;
            crate::paths::ensure_private_dir(&self.paths.logs())?;
            std::fs::create_dir_all(plist.parent().expect("has parent"))?;
            std::fs::write(
                &plist,
                launch_agent_plist(self.paths, &self.binary, &self.path_env),
            )?;
            let domain = format!("gui/{}", self.uid);
            // Unload an older copy first; it's fine if there isn't one.
            let _ = self
                .launchctl
                .run(&["bootout".into(), format!("{domain}/{LAUNCHD_LABEL}")]);
            self.launchctl.run(&[
                "bootstrap".into(),
                domain,
                plist.to_string_lossy().into_owned(),
            ])?;
        }

        // 4. Starter config.
        for (file, text) in [
            (self.paths.config_file(), crate::config::STARTER),
            (self.paths.theme_file(), crate::theme::STARTER),
        ] {
            if !file.exists() {
                self.say(out, &format!("write a starter {}", file.display()));
                if !self.dry_run {
                    std::fs::create_dir_all(file.parent().expect("has parent"))?;
                    std::fs::write(&file, text)?;
                }
            }
        }
        Ok(())
    }

    pub fn uninstall(&self, out: &mut dyn Write, purge: bool, now_ms: i64) -> anyhow::Result<()> {
        let command = hook_command(&self.binary);
        let settings = self.paths.claude_settings();
        if settings.exists() {
            let backup = edit_settings(&settings, now_ms, |v| Ok(remove_hooks(v, &command) > 0))?;
            match backup {
                Some(b) => self.say(
                    out,
                    &format!(
                        "removed hooks from {} (backup: {})",
                        settings.display(),
                        b.display()
                    ),
                ),
                None => self.say(
                    out,
                    &format!("no bletherneuk hooks in {}", settings.display()),
                ),
            }
        }
        let bridge = self.paths.iterm_autolaunch().join(BRIDGE_FILE);
        if bridge.exists() {
            std::fs::remove_file(&bridge)?;
            self.say(out, &format!("removed {}", bridge.display()));
        }
        let plist = self.paths.launch_agent();
        let _ = self.launchctl.run(&[
            "bootout".into(),
            format!("gui/{}/{LAUNCHD_LABEL}", self.uid),
        ]);
        if plist.exists() {
            std::fs::remove_file(&plist)?;
            self.say(out, &format!("removed {}", plist.display()));
        }
        if purge {
            let data = self.paths.data();
            if data.exists() {
                std::fs::remove_dir_all(&data)?;
                self.say(out, &format!("deleted {}", data.display()));
            }
        } else {
            self.say(
                out,
                &format!(
                    "kept your data in {} (use --purge to delete it)",
                    self.paths.data().display()
                ),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
