//! Branches and worktrees, read through libgit2 (design §9).
//!
//! libgit2 never runs hooks, fsmonitor, or external filter, textconv or diff programs.
//! Its check that a repo is owned by the current user stays on, so repos owned by
//! another account fail to open and are skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use git2::{BranchType, Repository, RepositoryOpenFlags, StatusOptions};

/// Where a path sits in git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The worktree's top folder.
    pub worktree: PathBuf,
    /// The main checkout's top folder, shared by all worktrees of the repo.
    pub repo_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    Detached(String),
    Unborn,
}

impl Head {
    pub fn branch(&self) -> Option<&str> {
        match self {
            Head::Branch(b) => Some(b),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoInfo {
    /// `owner/repo` of the `origin` remote, when it is on GitHub.
    pub github: Option<String>,
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub name: String,
    pub head: Head,
    pub removed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    Exact(u32),
    /// Over the time budget.
    Many,
}

/// Opens the repo at `path` (no searching), after checking its git files are safe to read.
pub fn open(path: &Path) -> Option<Repository> {
    if !metadata_is_safe(path, false) {
        return None;
    }
    Repository::open_ext(
        path,
        RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )
    .ok()
}

/// Opens the repo containing `path`, searching upwards from the nearest folder that exists.
fn discover(path: &Path) -> Option<Repository> {
    let mut dir = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };
    while let Some(d) = dir {
        if d.exists() {
            if !metadata_is_safe(d, true) {
                return None;
            }
            return Repository::discover(d).ok();
        }
        dir = d.parent();
    }
    None
}

/// True if `p` is missing or a regular file (following symlinks). A FIFO, socket or
/// device would make libgit2 block forever when it reads it.
fn file_ok(p: &Path) -> bool {
    match std::fs::metadata(p) {
        Err(_) => true,
        Ok(m) => m.is_file(),
    }
}

fn read_small(p: &Path) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(p).ok()?;
    let mut s = String::new();
    f.take(4096).read_to_string(&mut s).ok()?;
    Some(s)
}

const GIT_DIR_FILES: &[&str] = &[
    "HEAD",
    "config",
    "index",
    "commondir",
    "gitdir",
    "packed-refs",
    "ORIG_HEAD",
    "FETCH_HEAD",
    "MERGE_HEAD",
    "shallow",
    "info/exclude",
    "info/attributes",
    "info/grafts",
    "objects/info/alternates",
];

fn git_dir_ok(g: &Path) -> bool {
    if !GIT_DIR_FILES.iter().all(|f| file_ok(&g.join(f))) {
        return false;
    }
    let common = match read_small(&g.join("commondir")) {
        Some(c) => {
            let c = Path::new(c.trim());
            if c.is_absolute() {
                c.to_path_buf()
            } else {
                g.join(c)
            }
        }
        None => g.to_path_buf(),
    };
    if common != g && !GIT_DIR_FILES.iter().all(|f| file_ok(&common.join(f))) {
        return false;
    }
    // The branch HEAD points at.
    if let Some(head) = read_small(&g.join("HEAD"))
        && let Some(target) = head.trim().strip_prefix("ref: ")
    {
        if target.contains("..") {
            return false;
        }
        return file_ok(&g.join(target)) && file_ok(&common.join(target));
    }
    true
}

/// Checks the git metadata libgit2 will read for the repo at (or, with `search`, above)
/// `path`. A downloaded repo could ship `.git/index` or `.git/HEAD` as a FIFO, which
/// would otherwise hang the service.
pub fn metadata_is_safe(path: &Path, search: bool) -> bool {
    let mut dir = Some(path);
    while let Some(d) = dir {
        let dotgit = d.join(".git");
        if let Ok(m) = std::fs::metadata(&dotgit) {
            if m.is_dir() {
                return git_dir_ok(&dotgit)
                    && file_ok(&d.join(".gitattributes"))
                    && file_ok(&d.join(".gitignore"));
            }
            if !m.is_file() {
                return false;
            }
            // A linked worktree: `.git` is a file naming the real git folder.
            let Some(text) = read_small(&dotgit) else {
                return false;
            };
            let Some(target) = text.trim().strip_prefix("gitdir: ") else {
                return false;
            };
            let target = Path::new(target);
            let g = if target.is_absolute() {
                target.to_path_buf()
            } else {
                d.join(target)
            };
            return git_dir_ok(&g)
                && file_ok(&d.join(".gitattributes"))
                && file_ok(&d.join(".gitignore"));
        }
        if std::fs::symlink_metadata(&dotgit).is_ok() {
            return false; // a dangling or looping symlink
        }
        // A bare repo, or a git folder itself.
        if d.join("objects").is_dir() && std::fs::symlink_metadata(d.join("HEAD")).is_ok() {
            return git_dir_ok(d);
        }
        if !search {
            return true;
        }
        dir = d.parent();
    }
    true
}

/// The main checkout of a repo, from its common git folder.
fn main_root(repo: &Repository) -> Option<PathBuf> {
    let common = repo.commondir();
    let common = common
        .canonicalize()
        .unwrap_or_else(|_| common.to_path_buf());
    if common.file_name()? == ".git" {
        common.parent().map(Path::to_path_buf)
    } else {
        // A bare repo with worktrees has no main checkout.
        None
    }
}

/// Finds the worktree and repo for a path. Caches results per folder.
#[derive(Default)]
pub struct Locator {
    cache: HashMap<PathBuf, Option<Location>>,
}

impl Locator {
    pub fn locate(&mut self, path: &Path) -> Option<Location> {
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()?.to_path_buf()
        };
        if let Some(hit) = self.cache.get(&dir) {
            return hit.clone();
        }
        let found = locate_uncached(&dir);
        self.cache.insert(dir, found.clone());
        found
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

fn locate_uncached(path: &Path) -> Option<Location> {
    let repo = discover(path)?;
    let workdir = repo.workdir()?;
    let worktree = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    let worktree = trim_slash(worktree);
    let repo_root = main_root(&repo)?;
    Some(Location {
        worktree,
        repo_root,
    })
}

fn trim_slash(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if s.len() > 1 && s.ends_with('/') {
        PathBuf::from(s.trim_end_matches('/'))
    } else {
        p
    }
}

pub fn head_of(worktree: &Path) -> Option<Head> {
    let repo = open(worktree)?;
    Some(read_head(&repo))
}

fn read_head(repo: &Repository) -> Head {
    match repo.head() {
        Ok(h) if h.is_branch() => {
            Head::Branch(String::from_utf8_lossy(h.shorthand_bytes()).into_owned())
        }
        Ok(h) => Head::Detached(
            h.target()
                .map(|o| o.to_string()[..7].to_string())
                .unwrap_or_default(),
        ),
        Err(_) => Head::Unborn,
    }
}

pub fn repo_info(repo_root: &Path) -> Option<RepoInfo> {
    let repo = open(repo_root)?;
    let github = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().ok().and_then(github_repo_from_url));
    Some(RepoInfo {
        github,
        default_branch: default_branch(&repo),
    })
}

/// The default branch: `origin/HEAD`, then `main`, then `master`.
fn default_branch(repo: &Repository) -> Option<String> {
    if let Ok(r) = repo.find_reference("refs/remotes/origin/HEAD")
        && let Ok(Some(target)) = r.symbolic_target()
        && let Some(name) = target.strip_prefix("refs/remotes/origin/")
    {
        return Some(name.to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|b| repo.find_branch(b, BranchType::Local).is_ok())
        .map(str::to_string)
}

/// `owner/repo` from a GitHub remote URL in any of its usual forms.
pub fn github_repo_from_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git://github.com/"))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    crate::links::is_github_repo(rest).then(|| rest.to_string())
}

/// The main checkout plus every linked worktree of a repo.
pub fn list(repo_root: &Path) -> Vec<Worktree> {
    let Some(repo) = open(repo_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(wd) = repo.workdir() {
        let path = trim_slash(wd.to_path_buf());
        out.push(Worktree {
            name: folder_name(&path),
            head: read_head(&repo),
            path,
            removed: false,
        });
    }
    if let Ok(names) = repo.worktrees() {
        for name in names.iter().filter_map(|n| n.ok().flatten()) {
            let Ok(wt) = repo.find_worktree(name) else {
                continue;
            };
            let path = trim_slash(wt.path().to_path_buf());
            let path = path.canonicalize().unwrap_or(path);
            match open(&path) {
                Some(r) => out.push(Worktree {
                    name: folder_name(&path),
                    head: read_head(&r),
                    path,
                    removed: false,
                }),
                None => out.push(Worktree {
                    name: folder_name(&path),
                    head: Head::Unborn,
                    path,
                    removed: true,
                }),
            }
        }
    }
    out
}

fn folder_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Counts uncommitted files (changed, staged or untracked, but not ignored), giving up
/// after `budget`.
pub fn uncommitted(worktree: &Path, budget: Duration) -> Option<Count> {
    let repo = open(worktree)?;
    let start = Instant::now();
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .exclude_submodules(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).ok()?;
    if start.elapsed() > budget {
        return Some(Count::Many);
    }
    Some(Count::Exact(statuses.len() as u32))
}

/// Commits the worktree's `HEAD` is ahead of and behind the default branch.
pub fn ahead_behind(worktree: &Path, default_branch: &str) -> Option<(usize, usize)> {
    let repo = open(worktree)?;
    let head = repo.head().ok()?.target()?;
    let base = repo
        .find_reference(&format!("refs/remotes/origin/{default_branch}"))
        .or_else(|_| repo.find_reference(&format!("refs/heads/{default_branch}")))
        .ok()?
        .target()?;
    repo.graph_ahead_behind(head, base).ok()
}

/// The git folder whose `HEAD` and `index` describe a worktree (for watching).
pub fn git_dir(worktree: &Path) -> Option<PathBuf> {
    open(worktree).map(|r| r.path().to_path_buf())
}

#[cfg(test)]
pub(crate) mod testrepo {
    //! Builds real repos and worktrees with libgit2 for tests.
    use super::*;
    use git2::{Signature, WorktreeAddOptions};

    pub fn commit_all(repo: &Repository, msg: &str) -> git2::Oid {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::new(
            "Test",
            "test@example.com",
            &git2::Time::new(1_700_000_000, 0),
        )
        .unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    /// A repo at `dir` on `main` with one commit.
    pub fn init(dir: &Path) -> Repository {
        std::fs::create_dir_all(dir).unwrap();
        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(dir, &opts).unwrap();
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        commit_all(&repo, "first");
        repo
    }

    /// Adds a worktree at `path` on a new branch `branch`.
    pub fn add_worktree(repo: &Repository, name: &str, path: &Path, branch: &str) -> Repository {
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let b = repo.branch(branch, &head, false).unwrap();
        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(b.get()));
        repo.worktree(name, path, Some(&opts)).unwrap();
        Repository::open(path).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testrepo::*;
    use super::*;

    fn canon(p: &Path) -> PathBuf {
        p.canonicalize().unwrap()
    }

    #[test]
    fn locates_main_checkout_and_linked_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        let repo = init(&root);
        let wt_path = root.join(".claude/worktrees/retry");
        std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
        add_worktree(&repo, "retry", &wt_path, "feat/retry");

        let mut loc = Locator::default();
        let main = loc.locate(&root.join("src/new_file.py")).unwrap();
        assert_eq!(main.worktree, root);
        assert_eq!(main.repo_root, root);
        let linked = loc.locate(&wt_path.join("README.md")).unwrap();
        assert_eq!(linked.worktree, wt_path);
        assert_eq!(linked.repo_root, root);
        assert_eq!(loc.locate(&canon(dir.path()).join("elsewhere/x.md")), None);
    }

    #[test]
    fn reads_branches_detached_heads_and_the_default_branch() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        let repo = init(&root);
        assert_eq!(head_of(&root), Some(Head::Branch("main".into())));
        let info = repo_info(&root).unwrap();
        assert_eq!(info.default_branch.as_deref(), Some("main"));
        assert_eq!(info.github, None);

        repo.remote("origin", "git@github.com:acme/shop.git")
            .unwrap();
        assert_eq!(
            repo_info(&root).unwrap().github.as_deref(),
            Some("acme/shop")
        );

        let head = repo.head().unwrap().target().unwrap();
        repo.set_head_detached(head).unwrap();
        assert_eq!(
            head_of(&root),
            Some(Head::Detached(head.to_string()[..7].to_string()))
        );
    }

    #[test]
    fn origin_head_wins_for_the_default_branch() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        let repo = init(&root);
        let head = repo.head().unwrap().target().unwrap();
        repo.reference("refs/remotes/origin/develop", head, true, "test")
            .unwrap();
        repo.reference_symbolic(
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
            true,
            "test",
        )
        .unwrap();
        assert_eq!(
            repo_info(&root).unwrap().default_branch.as_deref(),
            Some("develop")
        );
    }

