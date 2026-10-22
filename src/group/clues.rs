//! The grouping clues (design §9). Pure functions: facts in, a block key and a reason out.

use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    /// `owner/repo` is lower-cased, since GitHub names are case-insensitive.
    Pr {
        repo: String,
        number: u64,
    },
    Ticket(String),
    Branch {
        repo_root: String,
        branch: String,
    },
}

impl Key {
    pub fn pr(repo: &str, number: u64) -> Key {
        Key::Pr {
            repo: repo.to_ascii_lowercase(),
            number,
        }
    }

    pub fn parse(s: &str) -> Option<Key> {
        if let Some(rest) = s.strip_prefix("pr:") {
            let (repo, n) = rest.rsplit_once('#')?;
            return Some(Key::pr(repo, n.parse().ok()?));
        }
        if let Some(t) = s.strip_prefix("ticket:") {
            return Some(Key::Ticket(t.to_string()));
        }
        if let Some(rest) = s.strip_prefix("branch:") {
            // Branch names can't contain "@{", but may contain "@"; repo roots are absolute
            // paths, so split at the first "@" that follows a path.
            let (root, branch) = rest.split_once('@')?;
            return Some(Key::Branch {
                repo_root: root.to_string(),
                branch: branch.to_string(),
            });
        }
        None
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Pr { repo, number } => write!(f, "pr:{repo}#{number}"),
            Key::Ticket(t) => write!(f, "ticket:{t}"),
            Key::Branch { repo_root, branch } => write!(f, "branch:{repo_root}@{branch}"),
        }
    }
}

/// A worktree a chat wrote files into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WroteIn {
    pub worktree: String,
    pub repo_root: String,
    /// The branch the worktree was on when the chat wrote there.
    pub branch: Option<String>,
    pub default_branch: Option<String>,
    pub writes: u64,
}

/// What grouping knows about a chat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub recorded_branch: Option<String>,
    /// The repo of the folder the chat started in.
    pub repo_root: Option<String>,
    /// `owner/repo` of that repo's `origin`, if on GitHub.
    pub github: Option<String>,
    pub default_branch: Option<String>,
    /// The worktree containing the folder the chat started in.
    pub start_worktree: Option<String>,
    pub wrote_in: Vec<WroteIn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// `None` means Unsorted.
    pub key: Option<Key>,
    pub reason: String,
}

static PR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)github\.com/([A-Za-z0-9-]+/[A-Za-z0-9._-]+)/pull/([0-9]{1,9})").unwrap()
});
static PR_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^A-Za-z0-9_])pr ?#? ?([0-9]{1,9})(?:$|[^A-Za-z0-9_])").unwrap()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrRef {
    Url { repo: String, number: u64 },
    Number(u64),
}

/// The first PR reference in `text`: a GitHub pull URL, or `PR #123`, `PR 123`, `pr123`.
pub fn find_pr(text: &str) -> Option<PrRef> {
    let url = PR_URL.captures(text).and_then(|c| {
        let start = c.get(0)?.start();
        let repo = c.get(1)?.as_str().trim_end_matches(".git");
        let number = c.get(2)?.as_str().parse().ok()?;
        crate::links::is_github_repo(repo).then(|| {
            (
                start,
                PrRef::Url {
                    repo: repo.to_ascii_lowercase(),
                    number,
                },
            )
        })
    });
    let num = PR_REF.captures(text).and_then(|c| {
        let m = c.get(1)?;
        Some((m.start(), PrRef::Number(m.as_str().parse().ok()?)))
    });
    match (url, num) {
        (Some(u), Some(n)) => Some(if u.0 <= n.0 { u.1 } else { n.1 }),
        (Some(u), None) => Some(u.1),
        (None, Some(n)) => Some(n.1),
        (None, None) => None,
    }
}

/// The first ticket key with a configured prefix. In branch names `_` may stand in
/// for `-` (`feat/abc_123_retry` → `ABC-123`).
pub fn find_ticket(text: &str, prefixes: &[String], branch: bool) -> Option<String> {
    if prefixes.is_empty() {
        return None;
    }
    let alt: Vec<String> = prefixes.iter().map(|p| regex::escape(p)).collect();
    let sep = if branch { "[-_]" } else { "-" };
    let re = Regex::new(&format!(
        r"(?i)(?:^|[^A-Za-z0-9])({})({sep})([0-9]{{1,9}})(?:$|[^0-9])",
        alt.join("|")
    ))
    .ok()?;
    let c = re.captures(text)?;
    Some(format!(
        "{}-{}",
        c.get(1)?.as_str().to_ascii_uppercase(),
        c.get(3)?.as_str()
    ))
}

fn is_default_or_head(branch: &str, default: Option<&str>) -> bool {
    branch == "HEAD"
        || match default {
            Some(d) => branch == d,
            None => branch == "main" || branch == "master",
        }
}

