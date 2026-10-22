use std::path::{Path, PathBuf};
use std::time::Duration;

use git2::Repository;

use super::diff::diff_trees;
use super::prune;
use super::*;
use crate::worktrees::testrepo;

fn limits() -> Limits {
    Limits {
        new_file_max: 1024,
        tracked_max: 2048,
        max_new_data: 10 * 1024 * 1024,
        max_files: 10_000,
        max_duration: Duration::from_secs(30),
        skip_patterns: vec![".env*".into(), "*.pem".into()],
        output_extensions: vec!["html".into(), "csv".into()],
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    snaps: PathBuf,
    repo: Repository,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let root = base.join("shop");
    let repo = testrepo::init(&root);
    std::fs::write(root.join("app.py"), "a = 1\nb = 2\nc = 3\n").unwrap();
    std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
    testrepo::commit_all(&repo, "app");
    Fixture {
        _dir: dir,
        snaps: base.join("snapshots"),
        root,
        repo,
    }
}

fn files_in(shadow: &Shadow, tree: git2::Oid) -> Vec<String> {
    let mut out = Vec::new();
    shadow
        .repo
        .find_tree(tree)
        .unwrap()
        .walk(git2::TreeWalkMode::PreOrder, |dir, e| {
            if e.kind() == Some(git2::ObjectType::Blob) {
                out.push(format!("{dir}{}", e.name().unwrap()));
            }
            git2::TreeWalkResult::Ok
        })
        .unwrap();
    out.sort();
    out
}

fn content(shadow: &Shadow, tree: git2::Oid, path: &str) -> String {
    let t = shadow.repo.find_tree(tree).unwrap();
    let e = t.get_path(Path::new(path)).unwrap();
    String::from_utf8(shadow.repo.find_blob(e.id()).unwrap().content().to_vec()).unwrap()
}

#[test]
fn snapshots_tracked_files_at_their_current_content() {
    let f = fixture();
    std::fs::write(f.root.join("app.py"), "a = 1\nb = 20\nc = 3\n").unwrap();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t = shadow.take(&f.root, &[], None, &limits()).unwrap();
    assert_eq!(
        files_in(&shadow, t.tree),
        vec![".gitignore", "README.md", "app.py"]
    );
    assert_eq!(content(&shadow, t.tree, "app.py"), "a = 1\nb = 20\nc = 3\n");
}

#[test]
fn includes_chat_written_new_files_by_the_rules() {
    let f = fixture();
    let w = |rel: &str, size: usize| {
        let p = f.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![b'x'; size]).unwrap();
        p
    };
    let chat_files = vec![
        w("src/new.py", 10),
        w("out/report.html", 10),
        w("big.txt", 5000),
        w("build/gen.py", 10),
        w(".env.local", 10),
        f.root.join("deleted.py"),
        PathBuf::from("/elsewhere/x.py"),
    ];
    w("untracked_by_script.py", 10);
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t = shadow.take(&f.root, &chat_files, None, &limits()).unwrap();
    assert_eq!(
        files_in(&shadow, t.tree),
        vec![".gitignore", "README.md", "app.py", "src/new.py"]
    );
    assert_eq!(t.skipped.secrets, vec![".env.local"]);
    let mut left: Vec<(String, String)> = t.skipped.new_left_out.clone();
    left.sort();
    assert_eq!(
        left,
        vec![
            ("big.txt".into(), "too large".into()),
            ("build/gen.py".into(), "ignored".into()),
            ("out/report.html".into(), "output type".into()),
        ]
    );
}

