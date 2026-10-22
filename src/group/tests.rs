use std::path::PathBuf;

use rusqlite::{Connection, params};

use super::*;
use crate::links::gh::{Checks, Pr};
use crate::testutil::Env;
use crate::worktrees::testrepo;

const NOW: i64 = 1_767_400_000_000;

struct Repo {
    root: PathBuf,
    wt: PathBuf,
    _repo: git2::Repository,
    wt_repo: git2::Repository,
}

fn setup(env: &mut Env) -> Repo {
    let root = env.root().join("shop");
    let repo = testrepo::init(&root);
    repo.remote("origin", "git@github.com:acme/shop.git")
        .unwrap();
    let wt = root.join(".claude/worktrees/retry");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    let wt_repo = testrepo::add_worktree(&repo, "retry", &wt, "feat/retry");
    env.config.archive.idle_days = 14;
    env.config.tickets.projects = vec!["ABC".into()];
    store_ticket_prefixes(env.store.conn(), &env.config).unwrap();
    Repo {
        root,
        wt,
        _repo: repo,
        wt_repo,
    }
}

struct ChatSpec<'a> {
    id: &'a str,
    cwd: &'a std::path::Path,
    branch: &'a str,
    title: Option<&'a str>,
    prompt: Option<&'a str>,
    last_activity: i64,
}

fn add_chat(c: &Connection, s: ChatSpec) {
    c.execute(
        "INSERT INTO chats(id, title, first_prompt, recorded_branch, start_cwd, last_cwd,
                           last_activity)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
        params![
            s.id,
            s.title,
            s.prompt,
            s.branch,
            s.cwd.to_string_lossy(),
            s.last_activity
        ],
    )
    .unwrap();
}

fn chat(id: &str, cwd: &std::path::Path, branch: &str) -> ChatSpec<'static> {
    ChatSpec {
        id: Box::leak(id.to_string().into_boxed_str()),
        cwd: Box::leak(cwd.to_path_buf().into_boxed_path()),
        branch: Box::leak(branch.to_string().into_boxed_str()),
        title: None,
        prompt: None,
        last_activity: NOW,
    }
}

fn place(env: &Env, id: &str) -> Option<i64> {
    let c = env.store.conn();
    let mut loc = Locator::default();
    refresh_location(c, &mut loc, id).unwrap();
    place_chat(c, &env.config, id, NOW).unwrap();
    block_and_reason(c, id).0
}

fn block_and_reason(c: &Connection, id: &str) -> (Option<i64>, String) {
    c.query_row(
        "SELECT block_id, group_reason FROM chats WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn title(c: &Connection, block: i64) -> String {
    c.query_row("SELECT title FROM blocks WHERE id = ?1", [block], |r| {
        r.get(0)
    })
    .unwrap()
}

fn block_count(c: &Connection) -> i64 {
    c.query_row("SELECT COUNT(*) FROM blocks", [], |r| r.get(0))
        .unwrap()
}

fn pr(number: u64, title: &str, head: &str, state: &str) -> Pr {
    Pr {
        number,
        title: title.into(),
        head_branch: head.into(),
        state: state.into(),
        is_draft: false,
        review_decision: None,
        checks: Checks::Passing,
        url: format!("https://github.com/acme/shop/pull/{number}"),
    }
}

#[test]
fn chats_on_one_branch_share_a_block() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("a", &r.wt, "feat/retry"));
    add_chat(env.store.conn(), chat("b", &r.wt, "feat/retry"));
    let a = place(&env, "a").unwrap();
    let b = place(&env, "b").unwrap();
    assert_eq!(a, b);
    let c = env.store.conn();
    assert_eq!(title(c, a), "feat/retry");
    assert_eq!(block_and_reason(c, "a").1, "on branch feat/retry");
    let key: String = c
        .query_row("SELECT key FROM block_keys WHERE block_id = ?1", [a], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(key, format!("branch:{}@feat/retry", r.root.display()));
}

#[test]
fn default_branch_chats_are_unsorted() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("a", &r.root, "main"));
    assert_eq!(place(&env, "a"), None);
    assert_eq!(block_and_reason(env.store.conn(), "a").1, "no clue matched");
    assert_eq!(block_count(env.store.conn()), 0);
}

