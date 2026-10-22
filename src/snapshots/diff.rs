//! Tree-to-tree diffs with libgit2, as structured data for the Changes page.
//!
//! libgit2 runs no textconv or external diff programs.

use git2::{Delta, DiffFindOptions, DiffOptions, Oid, Patch, Repository};
use serde::Serialize;

/// Files with more changed lines than this start collapsed on the page.
pub const COLLAPSE_LINES: usize = 5000;
/// Lines beyond this many per file are dropped from the response.
pub const MAX_LINES_PER_FILE: usize = 20_000;

const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "Gemfile.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Line {
    /// '+', '-' or ' '
    pub kind: char,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    /// added | deleted | modified | renamed | copied | typechange
    pub status: &'static str,
    pub binary: bool,
    pub additions: usize,
    pub deletions: usize,
    /// Why the page should start with this file collapsed: lockfile, minified or large.
    pub collapsed: Option<&'static str>,
    pub truncated: bool,
    /// The file's contents are missing from both stores (history rewritten and pruned).
    pub unavailable: bool,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Totals {
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
}

pub fn totals(files: &[FileDiff]) -> Totals {
    Totals {
        files: files.len(),
        additions: files.iter().map(|f| f.additions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
    }
}

fn status(d: Delta) -> &'static str {
    match d {
        Delta::Added | Delta::Untracked => "added",
        Delta::Deleted => "deleted",
        Delta::Renamed => "renamed",
        Delta::Copied => "copied",
        Delta::Typechange => "typechange",
        _ => "modified",
    }
}

fn is_lockfile(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    LOCKFILES.contains(&name)
}

fn is_minified_name(path: &str) -> bool {
    path.ends_with(".min.js") || path.ends_with(".min.css") || path.ends_with(".map")
}

/// Diffs two trees in `repo` (which can see both, through alternates). With
/// `summary_only`, hunks are left out and only counts are filled in.
pub fn diff_trees(
    repo: &Repository,
    old: Option<Oid>,
    new: Oid,
    summary_only: bool,
) -> Result<Vec<FileDiff>, git2::Error> {
    let old_tree = match old {
        Some(o) => Some(repo.find_tree(o)?),
        None => None,
    };
    let new_tree = repo.find_tree(new)?;
    let mut opts = DiffOptions::new();
    opts.context_lines(3)
        .ignore_submodules(true)
        .include_typechange(true)
        .indent_heuristic(true);
    let mut diff = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true).copies(false)))?;

    let mut out = Vec::new();
    for idx in 0..diff.deltas().len() {
        let delta = diff.get_delta(idx).expect("index in range");
        let path_of = |f: git2::DiffFile| f.path().map(|p| p.to_string_lossy().into_owned());
        let old_path = (delta.status() != Delta::Added)
            .then(|| path_of(delta.old_file()))
            .flatten();
        let new_path = (delta.status() != Delta::Deleted)
            .then(|| path_of(delta.new_file()))
            .flatten();
        let shown = new_path
            .clone()
            .or_else(|| old_path.clone())
            .unwrap_or_default();
        let mut fd = FileDiff {
            old_path,
            new_path,
            status: status(delta.status()),
            binary: false,
            additions: 0,
            deletions: 0,
            collapsed: None,
            truncated: false,
            unavailable: false,
            hunks: Vec::new(),
        };
        let patch = match Patch::from_diff(&diff, idx) {
            Ok(p) => p,
            Err(e) if e.code() == git2::ErrorCode::NotFound => {
                fd.unavailable = true;
                out.push(fd);
                continue;
            }
            Err(e) => return Err(e),
        };
        let Some(patch) = patch else {
            fd.binary = true;
            out.push(fd);
            continue;
        };
        if patch.delta().flags().is_binary() {
            fd.binary = true;
            out.push(fd);
            continue;
        }
        let (_, adds, dels) = patch.line_stats()?;
        fd.additions = adds;
        fd.deletions = dels;
        if !summary_only {
            let mut kept = 0usize;
            let mut longest = 0usize;
            let mut total_len = 0usize;
            let mut count = 0usize;
            'hunks: for h in 0..patch.num_hunks() {
                let (hunk, n) = patch.hunk(h)?;
                let mut lines = Vec::with_capacity(n);
                for l in 0..n {
                    if kept >= MAX_LINES_PER_FILE {
                        fd.truncated = true;
                        fd.hunks.push(Hunk {
                            header: String::from_utf8_lossy(hunk.header())
                                .trim_end()
                                .to_string(),
                            lines,
                        });
                        break 'hunks;
                    }
                    let line = patch.line_in_hunk(h, l)?;
                    let kind = match line.origin() {
                        '+' | '-' | ' ' => line.origin(),
                        _ => continue, // "no newline at end of file" markers
                    };
                    let text = String::from_utf8_lossy(line.content())
                        .trim_end_matches(['\n', '\r'])
                        .to_string();
                    longest = longest.max(text.len());
                    total_len += text.len();
                    count += 1;
                    lines.push(Line {
                        kind,
                        old: line.old_lineno(),
                        new: line.new_lineno(),
                        text,
                    });
                    kept += 1;
                }
                fd.hunks.push(Hunk {
                    header: String::from_utf8_lossy(hunk.header())
                        .trim_end()
                        .to_string(),
                    lines,
                });
            }
            let minified = is_minified_name(&shown)
                || (count > 0 && longest > 2000 && total_len / count > 300);
            fd.collapsed = if is_lockfile(&shown) {
                Some("lockfile")
            } else if minified {
                Some("minified")
            } else if adds + dels > COLLAPSE_LINES {
                Some("large")
            } else {
                None
            };
        } else if is_lockfile(&shown) {
            fd.collapsed = Some("lockfile");
        }
        out.push(fd);
    }
    Ok(out)
}
