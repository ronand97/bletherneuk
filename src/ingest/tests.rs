use std::path::Path;

use rusqlite::Connection;

use super::*;
use crate::testutil::{Env, SESSION};

const NOW: i64 = 1_767_400_000_000;

fn chat_field<T: rusqlite::types::FromSql>(c: &Connection, col: &str) -> T {
    c.query_row(
        &format!("SELECT {col} FROM chats WHERE id = ?1"),
        [SESSION],
        |r| r.get(0),
    )
    .unwrap()
}

fn files(c: &Connection) -> Vec<(String, String, String)> {
    let mut stmt = c
        .prepare("SELECT path, kind, found_by FROM files ORDER BY path")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn writes(c: &Connection) -> Vec<(String, i64)> {
    let mut stmt = c
        .prepare("SELECT path, count FROM chat_writes ORDER BY path")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn setup() -> Env {
    let env = Env::new();
    env.write_file("shop/out/retry.html", "<html>");
    env.write_file("shop/out/latency.csv", "a,b");
    env
}

#[test]
fn classifies_transcript_paths() {
    let projects = Path::new("/h/.claude/projects");
    assert_eq!(
        classify_transcript(projects, &projects.join(format!("-w-shop/{SESSION}.jsonl"))),
        Some(Transcript::Main {
            session_id: SESSION.into()
        })
    );
    assert_eq!(
        classify_transcript(
            projects,
            &projects.join(format!("-w-shop/{SESSION}/subagents/agent-a1.jsonl"))
        ),
        Some(Transcript::Subagent {
            session_id: SESSION.into()
        })
    );
    assert_eq!(
        classify_transcript(projects, &projects.join("-w-shop/notes.jsonl")),
        None
    );
    assert_eq!(
        classify_transcript(projects, &projects.join(format!("-w-shop/{SESSION}.json"))),
        None
    );
    assert_eq!(
        classify_transcript(
            Path::new("/elsewhere"),
            &projects.join(format!("x/{SESSION}.jsonl"))
        ),
        None
    );
}

#[test]
fn reads_a_whole_transcript() {
    let mut env = setup();
    let path = env.transcript_path("-w-shop", SESSION);
    std::fs::write(&path, env.fixture("basic.jsonl")).unwrap();
    let ing = Ingestor::new(&env.config, &env.paths);
    let (id, counts) = ing
        .ingest_file(env.store.conn_mut(), &path, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(id, SESSION);
    assert_eq!(counts.malformed, 1);

    let root = env.root();
    let r = |s: &str| format!("{}/{s}", root.display());
    let c = env.store.conn();
    assert_eq!(chat_field::<String>(c, "title"), "Payment retries");
    assert_eq!(chat_field::<String>(c, "title_source"), "custom");
    assert_eq!(
        chat_field::<String>(c, "first_prompt"),
        "Add retry backoff to the payment client, see PR #42"
    );
    assert_eq!(
        chat_field::<String>(c, "recorded_branch"),
        "feat/abc_123_retry"
    );
    assert_eq!(chat_field::<String>(c, "start_cwd"), r("shop"));
    assert_eq!(
        chat_field::<String>(c, "last_cwd"),
        r("shop/.claude/worktrees/retry")
    );
    assert_eq!(chat_field::<i64>(c, "first_activity"), 1_767_344_400_000);
    assert_eq!(chat_field::<i64>(c, "last_activity"), 1_767_346_260_000);
    assert_eq!(
        chat_field::<String>(c, "transcript_path"),
        path.to_string_lossy()
    );
    assert!(!chat_field::<bool>(c, "hidden"));

    assert_eq!(
        writes(c),
        vec![
            (r("shop/.claude/worktrees/retry/analysis.ipynb"), 1),
            (r("shop/CLAUDE.md"), 1),
            (r("shop/docs/retry-design.md"), 1),
            (r("shop/src/client.py"), 2),
        ]
    );
    assert_eq!(
        files(c),
        vec![
            (r("shop/docs/retry-design.md"), "doc".into(), "tool".into()),
            (r("shop/out/latency.csv"), "output".into(), "output".into()),
            (r("shop/out/retry.html"), "output".into(), "command".into()),
        ]
    );
    let malformed: String = c
        .query_row(
            "SELECT value FROM meta WHERE key = 'ingest.malformed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(malformed, "1");
}

#[test]
fn reading_in_two_parts_matches_reading_once() {
    let mut env = setup();
    let path = env.transcript_path("-w-shop", SESSION);
    let text = env.fixture("basic.jsonl");
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    std::fs::write(&path, lines[..5].concat()).unwrap();
    let ing = Ingestor::new(&env.config, &env.paths);
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    assert_eq!(
        chat_field::<String>(env.store.conn(), "title"),
        "Add retry backoff to the payment client, see PR #42"
    );
    assert_eq!(
        chat_field::<String>(env.store.conn(), "title_source"),
        "prompt"
    );

    std::fs::write(&path, &text).unwrap();
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    let c = env.store.conn();
    assert_eq!(chat_field::<String>(c, "title"), "Payment retries");
    let client = format!("{}/shop/src/client.py", env.root().display());
    assert!(writes(c).contains(&(client, 2)));
}

#[test]
fn a_rewritten_transcript_is_read_again_without_double_counting() {
    let mut env = setup();
    let path = env.transcript_path("-w-shop", SESSION);
    let text = env.fixture("basic.jsonl");
    std::fs::write(&path, &text).unwrap();
    let ing = Ingestor::new(&env.config, &env.paths);
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    // Replace the file with a shorter one.
    let tmp = path.with_extension("tmp");
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    std::fs::write(&tmp, lines[..5].concat()).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    let client = format!("{}/shop/src/client.py", env.root().display());
    assert!(writes(env.store.conn()).contains(&(client, 1)));
}

#[test]
fn subagent_writes_count_for_the_parent_chat() {
    let mut env = setup();
    let main = env.transcript_path("-w-shop", SESSION);
    std::fs::write(&main, env.fixture("basic.jsonl")).unwrap();
    let sub_dir = main.with_extension("").join("subagents");
    std::fs::create_dir_all(&sub_dir).unwrap();
    let sub = sub_dir.join("agent-a1.jsonl");
    std::fs::write(&sub, env.fixture("subagent.jsonl")).unwrap();

    let ing = Ingestor::new(&env.config, &env.paths);
    ing.ingest_file(env.store.conn_mut(), &main, NOW).unwrap();
    let (id, _) = ing
        .ingest_file(env.store.conn_mut(), &sub, NOW)
        .unwrap()
        .unwrap();
    assert_eq!(id, SESSION);
    let c = env.store.conn();
    assert_eq!(chat_field::<String>(c, "title"), "Payment retries");
    assert_eq!(
        chat_field::<String>(c, "transcript_path"),
        main.to_string_lossy(),
        "the subagent file is not the chat's transcript"
    );
    let svg = format!("{}/shop/out/latency.svg", env.root().display());
    assert!(files(c).contains(&(svg, "output".into(), "tool".into())));
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM chats", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "subagents are not listed as chats");
}

#[test]
fn ai_title_wins_over_the_prompt_but_not_a_custom_title() {
    let mut env = setup();
    let path = env.transcript_path("-w-shop", SESSION);
    let line = |s: &str| format!("{s}\n");
    std::fs::write(
        &path,
        line(r#"{"type":"user","message":{"content":"first prompt"}}"#)
            + &line(r#"{"type":"ai-title","aiTitle":"AI one"}"#),
    )
    .unwrap();
    let ing = Ingestor::new(&env.config, &env.paths);
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    assert_eq!(chat_field::<String>(env.store.conn(), "title"), "AI one");

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    use std::io::Write;
    f.write_all(
        (line(r#"{"type":"custom-title","customTitle":"Mine"}"#)
            + &line(r#"{"type":"ai-title","aiTitle":"AI two"}"#))
            .as_bytes(),
    )
    .unwrap();
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    assert_eq!(chat_field::<String>(env.store.conn(), "title"), "Mine");
}

#[test]
fn long_prompts_make_short_titles() {
    let mut env = setup();
    let path = env.transcript_path("-w-shop", SESSION);
    let prompt = format!("line one\n{}", "x".repeat(200));
    let json = serde_json::json!({"type": "user", "message": {"content": prompt}});
    std::fs::write(&path, format!("{json}\n")).unwrap();
    let ing = Ingestor::new(&env.config, &env.paths);
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    let title: String = chat_field(env.store.conn(), "title");
    assert_eq!(title.chars().count(), 80);
    assert!(title.starts_with("line one x"));
}

#[test]
fn branchless_chats_in_temp_folders_are_hidden() {
    let mut env = setup();
    let scratch = env.base().join("scratch/run1");
    let path = env.transcript_path("-scratch-run1", SESSION);
    let json = serde_json::json!({
        "type": "user", "cwd": scratch, "gitBranch": "HEAD",
        "message": {"content": "scheduled job"}
    });
    std::fs::write(&path, format!("{json}\n")).unwrap();
    let ing = Ingestor::new(&env.config, &env.paths);
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    assert!(chat_field::<bool>(env.store.conn(), "hidden"));
}

#[test]
fn forgetting_a_transcript_removes_prompt_text() {
    let mut env = setup();
    let path = env.transcript_path("-w-shop", SESSION);
    std::fs::write(
        &path,
        "{\"type\":\"user\",\"message\":{\"content\":\"secret plan\"}}\n",
    )
    .unwrap();
    let ing = Ingestor::new(&env.config, &env.paths);
    ing.ingest_file(env.store.conn_mut(), &path, NOW).unwrap();
    env.store
        .conn()
        .execute(
            "INSERT INTO turns(chat_id, prompt, started_at) VALUES (?1, 'secret plan', 0)",
            [SESSION],
        )
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    ing.forget_transcript(env.store.conn(), &path).unwrap();
    let c = env.store.conn();
    assert_eq!(chat_field::<Option<String>>(c, "first_prompt"), None);
    assert_eq!(chat_field::<Option<String>>(c, "title"), None);
    assert!(chat_field::<bool>(c, "transcript_gone"));
    let p: Option<String> = c
        .query_row("SELECT prompt FROM turns", [], |r| r.get(0))
        .unwrap();
    assert_eq!(p, None);
}