#[test]
fn a_pr_review_on_main_joins_the_branch_block_once_gh_reports_the_head() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("work", &r.wt, "feat/retry"));
    add_chat(
        env.store.conn(),
        ChatSpec {
            title: Some("Review PR #42"),
            ..chat("review", &r.root, "main")
        },
    );
    let work = place(&env, "work").unwrap();
    let review = place(&env, "review").unwrap();
    assert_ne!(work, review);
    assert_eq!(title(env.store.conn(), review), "acme/shop#42");

    let changed = apply_prs(
        env.store.conn(),
        &env.config,
        "acme/shop",
        &[pr(42, "Add retry backoff", "feat/retry", "OPEN")],
        NOW,
    )
    .unwrap();
    let c = env.store.conn();
    assert_eq!(block_count(c), 1);
    let (b, reason) = block_and_reason(c, "review");
    assert_eq!(b, Some(work.min(review)));
    assert_eq!(reason, "title mentions PR #42");
    assert_eq!(block_and_reason(c, "work").0, b);
    assert!(changed.contains(&b.unwrap()));
    assert_eq!(title(c, b.unwrap()), "Add retry backoff");
    let (url, status): (String, String) = c
        .query_row(
            "SELECT url, status FROM links WHERE block_id = ?1 AND kind = 'github_pr'",
            [b.unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(url, "https://github.com/acme/shop/pull/42");
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["state"], "OPEN");
    assert_eq!(status["checks"], "passing");
}

#[test]
fn a_pr_with_no_matching_block_creates_nothing() {
    let mut env = Env::new();
    let _r = setup(&mut env);
    apply_prs(
        env.store.conn(),
        &env.config,
        "acme/shop",
        &[pr(9, "Unrelated", "feat/other", "OPEN")],
        NOW,
    )
    .unwrap();
    assert_eq!(block_count(env.store.conn()), 0);
}

#[test]
fn worktree_drift_follows_the_files_and_survives_a_branch_switch() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("drift", &r.root, "main"));
    env.store
        .conn()
        .execute(
            "INSERT INTO chat_writes(chat_id, path, count) VALUES ('drift', ?1, 3), ('drift', ?2, 1)",
            params![
                r.wt.join("src/a.py").to_string_lossy(),
                r.root.join("notes.txt").to_string_lossy()
            ],
        )
        .unwrap();
    let b = place(&env, "drift").unwrap();
    let c = env.store.conn();
    assert_eq!(title(c, b), "feat/retry");
    assert_eq!(
        block_and_reason(c, "drift").1,
        "wrote files in worktree retry (feat/retry)"
    );

    // The worktree moves to another branch; the chat stays with its work.
    let head = r.wt_repo.head().unwrap().peel_to_commit().unwrap();
    r.wt_repo.branch("feat/next", &head, false).unwrap();
    r.wt_repo.set_head("refs/heads/feat/next").unwrap();
    assert_eq!(place(&env, "drift"), Some(b));
}

#[test]
fn manual_moves_win_and_stick() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("a", &r.wt, "feat/retry"));
    add_chat(
        env.store.conn(),
        ChatSpec {
            prompt: Some("work on ABC-7"),
            ..chat("b", &r.root, "main")
        },
    );
    let a_block = place(&env, "a").unwrap();
    let b_block = place(&env, "b").unwrap();
    let c = env.store.conn();
    move_chat(c, &env.config, "a", Some(b_block), NOW).unwrap();
    assert_eq!(
        block_and_reason(c, "a"),
        (Some(b_block), "moved by you".into())
    );
    // Re-evaluating doesn't undo it.
    assert_eq!(place(&env, "a"), Some(b_block));
    move_chat(env.store.conn(), &env.config, "a", None, NOW).unwrap();
    assert_eq!(place(&env, "a"), None);
    assert_ne!(a_block, b_block);
}

#[test]
fn a_chat_started_from_a_block_stays_there() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("a", &r.wt, "feat/retry"));
    let block = place(&env, "a").unwrap();
    // A new chat in a brand-new worktree whose branch nothing else knows.
    add_chat(env.store.conn(), chat("new", &r.root, "feat/brand-new"));
    assign_started_from(env.store.conn(), &env.config, "new", block, NOW).unwrap();
    assert_eq!(place(&env, "new"), Some(block));
    assert_eq!(
        block_and_reason(env.store.conn(), "new").1,
        "started from this block"
    );
}

