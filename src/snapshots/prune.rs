//! Keeping each shadow store under its size limit (design §13).

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use git2::{ObjectType, Oid, TreeWalkMode, TreeWalkResult};

use super::Shadow;

/// Bytes used by the shadow store's own objects (not the user repo's, which it only reads).
pub fn store_size(shadow_path: &Path) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    walk(&shadow_path.join("objects"))
}

/// Every tree and blob reachable from `refs/snap/*`.
fn reachable(shadow: &Shadow) -> Result<HashSet<Oid>, git2::Error> {
    let mut seen = HashSet::new();
    for r in shadow.repo.references_glob("refs/snap/*")? {
        let Some(oid) = r?.target() else { continue };
        if !seen.insert(oid) {
            continue;
        }
        let Ok(tree) = shadow.repo.find_tree(oid) else {
            continue;
        };
        tree.walk(TreeWalkMode::PreOrder, |_, entry| {
            let new = seen.insert(entry.id());
            // A subtree seen before has already been walked.
            if entry.kind() == Some(ObjectType::Tree) && !new {
                TreeWalkResult::Skip
            } else {
                TreeWalkResult::Ok
            }
        })?;
    }
    Ok(seen)
}

/// Deletes loose objects no snapshot refers to. Objects newer than `grace` are kept, so
/// a snapshot or diff in progress never loses its data.
pub fn prune_unreferenced(shadow: &Shadow, grace: Duration) -> anyhow::Result<u64> {
    let keep = reachable(shadow)?;
    let cutoff = SystemTime::now() - grace;
    let objects = shadow.path.join("objects");
    let mut freed = 0;
    for dir in std::fs::read_dir(&objects)?.flatten() {
        let name = dir.file_name().to_string_lossy().into_owned();
        if name.len() != 2 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        for f in std::fs::read_dir(dir.path())?.flatten() {
            let rest = f.file_name().to_string_lossy().into_owned();
            let Ok(oid) = Oid::from_str(&format!("{name}{rest}")) else {
                continue;
            };
            if keep.contains(&oid) {
                continue;
            }
            let meta = f.metadata()?;
            if meta.modified().is_ok_and(|m| m > cutoff) {
                continue;
            }
            std::fs::remove_file(f.path())?;
            freed += meta.len();
        }
        let _ = std::fs::remove_dir(dir.path()); // only succeeds when empty
    }
    Ok(freed)
}

/// One snapshot, as the pruning policy sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: i64,
    pub taken_at: i64,
    /// Taken before the latest review marker for its worktree.
    pub reviewed: bool,
    /// The latest snapshot of its worktree, or a review marker's snapshot.
    pub protected: bool,
}

/// The order to delete snapshots in when a store is over its limit: reviewed ones
/// oldest first, then the rest oldest first. Protected snapshots are never deleted.
pub fn deletion_order(snaps: &[Candidate]) -> Vec<i64> {
    let mut c: Vec<&Candidate> = snaps.iter().filter(|s| !s.protected).collect();
    c.sort_by_key(|s| (!s.reviewed, s.taken_at, s.id));
    c.into_iter().map(|s| s.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_snapshots_go_first_and_protected_ones_stay() {
        let c = |id, taken_at, reviewed, protected| Candidate {
            id,
            taken_at,
            reviewed,
            protected,
        };
        let order = deletion_order(&[
            c(1, 10, false, false),
            c(2, 20, true, false),
            c(3, 5, true, true),
            c(4, 1, false, false),
            c(5, 15, true, false),
        ]);
        assert_eq!(order, vec![5, 2, 4, 1]);
    }
}