    #[test]
    fn lists_worktrees_and_notices_removed_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        let repo = init(&root);
        let a = root.join(".claude/worktrees/a");
        let b = root.join(".claude/worktrees/b");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        add_worktree(&repo, "a", &a, "feat/a");
        add_worktree(&repo, "b", &b, "feat/b");
        std::fs::remove_dir_all(&b).unwrap();

        let list = list(&root);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].path, root);
        assert_eq!(list[0].head, Head::Branch("main".into()));
        let wa = list.iter().find(|w| w.name == "a").unwrap();
        assert_eq!(wa.head, Head::Branch("feat/a".into()));
        assert!(!wa.removed);
        let wb = list.iter().find(|w| w.name == "b").unwrap();
        assert!(wb.removed);
    }

    #[test]
    fn counts_uncommitted_files_and_commits_ahead() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        let repo = init(&root);
        let wt = root.join(".claude/worktrees/x");
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let wrepo = add_worktree(&repo, "x", &wt, "feat/x");
        std::fs::write(wt.join("a.txt"), "a").unwrap();
        commit_all(&wrepo, "one");
        std::fs::write(wt.join("b.txt"), "b").unwrap();
        commit_all(&wrepo, "two");
        std::fs::write(wt.join("README.md"), "changed\n").unwrap();
        std::fs::write(wt.join("new.txt"), "new").unwrap();
        // The main checkout ignores .claude/, as Claude Code sets up.
        std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();

        assert_eq!(
            uncommitted(&wt, Duration::from_secs(2)),
            Some(Count::Exact(2))
        );
        assert_eq!(ahead_behind(&wt, "main"), Some((2, 0)));
        assert_eq!(
            uncommitted(&wt, Duration::ZERO),
            Some(Count::Many),
            "a zero budget is always exceeded"
        );
    }

    fn mkfifo(p: &Path) {
        let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    }

    #[test]
    fn repos_with_fifos_in_their_git_files_are_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        init(&root);
        assert!(metadata_is_safe(&root, false));

        std::fs::remove_file(root.join(".git/index")).unwrap();
        mkfifo(&root.join(".git/index"));
        assert!(!metadata_is_safe(&root, false));
        assert!(!metadata_is_safe(&root.join("src/deep"), true));
        let mut loc = Locator::default();
        assert_eq!(loc.locate(&root.join("README.md")), None);
        assert_eq!(head_of(&root), None);
        assert_eq!(uncommitted(&root, Duration::from_secs(1)), None);
    }

    #[test]
    fn a_fifo_branch_ref_or_ignore_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        init(&root);
        let head_ref = root.join(".git/refs/heads/main");
        std::fs::remove_file(&head_ref).unwrap();
        mkfifo(&head_ref);
        assert!(!metadata_is_safe(&root, false));

        let other = canon(dir.path()).join("other");
        init(&other);
        mkfifo(&other.join(".gitignore"));
        assert!(!metadata_is_safe(&other, false));
    }

    #[test]
    fn linked_worktrees_are_checked_through_their_gitdir_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = canon(dir.path()).join("shop");
        let repo = init(&root);
        let wt = root.join(".claude/worktrees/x");
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        add_worktree(&repo, "x", &wt, "feat/x");
        assert!(metadata_is_safe(&wt, false));
        let index = root.join(".git/worktrees/x/index");
        let _ = std::fs::remove_file(&index);
        mkfifo(&index);
        assert!(!metadata_is_safe(&wt, false));
    }

    #[test]
    fn github_urls() {
        for (url, want) in [
            ("git@github.com:acme/shop.git", Some("acme/shop")),
            ("https://github.com/acme/shop", Some("acme/shop")),
            ("https://github.com/acme/shop.git/", Some("acme/shop")),
            ("ssh://git@github.com/acme/shop.git", Some("acme/shop")),
            ("https://gitlab.com/acme/shop.git", None),
            ("git@github.com:acme/shop;x.git", None),
        ] {
            assert_eq!(github_repo_from_url(url).as_deref(), want, "{url}");
        }
    }
}