#[test]
fn chat_written_files_stay_in_later_snapshots() {
    let f = fixture();
    let new = f.root.join("src/new.py");
    std::fs::create_dir_all(new.parent().unwrap()).unwrap();
    std::fs::write(&new, "v1").unwrap();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t1 = shadow
        .take(&f.root, std::slice::from_ref(&new), None, &limits())
        .unwrap();
    std::fs::write(&new, "v2").unwrap();
    let t2 = shadow.take(&f.root, &[], Some(t1.tree), &limits()).unwrap();
    assert_eq!(content(&shadow, t2.tree, "src/new.py"), "v2");
    std::fs::remove_file(&new).unwrap();
    let t3 = shadow.take(&f.root, &[], Some(t2.tree), &limits()).unwrap();
    assert!(!files_in(&shadow, t3.tree).contains(&"src/new.py".to_string()));
}

#[test]
fn large_tracked_files_keep_their_previous_content() {
    let f = fixture();
    std::fs::write(f.root.join("data.txt"), "small").unwrap();
    testrepo::commit_all(&f.repo, "data");
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    std::fs::write(f.root.join("data.txt"), "edited but small").unwrap();
    let t1 = shadow.take(&f.root, &[], None, &limits()).unwrap();
    std::fs::write(f.root.join("data.txt"), vec![b'y'; 4096]).unwrap();
    let t2 = shadow.take(&f.root, &[], Some(t1.tree), &limits()).unwrap();
    assert_eq!(content(&shadow, t2.tree, "data.txt"), "edited but small");
    assert_eq!(t2.skipped.too_large, vec!["data.txt"]);
}

#[test]
fn tracked_secret_files_are_left_out() {
    let f = fixture();
    std::fs::write(f.root.join("server.pem"), "key").unwrap();
    testrepo::commit_all(&f.repo, "pem");
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t = shadow.take(&f.root, &[], None, &limits()).unwrap();
    assert!(!files_in(&shadow, t.tree).contains(&"server.pem".to_string()));
    assert_eq!(t.skipped.secrets, vec!["server.pem"]);
}

#[test]
fn the_user_repo_is_never_written_to() {
    let f = fixture();
    std::fs::write(f.root.join("app.py"), "changed\n").unwrap();
    let new = f.root.join("new.py");
    std::fs::write(&new, "new\n").unwrap();
    let git = f.root.join(".git");
    let index_before = std::fs::read(git.join("index")).unwrap();
    let count_objects = || walkdir(&git.join("objects"));
    let objects_before = count_objects();
    let refs_before: Vec<String> = f
        .repo
        .references()
        .unwrap()
        .map(|r| r.unwrap().name().unwrap().to_string())
        .collect();

    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t = shadow.take(&f.root, &[new], None, &limits()).unwrap();
    shadow.keep(1, t.tree).unwrap();

    assert_eq!(std::fs::read(git.join("index")).unwrap(), index_before);
    assert_eq!(count_objects(), objects_before);
    let refs_after: Vec<String> = f
        .repo
        .references()
        .unwrap()
        .map(|r| r.unwrap().name().unwrap().to_string())
        .collect();
    assert_eq!(refs_after, refs_before);
}

