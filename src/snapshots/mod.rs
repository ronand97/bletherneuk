//! Snapshots: git trees recording a worktree's code at one moment (design §13).
//!
//! Snapshot data goes into a bare "shadow store" per repo, owned by bletherneuk, with
//! the user repo's object database attached read-only as an alternate. Nothing is
//! ever written to the user's repo, and no `git` command ever runs inside it.

pub mod diff;
pub mod prune;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use git2::{Oid, Repository, RepositoryOpenFlags};
use serde::{Deserialize, Serialize};

/// Limits for one snapshot (design §13 and §20.9). Tests pass these explicitly.
#[derive(Debug, Clone)]
pub struct Limits {
    pub new_file_max: u64,
    pub tracked_max: u64,
    pub max_new_data: u64,
    pub max_files: u64,
    pub max_duration: Duration,
    pub skip_patterns: Vec<String>,
    pub output_extensions: Vec<String>,
}

impl Limits {
    pub fn from_config(c: &crate::config::Config) -> Limits {
        const MB: u64 = 1024 * 1024;
        Limits {
            new_file_max: c.snapshots.new_file_max_mb * MB,
            tracked_max: c.snapshots.tracked_max_mb * MB,
            max_new_data: c.snapshots.max_new_data_mb * MB,
            max_files: c.snapshots.max_files,
            max_duration: Duration::from_secs(c.snapshots.max_seconds),
            skip_patterns: c.snapshots.skip_patterns.clone(),
            output_extensions: c.outputs.extensions.clone(),
        }
    }

    pub fn is_secret(&self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.skip_patterns.iter().any(|p| glob_match(p, name))
    }
}

/// What a snapshot left out, shown under the diff as "Not in this diff".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    /// Tracked files that changed but were too large; they keep their previous content.
    pub too_large: Vec<String>,
    /// Files whose names look like secrets.
    pub secrets: Vec<String>,
    /// New files a chat wrote that were left out, with why ("output type", "over 1 MB",
    /// "ignored", "inside .git").
    pub new_left_out: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct Taken {
    pub tree: Oid,
    pub skipped: Skipped,
}

