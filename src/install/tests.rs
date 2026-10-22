//! Design §20.11: install and uninstall against a temporary home folder.

use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};

use super::*;
use crate::paths::Paths;

#[derive(Default)]
struct FakeLaunchctl(RefCell<Vec<Vec<String>>>);

impl Launchctl for FakeLaunchctl {
    fn run(&self, args: &[String]) -> anyhow::Result<()> {
        self.0.borrow_mut().push(args.to_vec());
        Ok(())
    }
}

const NOW: i64 = 1_767_225_600_000;

fn setup() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap().join("Some User");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let paths = Paths::from_home(&home, "/tmp");
    (dir, paths)
}

fn other_hooks() -> Value {
    json!({
        "model": "opus",
        "hooks": {
            "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "osascript -e 'beep'"}]}],
            "Notification": [{"hooks": [{"type": "command", "command": "/opt/tool/status"}]}]
        },
        "zz_last": true
    })
}

fn installer<'a>(paths: &'a Paths, l: &'a FakeLaunchctl, dry_run: bool) -> Installer<'a> {
    Installer {
        paths,
        binary: paths.installed_binary(),
        dry_run,
        launchctl: l,
        uid: 501,
        path_env: "/usr/bin:/bin".into(),
    }
}

#[test]
fn hook_command_is_a_quoted_absolute_path() {
    assert_eq!(
        hook_command(Path::new("/Users/Some User/.local/bin/blether")),
        "\"/Users/Some User/.local/bin/blether\" hook"
    );
    assert_eq!(
        hook_command(Path::new("/x/a\"b$c`d\\e/blether")),
        "\"/x/a\\\"b\\$c\\`d\\\\e/blether\" hook"
    );
}

#[test]
fn install_adds_hooks_and_keeps_everything_else() {
    let (_d, paths) = setup();
    let settings = paths.claude_settings();
    std::fs::write(
        &settings,
        serde_json::to_string_pretty(&other_hooks()).unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o644)).unwrap();
    let l = FakeLaunchctl::default();
    let mut out = Vec::new();
    installer(&paths, &l, false).install(&mut out, NOW).unwrap();

    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let command = hook_command(&paths.installed_binary());
    for ev in HOOK_EVENTS {
        let list = v["hooks"][ev].as_array().unwrap();
        assert!(
            list.iter()
                .any(|e| e["hooks"][0]["command"] == command.as_str()),
            "{ev}"
        );
    }
    // Other hooks and keys are untouched, in their original order.
    assert_eq!(v["hooks"]["Stop"][0], other_hooks()["hooks"]["Stop"][0]);
    assert_eq!(
        v["hooks"]["Notification"][0],
        other_hooks()["hooks"]["Notification"][0]
    );
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["model", "hooks", "zz_last"]);
    assert_eq!(
        std::fs::metadata(&settings).unwrap().permissions().mode() & 0o777,
        0o644,
        "the file keeps its mode"
    );

    // The backup holds the old file, privately.
    let backup = std::fs::read_dir(settings.parent().unwrap())
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("settings.json.bletherneuk-backup-")
        })
        .unwrap()
        .path();
    assert_eq!(
        std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let old: Value = serde_json::from_str(&std::fs::read_to_string(&backup).unwrap()).unwrap();
    assert_eq!(old, other_hooks());

    // Bridge, launchd agent and starter config.
    assert_eq!(
        std::fs::read_to_string(paths.iterm_autolaunch().join(BRIDGE_FILE)).unwrap(),
        BRIDGE_SCRIPT
    );
    let plist = std::fs::read_to_string(paths.launch_agent()).unwrap();
    assert!(plist.contains("<string>dev.bletherneuk.serve</string>"));
    assert!(plist.contains(&format!(
        "<string>{}</string>",
        paths.installed_binary().display()
    )));
    assert!(plist.contains("<key>KeepAlive</key>"));
    assert_eq!(
        l.0.borrow().last().unwrap(),
        &vec![
            "bootstrap".to_string(),
            "gui/501".into(),
            paths.launch_agent().to_string_lossy().into_owned()
        ]
    );
    assert!(paths.config_file().is_file());
    assert!(paths.theme_file().is_file());
}

#[test]
fn installing_twice_adds_nothing_more() {
    let (_d, paths) = setup();
    std::fs::write(paths.claude_settings(), "{}").unwrap();
    let l = FakeLaunchctl::default();
    installer(&paths, &l, false)
        .install(&mut Vec::new(), NOW)
        .unwrap();
    let first = std::fs::read_to_string(paths.claude_settings()).unwrap();
    installer(&paths, &l, false)
        .install(&mut Vec::new(), NOW + 1000)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(paths.claude_settings()).unwrap(),
        first
    );
}

#[test]
fn invalid_settings_are_left_alone() {
    let (_d, paths) = setup();
    std::fs::write(paths.claude_settings(), "{ not json").unwrap();
    let l = FakeLaunchctl::default();
    assert!(
        installer(&paths, &l, false)
            .install(&mut Vec::new(), NOW)
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(paths.claude_settings()).unwrap(),
        "{ not json"
    );
    assert!(l.0.borrow().is_empty());
}

#[test]
fn dry_run_changes_nothing() {
    let (_d, paths) = setup();
    std::fs::write(paths.claude_settings(), "{}").unwrap();
    let l = FakeLaunchctl::default();
    let mut out = Vec::new();
    installer(&paths, &l, true).install(&mut out, NOW).unwrap();
    assert_eq!(
        std::fs::read_to_string(paths.claude_settings()).unwrap(),
        "{}"
    );
    assert!(!paths.launch_agent().exists());
    assert!(!paths.iterm_autolaunch().exists());
    assert!(l.0.borrow().is_empty());
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("would: add"), "{text}");
}

#[test]
fn uninstall_removes_only_our_entries() {
    let (_d, paths) = setup();
    std::fs::write(
        paths.claude_settings(),
        serde_json::to_string_pretty(&other_hooks()).unwrap(),
    )
    .unwrap();
    let l = FakeLaunchctl::default();
    installer(&paths, &l, false)
        .install(&mut Vec::new(), NOW)
        .unwrap();
    installer(&paths, &l, false)
        .uninstall(&mut Vec::new(), false, NOW + 1000)
        .unwrap();
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(paths.claude_settings()).unwrap()).unwrap();
    assert_eq!(v, other_hooks());
    assert!(!paths.launch_agent().exists());
    assert!(!paths.iterm_autolaunch().join(BRIDGE_FILE).exists());
    assert!(l.0.borrow().last().unwrap()[0] == "bootout");
}

#[test]
fn a_concurrent_change_is_kept() {
    let (_d, paths) = setup();
    let settings = paths.claude_settings();
    std::fs::write(&settings, "{\"a\": 1}").unwrap();
    // The edit runs twice: once on the file as first read, and again on the file
    // something else rewrote in the meantime.
    let calls = std::cell::Cell::new(0);
    edit_settings(&settings, NOW, |v| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            std::fs::write(&settings, "{\"a\": 1, \"b\": 2}").unwrap();
        }
        v["ours"] = json!(true);
        Ok(true)
    })
    .unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v, json!({"a": 1, "b": 2, "ours": true}));
}