fn walkdir(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| {
                    if e.file_type().unwrap().is_dir() {
                        walkdir(&e.path())
                    } else {
                        1
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

#[test]
fn works_in_a_linked_worktree() {
    let f = fixture();
    let wt = f.root.join(".claude/worktrees/x");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    testrepo::add_worktree(&f.repo, "x", &wt, "feat/x");
    std::fs::write(wt.join("app.py"), "worktree edit\n").unwrap();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t = shadow.take(&wt, &[], None, &limits()).unwrap();
    assert_eq!(content(&shadow, t.tree, "app.py"), "worktree edit\n");
}

#[test]
fn over_budget_snapshots_are_missed() {
    let f = fixture();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let tight = Limits {
        max_files: 1,
        ..limits()
    };
    assert!(matches!(
        shadow.take(&f.root, &[], None, &tight),
        Err(Missed::OverBudget("too many files"))
    ));
    let not_repo = f.snaps.join("nope");
    std::fs::create_dir_all(&not_repo).unwrap();
    assert!(matches!(
        shadow.take(&not_repo, &[], None, &limits()),
        Err(Missed::NotARepo)
    ));
}

#[test]
fn diffs_between_snapshots() {
    let f = fixture();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t1 = shadow.take(&f.root, &[], None, &limits()).unwrap();
    std::fs::write(f.root.join("app.py"), "a = 1\nb = 22\nc = 3\nd = 4\n").unwrap();
    std::fs::rename(f.root.join("README.md"), f.root.join("README.txt")).unwrap();
    let new = f.root.join("lib.py");
    std::fs::write(&new, "x = 1\n").unwrap();
    let readme = f.root.join("README.txt");
    let t2 = shadow
        .take(&f.root, &[new, readme], Some(t1.tree), &limits())
        .unwrap();

    let files = diff_trees(&shadow.repo, Some(t1.tree), t2.tree, false).unwrap();
    let by_path = |p: &str| {
        files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(p))
            .unwrap()
            .clone()
    };
    let app = by_path("app.py");
    assert_eq!(app.status, "modified");
    assert_eq!((app.additions, app.deletions), (2, 1));
    let lines: Vec<(char, Option<u32>, Option<u32>, &str)> = app.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.old, l.new, l.text.as_str()))
        .collect();
    assert_eq!(
        lines,
        vec![
            (' ', Some(1), Some(1), "a = 1"),
            ('-', Some(2), None, "b = 2"),
            ('+', None, Some(2), "b = 22"),
            (' ', Some(3), Some(3), "c = 3"),
            ('+', None, Some(4), "d = 4"),
        ]
    );
    let readme = by_path("README.txt");
    assert_eq!(readme.status, "renamed");
    assert_eq!(readme.old_path.as_deref(), Some("README.md"));
    assert_eq!(by_path("lib.py").status, "added");

    let summary = diff_trees(&shadow.repo, Some(t1.tree), t2.tree, true).unwrap();
    assert!(summary.iter().all(|f| f.hunks.is_empty()));
    assert_eq!(diff::totals(&summary).additions, 3);
}

#[test]
fn binary_lockfile_and_large_files_are_flagged() {
    let f = fixture();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t1 = shadow.take(&f.root, &[], None, &limits()).unwrap();
    let bin = f.root.join("img.bin");
    std::fs::write(&bin, [0u8, 1, 2, 0, 255]).unwrap();
    let lock = f.root.join("Cargo.lock");
    std::fs::write(&lock, "[[package]]\n").unwrap();
    let t2 = shadow
        .take(&f.root, &[bin, lock], Some(t1.tree), &limits())
        .unwrap();
    let files = diff_trees(&shadow.repo, Some(t1.tree), t2.tree, false).unwrap();
    let get = |p: &str| {
        files
            .iter()
            .find(|f| f.new_path.as_deref() == Some(p))
            .unwrap()
    };
    assert!(get("img.bin").binary);
    assert!(get("img.bin").hunks.is_empty());
    assert_eq!(get("Cargo.lock").collapsed, Some("lockfile"));
}

#[test]
fn whole_branch_diff_starts_at_the_merge_base() {
    let f = fixture();
    let wt = f.root.join(".claude/worktrees/x");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    let wrepo = testrepo::add_worktree(&f.repo, "x", &wt, "feat/x");
    std::fs::write(wt.join("feature.py"), "f\n").unwrap();
    testrepo::commit_all(&wrepo, "feature");
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let base = shadow.merge_base_tree(&wt, "main").unwrap();
    let now = shadow.take(&wt, &[], None, &limits()).unwrap();
    let files = diff_trees(&shadow.repo, Some(base), now.tree, true).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].new_path.as_deref(), Some("feature.py"));
}