#[test]
fn a_ticket_in_a_pr_title_joins_the_ticket_block() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("branch", &r.wt, "feat/retry"));
    add_chat(
        env.store.conn(),
        ChatSpec {
            prompt: Some("Plan ABC-12 before coding"),
            ..chat("ticket", &r.root, "main")
        },
    );
    place(&env, "branch");
    place(&env, "ticket");
    assert_eq!(block_count(env.store.conn()), 2);
    apply_prs(
        env.store.conn(),
        &env.config,
        "acme/shop",
        &[pr(5, "ABC-12 retry backoff", "feat/retry", "OPEN")],
        NOW,
    )
    .unwrap();
    let c = env.store.conn();
    assert_eq!(block_count(c), 1);
    assert_eq!(
        block_and_reason(c, "branch").0,
        block_and_reason(c, "ticket").0
    );
}

#[test]
fn renames_win_over_pr_titles_and_survive_merges() {
    let mut env = Env::new();
    let r = setup(&mut env);
    add_chat(env.store.conn(), chat("a", &r.wt, "feat/retry"));
    add_chat(
        env.store.conn(),
        ChatSpec {
            title: Some("PR #42 follow-up"),
            ..chat("b", &r.root, "main")
        },
    );
    let a = place(&env, "a").unwrap();
    let b = place(&env, "b").unwrap();
    rename_block(env.store.conn(), &env.config, b, Some("My retries"), NOW).unwrap();
    apply_prs(
        env.store.conn(),
        &env.config,
        "acme/shop",
        &[pr(42, "Add retry backoff", "feat/retry", "OPEN")],
        NOW,
    )
    .unwrap();
    let c = env.store.conn();
    let survivor = a.min(b);
    assert_eq!(title(c, survivor), "My retries");
    rename_block(c, &env.config, survivor, None, NOW).unwrap();
    assert_eq!(title(c, survivor), "Add retry backoff");
}

#[test]
fn merged_and_idle_blocks_are_archived_until_new_activity() {
    let mut env = Env::new();
    let r = setup(&mut env);
    let old = NOW - 15 * 86_400_000;
    add_chat(
        env.store.conn(),
        ChatSpec {
            last_activity: old,
            ..chat("a", &r.wt, "feat/retry")
        },
    );
    let b = place(&env, "a").unwrap();
    // created_at is NOW in this test, so backdate the block too.
    env.store
        .conn()
        .execute(
            "UPDATE blocks SET created_at = ?1 WHERE id = ?2",
            params![old, b],
        )
        .unwrap();
    apply_prs(
        env.store.conn(),
        &env.config,
        "acme/shop",
        &[pr(42, "Add retry backoff", "feat/retry", "MERGED")],
        NOW,
    )
    .unwrap();
    let archived = |c: &Connection| -> bool {
        c.query_row("SELECT archived FROM blocks WHERE id = ?1", [b], |r| {
            r.get(0)
        })
        .unwrap()
    };
    assert!(archived(env.store.conn()));
    env.store
        .conn()
        .execute("UPDATE chats SET last_activity = ?1 WHERE id = 'a'", [NOW])
        .unwrap();
    place(&env, "a");
    assert!(!archived(env.store.conn()));
}

#[test]
fn ticket_keys_become_jira_links_and_manual_titles_win() {
    let mut env = Env::new();
    let r = setup(&mut env);
    env.config.tickets.url_template = Some("https://example.atlassian.net/browse/{key}".into());
    add_chat(
        env.store.conn(),
        ChatSpec {
            prompt: Some("ABC-12"),
            ..chat("t", &r.root, "main")
        },
    );
    let b = place(&env, "t").unwrap();
    let c = env.store.conn();
    let (url, title, source): (String, String, String) = c
        .query_row(
            "SELECT url, title, source FROM links WHERE block_id = ?1",
            [b],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(url, "https://example.atlassian.net/browse/ABC-12");
    assert_eq!(title, "ABC-12");
    assert_eq!(source, "auto");

    c.execute(
        "UPDATE links SET title = 'Retry ticket', source = 'manual' WHERE block_id = ?1",
        [b],
    )
    .unwrap();
    refresh_block(c, &env.config, b, NOW).unwrap();
    let title: String = c
        .query_row("SELECT title FROM links WHERE block_id = ?1", [b], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(title, "Retry ticket");
}
