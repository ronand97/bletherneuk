//! PR status through the GitHub CLI (design §11 and §20.4).
//!
//! `gh` runs `git` in its current folder, so it always runs from the service's own
//! data folder with `-R <owner>/<repo>`, never from inside a repo. Arguments are passed
//! as a list, never through a shell.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

const FIELDS: &str = "number,title,headRefName,isDraft,state,reviewDecision,statusCheckRollup,url";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checks {
    Passing,
    Failing,
    Pending,
    None,
}

impl Checks {
    pub fn as_str(self) -> &'static str {
        match self {
            Checks::Passing => "passing",
            Checks::Failing => "failing",
            Checks::Pending => "pending",
            Checks::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub head_branch: String,
    pub state: String,
    pub is_draft: bool,
    pub review_decision: Option<String>,
    pub checks: Checks,
    pub url: String,
    /// From a fork: its head branch name says nothing about branches in this repo.
    pub cross_repo: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPr {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    head_ref_name: String,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    state: String,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    status_check_rollup: Option<Value>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    is_cross_repository: bool,
}

pub fn parse_pr_list(json: &[u8]) -> anyhow::Result<Vec<Pr>> {
    let raw: Vec<RawPr> = serde_json::from_slice(json)?;
    Ok(raw
        .into_iter()
        .map(|r| Pr {
            number: r.number,
            title: r.title,
            head_branch: r.head_ref_name,
            state: r.state,
            is_draft: r.is_draft,
            review_decision: r.review_decision.filter(|s| !s.is_empty()),
            checks: summarise_checks(r.status_check_rollup.as_ref()),
            url: r.url,
            cross_repo: r.is_cross_repository,
        })
        .collect())
}

/// Summarises GitHub's check rollup: any failure wins, then anything still running.
pub fn summarise_checks(rollup: Option<&Value>) -> Checks {
    let Some(items) = rollup.and_then(Value::as_array) else {
        return Checks::None;
    };
    if items.is_empty() {
        return Checks::None;
    }
    let mut pending = false;
    for item in items {
        let get = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("");
        match get("__typename") {
            "StatusContext" => match get("state") {
                "FAILURE" | "ERROR" => return Checks::Failing,
                "PENDING" | "EXPECTED" => pending = true,
                _ => {}
            },
            _ => {
                if get("status") != "COMPLETED" {
                    pending = true;
                } else if matches!(
                    get("conclusion"),
                    "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
                ) {
                    return Checks::Failing;
                }
            }
        }
    }
    if pending {
        Checks::Pending
    } else {
        Checks::Passing
    }
}

/// Runs `gh pr list` for one repo from `neutral_dir`.
pub async fn list_prs(gh: &Path, repo: &str, neutral_dir: &Path) -> anyhow::Result<Vec<Pr>> {
    anyhow::ensure!(super::is_github_repo(repo), "invalid repo name");
    let mut cmd = tokio::process::Command::new(gh);
    cmd.args([
        "pr", "list", "-R", repo, "--state", "all", "--limit", "200", "--json", FIELDS,
    ])
    .current_dir(neutral_dir)
    .env("GH_PROMPT_DISABLED", "1")
    .env("GH_NO_UPDATE_NOTIFIER", "1")
    .env("NO_COLOR", "1")
    .stdin(std::process::Stdio::null())
    .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| anyhow::anyhow!("gh timed out"))??;
    anyhow::ensure!(
        out.status.success(),
        "gh exited with {}",
        out.status.code().unwrap_or(-1)
    );
    parse_pr_list(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_gh_output() {
        let json = json!([{
            "number": 42, "title": "Add retry backoff", "headRefName": "feat/retry",
            "isDraft": true, "state": "OPEN", "reviewDecision": "",
            "statusCheckRollup": [], "url": "https://github.com/acme/shop/pull/42"
        }]);
        let prs = parse_pr_list(json.to_string().as_bytes()).unwrap();
        assert_eq!(
            prs,
            vec![Pr {
                number: 42,
                title: "Add retry backoff".into(),
                head_branch: "feat/retry".into(),
                state: "OPEN".into(),
                is_draft: true,
                review_decision: None,
                checks: Checks::None,
                url: "https://github.com/acme/shop/pull/42".into(),
                cross_repo: false,
            }]
        );
    }

    #[test]
    fn check_summaries() {
        let run = |status: &str, conclusion: &str| json!({"__typename": "CheckRun", "status": status, "conclusion": conclusion});
        let ctx = |state: &str| json!({"__typename": "StatusContext", "state": state});
        let s = |v: Value| summarise_checks(Some(&v));
        assert_eq!(
            s(json!([run("COMPLETED", "SUCCESS"), ctx("SUCCESS")])),
            Checks::Passing
        );
        assert_eq!(
            s(json!([
                run("COMPLETED", "SKIPPED"),
                run("COMPLETED", "NEUTRAL")
            ])),
            Checks::Passing
        );
        assert_eq!(
            s(json!([run("IN_PROGRESS", ""), run("COMPLETED", "SUCCESS")])),
            Checks::Pending
        );
        assert_eq!(
            s(json!([run("IN_PROGRESS", ""), run("COMPLETED", "FAILURE")])),
            Checks::Failing
        );
        assert_eq!(s(json!([ctx("PENDING")])), Checks::Pending);
        assert_eq!(s(json!([ctx("ERROR"), run("QUEUED", "")])), Checks::Failing);
        assert_eq!(s(json!([])), Checks::None);
        assert_eq!(summarise_checks(None), Checks::None);
    }

    #[tokio::test]
    async fn runs_a_fake_gh_from_the_neutral_folder() {
        let dir = tempfile::tempdir().unwrap();
        let neutral = dir.path().join("neutral");
        std::fs::create_dir(&neutral).unwrap();
        let gh = dir.path().join("gh");
        // The fake records its arguments and folder, then prints canned JSON.
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {log}\npwd >> {log}\necho '[{{\"number\":7,\"state\":\"MERGED\"}}]'\n",
                log = dir.path().join("log").display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let prs = list_prs(&gh, "acme/shop", &neutral).await.unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].state, "MERGED");
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(&lines[..4], &["pr", "list", "-R", "acme/shop"]);
        assert_eq!(
            Path::new(lines.last().unwrap()).canonicalize().unwrap(),
            neutral.canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn refuses_bad_repo_names_and_failed_runs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            list_prs(
                Path::new("/usr/bin/false"),
                "acme/shop; rm -rf x",
                dir.path()
            )
            .await
            .is_err()
        );
        assert!(
            list_prs(Path::new("/usr/bin/false"), "acme/shop", dir.path())
                .await
                .is_err()
        );
    }
}