#[test]
fn pruning_removes_only_unreferenced_objects() {
    let f = fixture();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let new = f.root.join("only_in_shadow.py");
    std::fs::write(&new, "unique content 1\n").unwrap();
    let t1 = shadow
        .take(&f.root, std::slice::from_ref(&new), None, &limits())
        .unwrap();
    shadow.keep(1, t1.tree).unwrap();
    std::fs::write(&new, "unique content 2\n").unwrap();
    let t2 = shadow
        .take(
            &f.root,
            std::slice::from_ref(&new),
            Some(t1.tree),
            &limits(),
        )
        .unwrap();
    shadow.keep(2, t2.tree).unwrap();

    let before = prune::store_size(&shadow.path);
    shadow.forget(1).unwrap();
    let freed = prune::prune_unreferenced(&shadow, Duration::ZERO).unwrap();
    assert!(freed > 0);
    assert!(prune::store_size(&shadow.path) < before);
    // A fresh handle, so libgit2's in-memory object cache can't answer.
    let reopened = Shadow::open(&f.snaps, &f.root).unwrap();
    assert!(!reopened.has_tree(t1.tree));
    assert!(reopened.has_tree(t2.tree));
    assert_eq!(
        content(&reopened, t2.tree, "only_in_shadow.py"),
        "unique content 2\n"
    );

    // Recent objects survive a prune with a grace period.
    reopened.forget(2).unwrap();
    prune::prune_unreferenced(&reopened, Duration::from_secs(3600)).unwrap();
    assert!(Shadow::open(&f.snaps, &f.root).unwrap().has_tree(t2.tree));
}

#[test]
fn glob_patterns() {
    assert!(glob_match(".env*", ".env"));
    assert!(glob_match(".env*", ".env.production"));
    assert!(glob_match(
        "*credentials*.json",
        "gcp-credentials-prod.json"
    ));
    assert!(glob_match("id_rsa*", "id_rsa.pub"));
    assert!(!glob_match("*.pem", "pem.txt"));
    assert!(glob_match("?.key", "a.key"));
}

/// Design §20.11: a repo configured to run programs must never run them.
#[test]
fn repo_configured_programs_never_run() {
    let f = fixture();
    let marker = f.snaps.parent().unwrap().join("MARKER");
    let script = f.snaps.parent().unwrap().join("evil.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch {}\ncat\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let s = script.display().to_string();
    // The script really does leave a marker when something runs it.
    std::process::Command::new(&script)
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(marker.exists());
    std::fs::remove_file(&marker).unwrap();

    let mut cfg = f.repo.config().unwrap();
    cfg.set_str("core.fsmonitor", &s).unwrap();
    cfg.set_str("filter.evil.clean", &s).unwrap();
    cfg.set_str("filter.evil.smudge", &s).unwrap();
    cfg.set_str("diff.evil.textconv", &s).unwrap();
    cfg.set_str("diff.evil.command", &s).unwrap();
    cfg.set_str("diff.external", &s).unwrap();
    std::fs::write(f.root.join(".gitattributes"), "* filter=evil diff=evil\n").unwrap();
    let hooks = f.root.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for h in ["reference-transaction", "post-index-change", "pre-commit"] {
        std::fs::copy(&script, hooks.join(h)).unwrap();
    }

    std::fs::write(f.root.join("app.py"), "changed\n").unwrap();
    let new = f.root.join("new.py");
    std::fs::write(&new, "new\n").unwrap();
    let shadow = Shadow::open(&f.snaps, &f.root).unwrap();
    let t1 = shadow.take(&f.root, &[], None, &limits()).unwrap();
    shadow.keep(1, t1.tree).unwrap();
    std::fs::write(f.root.join("app.py"), "changed again\n").unwrap();
    let t2 = shadow
        .take(&f.root, &[new], Some(t1.tree), &limits())
        .unwrap();
    shadow.keep(2, t2.tree).unwrap();
    diff_trees(&shadow.repo, Some(t1.tree), t2.tree, false).unwrap();
    crate::worktrees::uncommitted(&f.root, Duration::from_secs(5));
    crate::worktrees::list(&f.root);

    assert!(!marker.exists(), "a repo-configured program ran");
}
