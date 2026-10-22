//! Recognising links (design §11) and PR status from `gh`.

pub mod gh;

use regex::Regex;
use std::sync::LazyLock;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    GithubPr,
    Jira,
    Notion,
    Other,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::GithubPr => "github_pr",
            Kind::Jira => "jira",
            Kind::Notion => "notion",
            Kind::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recognised {
    pub url: String,
    pub kind: Kind,
    /// The title shown until a better one is known (a PR title from `gh`, or one you typed).
    pub title: String,
    pub host: String,
    /// `owner/repo` and number, for PR links.
    pub pr: Option<(String, u64)>,
    /// The ticket key, for Jira links.
    pub ticket: Option<String>,
}

const MAX_URL: usize = 2048;

static GITHUB_REPO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$").unwrap());
static TICKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9]+-[0-9]+$").unwrap());
static NOTION_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-?[0-9a-f]{32}$").unwrap());

/// True for `owner/repo` names safe to pass to `gh -R`.
pub fn is_github_repo(s: &str) -> bool {
    GITHUB_REPO.is_match(s) && !s.contains("..")
}

/// Accepts only absolute `http`/`https` URLs with a host and no embedded credentials.
/// This runs when a link is saved, shown and opened.
pub fn validate(raw: &str) -> Result<Url, &'static str> {
    let raw = raw.trim();
    if raw.len() > MAX_URL {
        return Err("URL is too long");
    }
    if crate::util::has_control_or_bidi(raw) {
        return Err("URL contains control characters");
    }
    let url = Url::parse(raw).map_err(|_| "not a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https links are allowed");
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("URL has no host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL must not contain a user name or password");
    }
    Ok(url)
}

pub fn recognise(raw: &str) -> Result<Recognised, &'static str> {
    let url = validate(raw)?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    let base = Recognised {
        url: url.to_string(),
        kind: Kind::Other,
        title: other_title(&host, &segments),
        host: host.clone(),
        pr: None,
        ticket: None,
    };

    if (host == "github.com" || host == "www.github.com")
        && url.scheme() == "https"
        && let [owner, repo, "pull", n, ..] = segments.as_slice()
        && let Ok(n) = n.parse::<u64>()
    {
        let repo = format!("{owner}/{repo}");
        if is_github_repo(&repo) {
            return Ok(Recognised {
                kind: Kind::GithubPr,
                title: format!("{repo}#{n}"),
                pr: Some((repo, n)),
                ..base
            });
        }
    }
    if host.ends_with(".atlassian.net")
        && let ["browse", key, ..] = segments.as_slice()
        && TICKET.is_match(key)
    {
        return Ok(Recognised {
            kind: Kind::Jira,
            title: key.to_string(),
            ticket: Some(key.to_string()),
            ..base
        });
    }
    if host == "www.notion.so" || host == "notion.so" || host.ends_with(".notion.site") {
        return Ok(Recognised {
            kind: Kind::Notion,
            title: notion_title(segments.last().copied()),
            ..base
        });
    }
    Ok(base)
}

fn other_title(host: &str, segments: &[&str]) -> String {
    let path = segments.join("/");
    let t = if path.is_empty() {
        host.to_string()
    } else {
        format!("{host}/{path}")
    };
    crate::util::truncate_chars(&t, 120)
}

/// `Design-notes-1a2b…` → "Design notes".
fn notion_title(slug: Option<&str>) -> String {
    let Some(slug) = slug else {
        return "Notion page".into();
    };
    let slug = percent_decode(slug);
    let words = NOTION_ID.replace(&slug, "").replace('-', " ");
    let words = words.trim();
    if words.is_empty() {
        "Notion page".into()
    } else {
        crate::util::truncate_chars(words, 120)
    }
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

/// A ticket link built from `tickets.url_template`.
pub fn ticket_url(template: &str, key: &str) -> Option<String> {
    if !TICKET.is_match(key) {
        return None;
    }
    let url = template.replace("{key}", key);
    validate(&url).ok().map(|u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_pull_requests() {
        let r = recognise("https://github.com/acme/shop/pull/42/files").unwrap();
        assert_eq!(r.kind, Kind::GithubPr);
        assert_eq!(r.pr, Some(("acme/shop".into(), 42)));
        assert_eq!(r.title, "acme/shop#42");
        assert_eq!(r.host, "github.com");
        assert_eq!(
            recognise("https://github.com/acme/shop/issues/42")
                .unwrap()
                .kind,
            Kind::Other
        );
        assert_eq!(
            recognise("http://github.com/acme/shop/pull/42")
                .unwrap()
                .kind,
            Kind::Other
        );
    }

    #[test]
    fn jira_tickets() {
        let r = recognise("https://example.atlassian.net/browse/ABC-123").unwrap();
        assert_eq!(r.kind, Kind::Jira);
        assert_eq!(r.title, "ABC-123");
        assert_eq!(r.ticket.as_deref(), Some("ABC-123"));
        assert_eq!(
            recognise("https://example.atlassian.net/browse/abc")
                .unwrap()
                .kind,
            Kind::Other
        );
    }

    #[test]
    fn notion_titles_come_from_the_slug() {
        let r =
            recognise("https://www.notion.so/team/Design-notes-1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d")
                .unwrap();
        assert_eq!(r.kind, Kind::Notion);
        assert_eq!(r.title, "Design notes");
        assert_eq!(
            recognise("https://acme.notion.site/1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d")
                .unwrap()
                .title,
            "Notion page"
        );
        assert_eq!(
            recognise("https://www.notion.so/Caf%C3%A9-plan-1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d")
                .unwrap()
                .title,
            "Café plan"
        );
    }

    #[test]
    fn other_links_show_host_and_path() {
        let r = recognise("https://example.com/docs/intro?x=1").unwrap();
        assert_eq!(r.kind, Kind::Other);
        assert_eq!(r.title, "example.com/docs/intro");
        assert_eq!(
            recognise("https://example.com").unwrap().title,
            "example.com"
        );
    }

    #[test]
    fn only_http_and_https_are_accepted() {
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "smb://server/share",
            "vscode://file/x",
            "data:text/html,hi",
            "//example.com/x",
            "example.com",
            "https://user:pw@example.com/",
            "https://example.com/\u{202E}gpj.exe",
        ] {
            assert!(recognise(bad).is_err(), "{bad}");
        }
        assert!(validate(&format!("https://e.com/{}", "a".repeat(3000))).is_err());
    }

    #[test]
    fn github_repo_names_are_checked() {
        assert!(is_github_repo("acme/shop.web"));
        assert!(is_github_repo("a-b/c_d"));
        assert!(!is_github_repo("acme/shop;rm"));
        assert!(!is_github_repo("-R/x y"));
        assert!(!is_github_repo("acme/../x"));
        assert!(!is_github_repo("acme"));
    }

    #[test]
    fn ticket_urls_from_the_template() {
        let t = "https://example.atlassian.net/browse/{key}";
        assert_eq!(
            ticket_url(t, "ABC-123").as_deref(),
            Some("https://example.atlassian.net/browse/ABC-123")
        );
        assert_eq!(ticket_url(t, "abc-1"), None);
        assert_eq!(ticket_url("javascript:{key}", "ABC-1"), None);
    }
}