fn worktree_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Applies clues 2–6. Clue 1 (a manual move) is checked by the caller.
pub fn evaluate(f: &Facts, ticket_prefixes: &[String]) -> Placement {
    let texts = [("title", &f.title), ("first prompt", &f.first_prompt)];

    for (what, text) in texts {
        let Some(text) = text else { continue };
        match find_pr(text) {
            Some(PrRef::Url { repo, number }) => {
                return Placement {
                    key: Some(Key::pr(&repo, number)),
                    reason: format!("{what} links PR #{number}"),
                };
            }
            Some(PrRef::Number(number)) => {
                if let Some(gh) = &f.github {
                    return Placement {
                        key: Some(Key::pr(gh, number)),
                        reason: format!("{what} mentions PR #{number}"),
                    };
                }
            }
            None => {}
        }
    }

    for (what, text) in texts {
        if let Some(t) = text
            .as_deref()
            .and_then(|t| find_ticket(t, ticket_prefixes, false))
        {
            return Placement {
                reason: format!("{what} mentions {t}"),
                key: Some(Key::Ticket(t)),
            };
        }
    }
    if let Some(t) = f
        .recorded_branch
        .as_deref()
        .and_then(|b| find_ticket(b, ticket_prefixes, true))
    {
        return Placement {
            reason: format!("branch name has {t}"),
            key: Some(Key::Ticket(t)),
        };
    }

    let drift = f
        .wrote_in
        .iter()
        .filter(|w| Some(&w.worktree) != f.start_worktree.as_ref())
        .max_by(|a, b| {
            a.writes
                .cmp(&b.writes)
                .then_with(|| b.worktree.cmp(&a.worktree))
        });
    if let Some(w) = drift
        && let Some(branch) = &w.branch
        && !is_default_or_head(branch, w.default_branch.as_deref())
    {
        return Placement {
            key: Some(Key::Branch {
                repo_root: w.repo_root.clone(),
                branch: branch.clone(),
            }),
            reason: format!(
                "wrote files in worktree {} ({branch})",
                worktree_name(&w.worktree)
            ),
        };
    }

    if let (Some(branch), Some(root)) = (&f.recorded_branch, &f.repo_root)
        && !is_default_or_head(branch, f.default_branch.as_deref())
    {
        return Placement {
            key: Some(Key::Branch {
                repo_root: root.clone(),
                branch: branch.clone(),
            }),
            reason: format!("on branch {branch}"),
        };
    }

    Placement {
        key: None,
        reason: "no clue matched".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes() -> Vec<String> {
        vec!["ABC".into(), "XY".into()]
    }

    fn base() -> Facts {
        Facts {
            repo_root: Some("/w/shop".into()),
            github: Some("Acme/Shop".into()),
            default_branch: Some("main".into()),
            start_worktree: Some("/w/shop".into()),
            ..Facts::default()
        }
    }

    fn branch_key(b: &str) -> Option<Key> {
        Some(Key::Branch {
            repo_root: "/w/shop".into(),
            branch: b.into(),
        })
    }

    #[test]
    fn keys_round_trip() {
        for k in [
            Key::pr("acme/shop", 42),
            Key::Ticket("ABC-123".into()),
            Key::Branch {
                repo_root: "/w/shop".into(),
                branch: "feat/a@b".into(),
            },
        ] {
            assert_eq!(Key::parse(&k.to_string()), Some(k));
        }
        assert_eq!(Key::pr("Acme/Shop", 1).to_string(), "pr:acme/shop#1");
    }

    #[test]
    fn pr_references() {
        for (text, n) in [
            ("Review PR #123 please", 123),
            ("review pr 77", 77),
            ("pr123", 123),
            ("(PR#9)", 9),
        ] {
            assert_eq!(find_pr(text), Some(PrRef::Number(n)), "{text}");
        }
        for text in ["April123", "PRs are good", "sprint 12", "pr12a"] {
            assert_eq!(find_pr(text), None, "{text}");
        }
        assert_eq!(
            find_pr("see https://github.com/Acme/Shop/pull/42/files then PR 7"),
            Some(PrRef::Url {
                repo: "acme/shop".into(),
                number: 42
            })
        );
        assert_eq!(
            find_pr("PR 7 then github.com/a/b/pull/8"),
            Some(PrRef::Number(7))
        );
    }

    #[test]
    fn ticket_keys_need_a_configured_prefix() {
        let p = prefixes();
        assert_eq!(
            find_ticket("Fix ABC-123 now", &p, false).as_deref(),
            Some("ABC-123")
        );
        assert_eq!(
            find_ticket("fix abc-123", &p, false).as_deref(),
            Some("ABC-123")
        );
        assert_eq!(find_ticket("UTF-8 and SHA-256", &p, false), None);
        assert_eq!(find_ticket("XYZ-1", &p, false), None);
        assert_eq!(
            find_ticket("ABC_123", &p, false),
            None,
            "underscore only in branches"
        );
        assert_eq!(
            find_ticket("feat/abc_123_retry", &p, true).as_deref(),
            Some("ABC-123")
        );
        assert_eq!(find_ticket("ABC-123", &[], false), None);
    }

    #[test]
    fn pr_in_title_wins() {
        let f = Facts {
            title: Some("Address PR #42 comments".into()),
            first_prompt: Some("ABC-9".into()),
            recorded_branch: Some("feat/x".into()),
            ..base()
        };
        let p = evaluate(&f, &prefixes());
        assert_eq!(p.key, Some(Key::pr("acme/shop", 42)));
        assert_eq!(p.reason, "title mentions PR #42");
    }

    #[test]
    fn pr_number_needs_a_github_repo() {
        let f = Facts {
            title: Some("PR #42".into()),
            github: None,
            recorded_branch: Some("feat/x".into()),
            ..base()
        };
        assert_eq!(evaluate(&f, &prefixes()).key, branch_key("feat/x"));
    }

    #[test]
    fn pr_url_in_first_prompt() {
        let f = Facts {
            first_prompt: Some("review https://github.com/other/lib/pull/5".into()),
            ..base()
        };
        let p = evaluate(&f, &prefixes());
        assert_eq!(p.key, Some(Key::pr("other/lib", 5)));
        assert_eq!(p.reason, "first prompt links PR #5");
    }

    #[test]
    fn ticket_before_drift_and_branch() {
        let f = Facts {
            first_prompt: Some("Work on ABC-12".into()),
            recorded_branch: Some("feat/other".into()),
            ..base()
        };
        let p = evaluate(&f, &prefixes());
        assert_eq!(p.key, Some(Key::Ticket("ABC-12".into())));
        assert_eq!(p.reason, "first prompt mentions ABC-12");

        let f = Facts {
            recorded_branch: Some("feat/abc_7_x".into()),
            ..base()
        };
        let p = evaluate(&f, &prefixes());
        assert_eq!(p.key, Some(Key::Ticket("ABC-7".into())));
        assert_eq!(p.reason, "branch name has ABC-7");
    }

    #[test]
    fn drift_beats_the_recorded_branch() {
        let wrote = |wt: &str, branch: &str, writes| WroteIn {
            worktree: wt.into(),
            repo_root: "/w/shop".into(),
            branch: Some(branch.into()),
            default_branch: Some("main".into()),
            writes,
        };
        let f = Facts {
            recorded_branch: Some("main".into()),
            wrote_in: vec![
                wrote("/w/shop", "main", 20),
                wrote("/w/shop/.claude/worktrees/a", "feat/a", 2),
                wrote("/w/shop/.claude/worktrees/b", "feat/b", 5),
            ],
            ..base()
        };
        let p = evaluate(&f, &prefixes());
        assert_eq!(p.key, branch_key("feat/b"));
        assert_eq!(p.reason, "wrote files in worktree b (feat/b)");
    }

    #[test]
    fn drift_into_a_default_branch_falls_through() {
        let f = Facts {
            recorded_branch: Some("feat/x".into()),
            start_worktree: Some("/w/shop/.claude/worktrees/x".into()),
            wrote_in: vec![WroteIn {
                worktree: "/w/shop".into(),
                repo_root: "/w/shop".into(),
                branch: Some("main".into()),
                default_branch: Some("main".into()),
                writes: 3,
            }],
            ..base()
        };
        assert_eq!(evaluate(&f, &prefixes()).key, branch_key("feat/x"));
    }

    #[test]
    fn default_branch_and_head_go_to_unsorted() {
        for b in ["main", "HEAD"] {
            let f = Facts {
                recorded_branch: Some(b.into()),
                ..base()
            };
            let p = evaluate(&f, &prefixes());
            assert_eq!(p.key, None);
            assert_eq!(p.reason, "no clue matched");
        }
        let f = Facts {
            recorded_branch: Some("master".into()),
            default_branch: None,
            ..base()
        };
        assert_eq!(evaluate(&f, &prefixes()).key, None);
        let f = Facts {
            recorded_branch: Some("develop".into()),
            default_branch: Some("develop".into()),
            ..base()
        };
        assert_eq!(evaluate(&f, &prefixes()).key, None);
    }

    #[test]
    fn recorded_branch() {
        let f = Facts {
            recorded_branch: Some("feat/x".into()),
            ..base()
        };
        let p = evaluate(&f, &prefixes());
        assert_eq!(p.key, branch_key("feat/x"));
        assert_eq!(p.reason, "on branch feat/x");
    }
}
