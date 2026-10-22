use std::path::Path;

use rusqlite::params;
use serde_json::json;

use super::testing::test_app;
use super::*;
use crate::status::Event;
use crate::worktrees::testrepo;

const CHAT: &str = "22222222-2222-4222-8222-222222222222";
const OTHER: &str = "33333333-3333-4333-8333-333333333333";
const SESSION: &str = "6F3A1E2B-1C2D-4E5F-8A9B-0C1D2E3F4A5B";

fn ev(name: &str, cwd: &Path) -> Event {
    Event {
        name: name.into(),
        session_id: CHAT.into(),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        ..Event::default()
    }
}

fn status_of(app: &Shared, chat: &str) -> String {
    app.db
        .lock()
        .unwrap()
        .conn()
        .query_row("SELECT status FROM chats WHERE id = ?1", [chat], |r| {
            r.get(0)
        })
        .unwrap()
}

#[test]
fn a_turn_records_status_snapshots_and_change_counts() {
    let mut t = test_app();
    let root = t.work_dir().join("shop");
    let repo = testrepo::init(&root);
    std::fs::write(root.join("app.py"), "a = 1\n").unwrap();
    testrepo::commit_all(&repo, "app");

    let app = t.app.clone();
    hooks::on_hook(&app, ev("SessionStart", &root), Some(SESSION.into()), 1_000);
    assert_eq!(status_of(&app, CHAT), "idle");
    hooks::on_hook(
        &app,
        Event {
            prompt: Some("Change a to 2".into()),
            ..ev("UserPromptSubmit", &root)
        },
        Some(SESSION.into()),
        2_000,
    );
    assert_eq!(status_of(&app, CHAT), "working");
    t.drain_snapshots();

    std::fs::write(root.join("app.py"), "a = 2\nb = 3\n").unwrap();
    let new_file = root.join("lib.py");
    std::fs::write(&new_file, "x = 1\n").unwrap();
    hooks::on_hook(
        &app,
        Event {
            tool_name: Some("Write".into()),
            file_path: Some(new_file.to_string_lossy().into_owned()),
            ..ev("PostToolUse", &root)
        },
        Some(SESSION.into()),
        3_000,
    );
    hooks::on_hook(&app, ev("Stop", &root), Some(SESSION.into()), 4_000);
    assert_eq!(status_of(&app, CHAT), "done");
    t.drain_snapshots();

    let (prompt, start, end, files, added, removed): (
        String,
        Option<i64>,
        Option<i64>,
        i64,
        i64,
        i64,
    ) = app
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT prompt, start_snapshot, end_snapshot, files_changed, lines_added, lines_removed
             FROM turns WHERE chat_id = ?1",
            [CHAT],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(prompt, "Change a to 2");
    assert!(start.is_some() && end.is_some());
    // app.py changed (+2 -1) and lib.py was added by the chat (+1).
    assert_eq!((files, added, removed), (2, 3, 1));

    let iterm: String = app
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT iterm_session FROM chats WHERE id = ?1",
            [CHAT],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(iterm, SESSION);
}

#[test]
fn parallel_turns_in_one_worktree_are_flagged() {
    let mut t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    let app = t.app.clone();
    let other = |name: &str| Event {
        session_id: OTHER.into(),
        ..ev(name, &root)
    };
    hooks::on_hook(&app, ev("UserPromptSubmit", &root), None, 1_000);
    hooks::on_hook(&app, other("UserPromptSubmit"), None, 1_500);
    hooks::on_hook(&app, ev("Stop", &root), None, 2_000);
    t.drain_snapshots();
    let parallel: bool = app
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT parallel FROM turns WHERE chat_id = ?1",
            [CHAT],
            |r| r.get(0),
        )
        .unwrap();
    assert!(parallel);
}