#[derive(Debug, thiserror::Error)]
pub enum Missed {
    #[error("not a git repo")]
    NotARepo,
    #[error("over the snapshot budget ({0})")]
    OverBudget(&'static str),
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A short, stable folder name for a repo's shadow store.
pub fn repo_id(repo_root: &Path) -> String {
    let oid = Oid::hash_object(
        git2::ObjectType::Blob,
        repo_root.to_string_lossy().as_bytes(),
    )
    .expect("hashing never fails");
    oid.to_string()[..16].to_string()
}

pub struct Shadow {
    pub repo: Repository,
    pub path: PathBuf,
}

impl Shadow {
    /// Opens (creating if needed) the shadow store for the repo at `repo_root`.
    pub fn open(snapshots_dir: &Path, repo_root: &Path) -> anyhow::Result<Shadow> {
        crate::paths::ensure_private_dir(snapshots_dir)?;
        let path = snapshots_dir.join(format!("{}.git", repo_id(repo_root)));
        if !path.exists() {
            Repository::init_bare(&path)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        let user = open_user(repo_root).ok_or_else(|| anyhow::anyhow!("not a git repo"))?;
        let objects = user.commondir().join("objects");
        let objects = objects.canonicalize().unwrap_or(objects);
        let alternates = path.join("objects/info/alternates");
        let want = format!("{}\n", objects.display());
        if std::fs::read_to_string(&alternates).ok().as_deref() != Some(want.as_str()) {
            std::fs::create_dir_all(alternates.parent().expect("has parent"))?;
            std::fs::write(&alternates, want).context("writing alternates")?;
        }
        let repo = Repository::open_ext(
            &path,
            RepositoryOpenFlags::NO_SEARCH | RepositoryOpenFlags::BARE,
            std::iter::empty::<&Path>(),
        )?;
        Ok(Shadow { repo, path })
    }

    /// Records a worktree's code: every tracked file, plus `chat_files` (untracked files
    /// chats wrote) that pass the rules. `previous` is this worktree's last snapshot,
    /// used to keep large files' previous content and earlier chat-written files.
    pub fn take(
        &self,
        worktree: &Path,
        chat_files: &[PathBuf],
        previous: Option<Oid>,
        limits: &Limits,
    ) -> Result<Taken, Missed> {
        let started = Instant::now();
        let user = open_user(worktree).ok_or(Missed::NotARepo)?;
        let user_index = user.path().join("index");
        let mut skipped = Skipped::default();

        // A temporary copy of the worktree's own index, so files git already knows are
        // not hashed again.
        let tmp_dir = self.path.join("tmp");
        std::fs::create_dir_all(&tmp_dir)?;
        let tmp_index = tmp_dir.join(format!(
            "index-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _cleanup = RemoveOnDrop(tmp_index.clone());
        let mut index = if user_index.exists() {
            std::fs::copy(&user_index, &tmp_index)?;
            git2::Index::open(&tmp_index)?
        } else {
            git2::Index::new()?
        };
        // Work against the shadow store with the worktree as its working directory.
        self.repo.set_workdir(worktree, false)?;
        self.repo.set_index(&mut index)?;
        let mut index = self.repo.index()?;

        if index.len() as u64 > limits.max_files {
            return Err(Missed::OverBudget("too many files"));
        }

        // Update tracked files that changed.
        let mut too_large = Vec::new();
        let mut new_data = 0u64;
        let mut over: Option<&'static str> = None;
        {
            let mut cb = |path: &Path, _spec: &[u8]| -> i32 {
                if started.elapsed() > limits.max_duration {
                    over = Some("time");
                    return -1;
                }
                let rel = path.to_string_lossy().into_owned();
                if limits.is_secret(&rel) {
                    return 1;
                }
                if let Ok(meta) = std::fs::symlink_metadata(worktree.join(path)) {
                    if meta.is_file() && meta.len() > limits.tracked_max {
                        too_large.push(rel);
                        return 1;
                    }
                    new_data += meta.len();
                    if new_data > limits.max_new_data {
                        over = Some("new data");
                        return -1;
                    }
                }
                0
            };
            let r = index.update_all(["*"].iter(), Some(&mut cb as &mut git2::IndexMatchedPath));
            if let Some(why) = over {
                return Err(Missed::OverBudget(why));
            }
            r?;
        }
        let prev_tree = previous.and_then(|p| self.repo.find_tree(p).ok());
        for rel in &too_large {
            if let Some(t) = &prev_tree
                && let Ok(e) = t.get_path(Path::new(rel))
                && let Some(mut entry) = index.get_path(Path::new(rel), 0)
            {
                entry.id = e.id();
                index.add(&entry)?;
            }
        }
        skipped.too_large = too_large;

        // New files: those chats wrote, plus untracked files already in the previous snapshot.
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        for p in chat_files {
            match p.strip_prefix(worktree) {
                Ok(rel) => {
                    candidates.insert(rel.to_string_lossy().into_owned());
                }
                Err(_) => continue,
            }
        }
        if let Some(t) = &prev_tree {
            t.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
                if entry.kind() == Some(git2::ObjectType::Blob) {
                    let rel = format!("{dir}{}", entry.name().unwrap_or_default());
                    if index.get_path(Path::new(&rel), 0).is_none() {
                        candidates.insert(rel);
                    }
                }
                git2::TreeWalkResult::Ok
            })?;
        }
        for rel in candidates {
            if started.elapsed() > limits.max_duration {
                return Err(Missed::OverBudget("time"));
            }
            let rel_path = Path::new(&rel);
            if index.get_path(rel_path, 0).is_some() {
                continue; // tracked, so already covered
            }
            if rel.starts_with(".git/") || rel == ".git" || rel.contains("/.git/") {
                skipped.new_left_out.push((rel, "inside .git".into()));
                continue;
            }
            let full = worktree.join(rel_path);
            let Ok(meta) = std::fs::symlink_metadata(&full) else {
                continue; // deleted
            };
            if !meta.is_file() {
                continue;
            }
            if user.is_path_ignored(rel_path).unwrap_or(false) {
                skipped.new_left_out.push((rel, "ignored".into()));
                continue;
            }
            if limits.is_secret(&rel) {
                skipped.secrets.push(rel);
                continue;
            }
            let ext = crate::files::extension(rel_path).unwrap_or_default();
            if limits.output_extensions.contains(&ext) {
                skipped.new_left_out.push((rel, "output type".into()));
                continue;
            }
            if meta.len() > limits.new_file_max {
                skipped.new_left_out.push((rel, "too large".into()));
                continue;
            }
            new_data += meta.len();
            if new_data > limits.max_new_data {
                return Err(Missed::OverBudget("new data"));
            }
            index.add_path(rel_path)?;
            if index.len() as u64 > limits.max_files {
                return Err(Missed::OverBudget("too many files"));
            }
        }

        // Secret-looking files never enter a snapshot.
        let secret_paths: Vec<String> = index
            .iter()
            .map(|e| String::from_utf8_lossy(&e.path).into_owned())
            .filter(|p| limits.is_secret(p))
            .collect();
        for p in &secret_paths {
            index.remove_path(Path::new(p))?;
        }
        skipped.secrets.extend(secret_paths);
        skipped.secrets.sort();
        skipped.secrets.dedup();

        let tree = index.write_tree_to(&self.repo)?;
        Ok(Taken { tree, skipped })
    }

    /// Keeps a snapshot's tree alive with `refs/snap/<id>`.
    pub fn keep(&self, snapshot_id: i64, tree: Oid) -> Result<(), git2::Error> {
        self.repo
            .reference(&format!("refs/snap/{snapshot_id}"), tree, true, "snapshot")?;
        Ok(())
    }

    pub fn forget(&self, snapshot_id: i64) -> Result<(), git2::Error> {
        if let Ok(mut r) = self
            .repo
            .find_reference(&format!("refs/snap/{snapshot_id}"))
        {
            r.delete()?;
        }
        Ok(())
    }

    pub fn has_tree(&self, tree: Oid) -> bool {
        self.repo.find_tree(tree).is_ok()
    }

    /// The tree of the merge base between a worktree's `HEAD` and the default branch,
    /// for "whole branch" diffs.
    pub fn merge_base_tree(&self, worktree: &Path, default_branch: &str) -> Option<Oid> {
        let user = open_user(worktree)?;
        let head = user.head().ok()?.target()?;
        let base = user
            .find_reference(&format!("refs/remotes/origin/{default_branch}"))
            .or_else(|_| user.find_reference(&format!("refs/heads/{default_branch}")))
            .ok()?
            .target()?;
        let mb = user.merge_base(head, base).ok()?;
        Some(user.find_commit(mb).ok()?.tree_id())
    }
}

fn open_user(path: &Path) -> Option<Repository> {
    Repository::open_ext(
        path,
        RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )
    .ok()
}

struct RemoveOnDrop(PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Shell-style matching of a file name against `*` and `?` patterns.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

#[cfg(test)]
mod tests;