#[test]
fn a_chat_started_from_a_block_joins_it_on_its_first_event() {
    let t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    let app = t.app.clone();
    let block = {
        let db = app.db.lock().unwrap();
        let key = crate::group::Key::Branch {
            repo_root: root.to_string_lossy().into_owned(),
            branch: "feat/x".into(),
        };
        crate::group::block_for_key(db.conn(), &key, None, 0).unwrap()
    };
    // The bridge opened a tab for the block, then claude started in it.
    app.sessions.lock().unwrap().insert(
        SESSION.into(),
        Session {
            block: Some(block),
            ..Session::default()
        },
    );
    hooks::on_hook(&app, ev("SessionStart", &root), Some(SESSION.into()), 1_000);
    let (b, reason): (Option<i64>, String) = app
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT block_id, group_reason FROM chats WHERE id = ?1",
            [CHAT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(b, Some(block));
    assert_eq!(reason, "started from this block");
}

#[test]
fn keep_awake_holds_until_the_chat_is_done_and_past_the_grace_period() {
    let t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    let app = t.app.clone();
    hooks::on_hook(
        &app,
        ev("UserPromptSubmit", &root),
        None,
        crate::util::now_ms(),
    );
    actions::keep_awake_chat(&app, CHAT, true).unwrap();
    assert!(*t.awake.0.lock().unwrap());
    let now = crate::util::now_ms();
    hooks::on_hook(&app, ev("Stop", &root), None, now);
    tasks::tick_keep_awake(&app, now + 60_000);
    assert!(*t.awake.0.lock().unwrap(), "still within the grace period");
    tasks::tick_keep_awake(&app, now + 121_000);
    assert!(!*t.awake.0.lock().unwrap());
    let reason: String = app
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row("SELECT end_reason FROM keep_awake", [], |r| r.get(0))
        .unwrap();
    assert_eq!(reason, "done");
}

#[test]
fn keep_awake_needs_a_working_chat() {
    let t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    hooks::on_hook(&t.app, ev("Stop", &root), None, 1_000);
    assert!(actions::keep_awake_chat(&t.app, CHAT, true).is_err());
}

fn seed_block(t: &testing::TestApp) -> (i64, std::path::PathBuf) {
    let root = t.work_dir().join("shop");
    let repo = testrepo::init(&root);
    let wt = root.join(".claude/worktrees/retry");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    testrepo::add_worktree(&repo, "retry", &wt, "feat/retry");
    let app = &t.app;
    let config = app.config();
    let db = app.db.lock().unwrap();
    let c = db.conn();
    c.execute(
        "INSERT INTO chats(id, title, recorded_branch, start_cwd, last_cwd, last_activity, status)
         VALUES (?1, '<img src=x onerror=alert(1)>', 'feat/retry', ?2, ?2, 5, 'working')",
        params![CHAT, wt.to_string_lossy()],
    )
    .unwrap();
    let mut loc = app.locator.lock().unwrap();
    crate::group::refresh_location(c, &mut loc, CHAT).unwrap();
    drop(loc);
    crate::group::place_chat(c, &config, CHAT, 5).unwrap();
    let block: i64 = c
        .query_row("SELECT block_id FROM chats WHERE id = ?1", [CHAT], |r| {
            r.get(0)
        })
        .unwrap();
    drop(db);
    tasks::refresh_worktrees(app, None);
    (block, wt)
}

#[test]
fn board_and_block_views_have_the_documented_shape() {
    let t = test_app();
    let (block, _) = seed_block(&t);
    let board = views::board(&t.app, false);
    assert_eq!(board["blocks"][0]["id"], block);
    assert_eq!(board["blocks"][0]["title"], "feat/retry");
    assert_eq!(board["blocks"][0]["worktrees"], json!(["retry"]));
    assert_eq!(board["blocks"][0]["chats"]["working"], 1);
    assert_eq!(board["iterm_connected"], false);

    let detail = views::block(&t.app, block).unwrap();
    let chat = &detail["chats"][0];
    assert_eq!(chat["id"], CHAT);
    assert_eq!(chat["worktree"], "retry");
    assert_eq!(chat["branch"], "feat/retry");
    assert_eq!(chat["group_reason"], "on branch feat/retry");
    assert_eq!(chat["can_resume"], true);
    // Untrusted text is passed through as data; pages render it as text.
    assert_eq!(chat["title"], "<img src=x onerror=alert(1)>");
    let wt = &detail["worktrees"][0];
    assert_eq!(wt["name"], "retry");
    assert_eq!(wt["branch"], "feat/retry");
    assert_eq!(detail["new_chat_name"], "feat-retry");
    assert!(views::block(&t.app, 9999).is_none());
}

#[test]
fn review_then_diff_shows_only_later_changes() {
    let t = test_app();
    let (block, wt) = seed_block(&t);
    let wid = views::worktree_path(&t.app, block, None).unwrap().0;
    std::fs::write(wt.join("README.md"), "before review\n").unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(actions::mark_reviewed(&t.app, block, wid, None))
        .unwrap();
    std::fs::write(wt.join("README.md"), "after review\n").unwrap();

    let d = views::diff(&t.app, block, Some(wid), "review", None).unwrap();
    assert_eq!(d["range"]["kind"], "review");
    assert_eq!(d["totals"]["files"], 1);
    let lines = &d["files"][0]["hunks"][0]["lines"];
    assert_eq!(lines[0]["text"], "before review");
    assert_eq!(lines[1]["text"], "after review");

    let b = views::diff(&t.app, block, Some(wid), "branch", None).unwrap();
    assert_eq!(b["range"]["kind"], "branch");
    assert_eq!(b["files"][0]["new_path"], "README.md");
}

#[test]
fn links_can_be_added_opened_and_removed() {
    let t = test_app();
    let (block, _) = seed_block(&t);
    let r = actions::add_link(
        &t.app,
        block,
        "https://example.com/spec",
        Some("Spec"),
        true,
    )
    .unwrap();
    let link = r["link"].as_i64().unwrap();
    assert!(actions::add_link(&t.app, block, "javascript:alert(1)", None, false).is_err());
    actions::open_link(&t.app, link).unwrap();
    assert_eq!(
        t.opened.0.lock().unwrap()[0],
        vec![
            "-b",
            "com.example.Browser",
            "--",
            "https://example.com/spec"
        ]
    );
    let detail = views::block(&t.app, block).unwrap();
    assert_eq!(detail["links"][0]["title"], "Spec");
    assert_eq!(detail["links"][0]["added_by_claude"], true);
    assert_eq!(detail["links"][0]["host"], "example.com");
    actions::delete_link(&t.app, link).unwrap();
    assert_eq!(views::block(&t.app, block).unwrap()["links"], json!([]));
}

#[test]
fn files_open_with_their_default_app_and_hide() {
    let t = test_app();
    let (block, wt) = seed_block(&t);
    let report = wt.join("out/report.html");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    std::fs::write(&report, "<html>").unwrap();
    let id = {
        let db = t.app.db.lock().unwrap();
        db.conn()
            .execute(
                "INSERT INTO files(chat_id, path, kind, found_by, first_seen) VALUES (?1, ?2, 'output', 'tool', 0)",
                params![CHAT, report.to_string_lossy()],
            )
            .unwrap();
        db.conn().last_insert_rowid()
    };
    actions::open_file(&t.app, id).unwrap();
    assert_eq!(
        t.opened.0.lock().unwrap()[0],
        vec!["-b", "com.example.Viewer", "--", report.to_str().unwrap()]
    );
    actions::hide_file(&t.app, id, true).unwrap();
    let detail = views::block(&t.app, block).unwrap();
    assert_eq!(detail["outputs"][0]["hidden"], true);
    assert_eq!(detail["outputs"][0]["name"], "report.html");
}

#[test]
fn files_in_temp_folders_are_copied_and_the_copy_opens_once_the_original_is_gone() {
    let t = test_app();
    let (block, _) = seed_block(&t);
    let scratch = t.work_dir().parent().unwrap().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let report = scratch.join("map.html");
    std::fs::write(&report, "<html>map</html>").unwrap();
    let id = {
        let db = t.app.db.lock().unwrap();
        db.conn()
            .execute(
                "INSERT INTO files(chat_id, path, kind, found_by, first_seen) VALUES (?1, ?2, 'output', 'output', 0)",
                params![CHAT, report.to_string_lossy()],
            )
            .unwrap();
        db.conn().last_insert_rowid()
    };
    assert!(tasks::copy_temp_files(&t.app).contains(&block));
    std::fs::remove_file(&report).unwrap();
    let detail = views::block(&t.app, block).unwrap();
    assert_eq!(detail["outputs"][0]["copy"], true);
    assert_eq!(detail["outputs"][0]["missing"], false);
    actions::open_file(&t.app, id).unwrap();
    let opened = &t.opened.0.lock().unwrap()[0];
    assert!(opened[3].contains("/copies/"), "{opened:?}");
}

#[test]
fn stale_working_chats_close_when_their_tab_is_gone() {
    let t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    hooks::on_hook(
        &t.app,
        ev("UserPromptSubmit", &root),
        Some(SESSION.into()),
        0,
    );
    // With the bridge down nobody can tell whether the tab is still there.
    tasks::close_stale(&t.app, 11 * 60_000);
    assert_eq!(status_of(&t.app, CHAT), "working");

    *t.app.bridge.lock().unwrap() = Some(bridge::fake_handle("3.7.6"));
    assert!(tasks::close_stale(&t.app, 5 * 60_000).is_empty());
    assert_eq!(status_of(&t.app, CHAT), "working", "not stale yet");
    tasks::close_stale(&t.app, 11 * 60_000);
    assert_eq!(status_of(&t.app, CHAT), "closed");
}

#[test]
fn a_working_chat_whose_tab_is_open_is_never_closed_as_stale() {
    let t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    hooks::on_hook(
        &t.app,
        ev("UserPromptSubmit", &root),
        Some(SESSION.into()),
        0,
    );
    *t.app.bridge.lock().unwrap() = Some(bridge::fake_handle("3.7.6"));
    t.app
        .sessions
        .lock()
        .unwrap()
        .insert(SESSION.into(), Session::default());
    tasks::close_stale(&t.app, 60 * 60_000);
    assert_eq!(status_of(&t.app, CHAT), "working");
}

#[test]
fn page_codes_work_once_and_expire() {
    let t = test_app();
    let url = t.app.page_url("/panel");
    let code = url
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(t.app.redeem(&code).as_deref(), Some("/panel"));
    assert_eq!(t.app.redeem(&code), None, "a code works once");

    let url = t.app.page_url("/");
    let code = url
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    t.app.codes.lock().unwrap().get_mut(&code).unwrap().expires =
        std::time::Instant::now() - std::time::Duration::from_secs(1);
    assert_eq!(t.app.redeem(&code), None, "an expired code doesn't work");
}

#[test]
fn page_paths_are_limited() {
    assert_eq!(App::page_path("board", None).as_deref(), Some("/"));
    assert_eq!(
        App::page_path("changes", Some(4)).as_deref(),
        Some("/changes?block=4")
    );
    assert_eq!(App::page_path("changes", None), None);
    assert_eq!(App::page_path("../etc", None), None);
}

#[test]
fn a_chat_whose_worktree_was_removed_resumes_in_the_main_checkout() {
    let t = test_app();
    let (block, wt) = seed_block(&t);
    let root = t.work_dir().join("shop");
    assert_eq!(
        actions::resume_folder(&t.app, CHAT).unwrap(),
        wt.to_string_lossy()
    );
    std::fs::remove_dir_all(&wt).unwrap();
    tasks::refresh_worktrees(&t.app, None);
    assert_eq!(
        actions::resume_folder(&t.app, CHAT).unwrap(),
        root.to_string_lossy()
    );
    let chat = &views::block(&t.app, block).unwrap()["chats"][0];
    assert_eq!(chat["can_resume"], true);
    assert_eq!(
        chat["resume_note"],
        "worktree removed; resumes in the main checkout"
    );
}

#[test]
fn a_tab_opened_by_hand_in_a_block_window_is_not_pinned_to_the_block() {
    let t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    let block = {
        let db = t.app.db.lock().unwrap();
        let key = crate::group::Key::Branch {
            repo_root: root.to_string_lossy().into_owned(),
            branch: "feat/x".into(),
        };
        crate::group::block_for_key(db.conn(), &key, None, 0).unwrap()
    };
    // After a reconnect, the bridge reports every session with its window's block.
    let hello = json!({"type": "bridge_hello", "sessions": [
        {"session": SESSION, "window": "w1", "tab": "t1", "block": block, "chat": null}
    ]});
    bridge::apply_hello(&t.app, &hello);
    hooks::on_hook(
        &t.app,
        ev("SessionStart", &root),
        Some(SESSION.into()),
        1_000,
    );
    let (b, reason): (Option<i64>, Option<String>) = t
        .app
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT block_id, group_reason FROM chats WHERE id = ?1",
            [CHAT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(b, None);
    assert_ne!(reason.as_deref(), Some("started from this block"));
}

fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn write_interrupt(t: &testing::TestApp, root: &Path, at: i64) -> std::path::PathBuf {
    let dir = t.app.paths.claude_projects().join("-w-shop");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{CHAT}.jsonl"));
    let line = json!({
        "type": "user", "sessionId": CHAT, "cwd": root, "timestamp": iso(at),
        "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}
    });
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

#[test]
fn an_interrupted_chat_stops_working_and_its_turn_ends() {
    let mut t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    let start = 1_767_225_600_000;
    hooks::on_hook(&t.app, ev("UserPromptSubmit", &root), None, start);
    t.drain_snapshots();
    let path = write_interrupt(&t, &root, start + 5_000);
    tasks::ingest_one(&t.app, &path);
    assert_eq!(status_of(&t.app, CHAT), "done");
    t.drain_snapshots();
    let (ended, end): (Option<i64>, Option<i64>) = t
        .app
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row(
            "SELECT ended_at, end_snapshot FROM turns WHERE chat_id = ?1",
            [CHAT],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(ended, Some(start + 5_000));
    assert!(end.is_some());
}

#[test]
fn an_old_interrupt_doesnt_stop_a_newer_turn() {
    let t = test_app();
    let root = t.work_dir().join("shop");
    testrepo::init(&root);
    let start = 1_767_225_600_000;
    hooks::on_hook(&t.app, ev("UserPromptSubmit", &root), None, start + 10_000);
    let path = write_interrupt(&t, &root, start + 5_000);
    tasks::ingest_one(&t.app, &path);
    assert_eq!(status_of(&t.app, CHAT), "working");
}

#[test]
fn mark_reviewed_covers_exactly_what_was_shown() {
    let t = test_app();
    let (block, wt) = seed_block(&t);
    let wid = views::worktree_path(&t.app, block, None).unwrap().0;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(actions::mark_reviewed(&t.app, block, wid, None))
        .unwrap();

    std::fs::write(wt.join("README.md"), "seen\n").unwrap();
    let shown = views::diff(&t.app, block, Some(wid), "review", None).unwrap();
    let tree = shown["to_tree"].as_str().unwrap().to_string();
    // More changes land after the page loaded.
    std::fs::write(wt.join("README.md"), "seen\nnot yet seen\n").unwrap();
    rt.block_on(actions::mark_reviewed(&t.app, block, wid, Some(&tree)))
        .unwrap();

    let after = views::diff(&t.app, block, Some(wid), "review", None).unwrap();
    assert_eq!(
        after["totals"]["files"], 1,
        "the unseen change is still to review"
    );
    assert_eq!(
        after["files"][0]["hunks"][0]["lines"][1]["text"],
        "not yet seen"
    );

    assert!(
        rt.block_on(actions::mark_reviewed(
            &t.app,
            block,
            wid,
            Some("not-a-tree")
        ))
        .is_err()
    );
    assert!(
        rt.block_on(actions::mark_reviewed(
            &t.app,
            block,
            wid,
            Some(&"0".repeat(40))
        ))
        .is_err()
    );
}

#[test]
fn files_a_chat_wrote_are_snapshotted_even_under_non_ascii_folders() {
    let t = test_app();
    let root = t.work_dir().join("café");
    testrepo::init(&root);
    let new = root.join("notes/plan.txt");
    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::write(&new, "plan").unwrap();
    {
        let db = t.app.db.lock().unwrap();
        db.conn()
            .execute("INSERT INTO chats(id) VALUES (?1)", [CHAT])
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO chat_writes(chat_id, path, count) VALUES (?1, ?2, 1)",
                params![CHAT, new.to_string_lossy()],
            )
            .unwrap();
    }
    let id = snapshots::take(&t.app, &root, "review", &[]).unwrap();
    let (repo, tree) = snapshots::tree_of(&t.app, id).unwrap();
    let shadow =
        crate::snapshots::Shadow::open(&t.app.paths.snapshots(), Path::new(&repo)).unwrap();
    let tree = shadow.repo.find_tree(tree).unwrap();
    assert!(tree.get_path(Path::new("notes/plan.txt")).is_ok());
}
