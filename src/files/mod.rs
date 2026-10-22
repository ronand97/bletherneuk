//! Docs and outputs: what counts, finding paths in commands and tool output, and
//! keeping copies of files that live in temporary folders (design §10).

pub mod copies;

use std::path::{Component, Path, PathBuf};

use regex::Regex;

use crate::config::Config;
use crate::paths::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Doc,
    Output,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Doc => "doc",
            Kind::Output => "output",
        }
    }
}

/// Folder names whose contents are never listed.
const IGNORED_DIRS: &[&str] = &[".venv", "venv", "node_modules", "site-packages", ".git"];

/// Agent instruction files are never listed as docs.
const IGNORED_DOC_NAMES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

pub fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// True for paths inside folders that never hold docs or outputs worth listing.
pub fn is_ignored(path: &Path, paths: &Paths) -> bool {
    paths.is_private_tool_dir(path)
        || path.components().any(|c| match c {
            Component::Normal(n) => n.to_str().is_some_and(|n| IGNORED_DIRS.contains(&n)),
            _ => false,
        })
}

/// Classifies a file a chat wrote with a tool.
pub fn classify_written(path: &Path, config: &Config, paths: &Paths) -> Option<Kind> {
    if !path.is_absolute() || is_ignored(path, paths) {
        return None;
    }
    let ext = extension(path)?;
    if ext == "md" {
        let name = path.file_name()?.to_str()?;
        return (!IGNORED_DOC_NAMES.contains(&name)).then_some(Kind::Doc);
    }
    config.is_output_extension(&ext).then_some(Kind::Output)
}

/// Classifies a path mentioned in a command or in tool output. Only outputs count,
/// and only when the file exists.
pub fn classify_mentioned(path: &Path, config: &Config, paths: &Paths) -> Option<Kind> {
    if !path.is_absolute() || is_ignored(path, paths) {
        return None;
    }
    let ext = extension(path)?;
    if !config.is_output_extension(&ext) {
        return None;
    }
    let meta = std::fs::symlink_metadata(path).ok()?;
    meta.file_type().is_file().then_some(Kind::Output)
}

/// Finds candidate output paths in a command or tool output. Absolute and `~` paths are
/// taken as they are; relative ones are resolved against `cwd`. URLs are skipped.
pub struct PathFinder {
    bare: Regex,
    quoted: Regex,
}

impl PathFinder {
    pub fn new(extensions: &[String]) -> PathFinder {
        let exts: Vec<String> = extensions.iter().map(|e| regex::escape(e)).collect();
        let alt = if exts.is_empty() {
            // Matches nothing.
            "(?!)".to_string()
        } else {
            exts.join("|")
        };
        // A run of path characters ending in .<ext>. What follows is checked in `find`.
        let bare = Regex::new(&format!(r#"(?i)[^\s'"`<>|;,(){{}}\[\]=*?$]+\.(?:{alt})"#))
            .expect("valid regex");
        let quoted = Regex::new(&format!(
            r#"(?i)(?:"([^"\n]+\.(?:{alt}))"|'([^'\n]+\.(?:{alt}))')"#
        ))
        .expect("valid regex");
        PathFinder { bare, quoted }
    }

    pub fn find(&self, text: &str, cwd: Option<&Path>, home: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let push = |raw: &str, out: &mut Vec<PathBuf>| {
            if let Some(p) = resolve(raw, cwd, home)
                && !out.contains(&p)
            {
                out.push(p);
            }
        };
        // Quoted paths may contain spaces. Blank them out afterwards so the bare scan
        // doesn't also pick up their last word.
        let mut rest = text.to_string();
        for c in self.quoted.captures_iter(text) {
            if let Some(m) = c.get(1).or_else(|| c.get(2)) {
                push(m.as_str(), &mut out);
            }
            let whole = c.get(0).expect("group 0");
            rest.replace_range(whole.range(), &" ".repeat(whole.len()));
        }
        for m in self.bare.find_iter(&rest) {
            // `report.html.rs` is not `report.html`: reject a match followed by a word
            // character, or by a dot and a word character.
            let mut after = rest[m.end()..].chars();
            let next = after.next();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if word(next) || (next == Some('.') && word(after.next())) {
                continue;
            }
            push(m.as_str(), &mut out);
        }
        out
    }
}

/// Turns a raw path mention into a normalised absolute path, or `None` for URLs and
/// relative paths with no folder to resolve against.
pub fn resolve(raw: &str, cwd: Option<&Path>, home: &Path) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.contains("://")
        || raw.starts_with("http:")
        || raw.starts_with("https:")
    {
        return None;
    }
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if raw.starts_with('/') {
        PathBuf::from(raw)
    } else if raw.starts_with('~') {
        // ~otheruser/... is someone else's home.
        return None;
    } else {
        cwd?.join(raw)
    };
    Some(normalise(&path))
}

/// Removes `.` and `..` components without touching the filesystem.
pub fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True when `path` is inside one of the configured temporary folders.
pub fn in_temp_dir(path: &Path, config: &Config) -> bool {
    config.chats.temp_dirs.iter().any(|d| path.starts_with(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths::from_home("/Users/dev", "/tmp")
    }

    fn finder() -> PathFinder {
        PathFinder::new(&Config::default().outputs.extensions)
    }

    fn found(text: &str) -> Vec<String> {
        finder()
            .find(text, Some(Path::new("/w/shop")), Path::new("/Users/dev"))
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn docs_are_markdown_outside_claude_folders() {
        let c = Config::default();
        let p = paths();
        assert_eq!(
            classify_written(Path::new("/w/shop/docs/spec.md"), &c, &p),
            Some(Kind::Doc)
        );
        assert_eq!(
            classify_written(Path::new("/w/shop/README.MD"), &c, &p),
            Some(Kind::Doc)
        );
        assert_eq!(
            classify_written(Path::new("/w/shop/CLAUDE.md"), &c, &p),
            None
        );
        assert_eq!(
            classify_written(Path::new("/w/shop/AGENTS.md"), &c, &p),
            None
        );
        assert_eq!(
            classify_written(Path::new("/Users/dev/.claude/memory/x.md"), &c, &p),
            None
        );
    }

    #[test]
    fn written_outputs_and_code() {
        let c = Config::default();
        let p = paths();
        assert_eq!(
            classify_written(Path::new("/w/shop/out/report.HTML"), &c, &p),
            Some(Kind::Output)
        );
        assert_eq!(
            classify_written(Path::new("/w/shop/src/main.rs"), &c, &p),
            None
        );
        assert_eq!(
            classify_written(Path::new("/w/shop/Makefile"), &c, &p),
            None
        );
        assert_eq!(
            classify_written(Path::new("/w/shop/.venv/lib/x.html"), &c, &p),
            None
        );
        assert_eq!(
            classify_written(Path::new("/w/shop/node_modules/a/b.svg"), &c, &p),
            None
        );
        assert_eq!(
            classify_written(Path::new("/Users/dev/Library/Caches/x.png"), &c, &p),
            None
        );
    }

    #[test]
    fn mentioned_outputs_must_exist_and_be_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::default();
        let p = paths();
        let real = dir.path().join("map.geojson");
        std::fs::write(&real, "{}").unwrap();
        assert_eq!(classify_mentioned(&real, &c, &p), Some(Kind::Output));
        assert_eq!(
            classify_mentioned(&dir.path().join("gone.csv"), &c, &p),
            None
        );
        let link = dir.path().join("link.csv");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(classify_mentioned(&link, &c, &p), None);
        let md = dir.path().join("notes.md");
        std::fs::write(&md, "x").unwrap();
        assert_eq!(
            classify_mentioned(&md, &c, &p),
            None,
            "docs only count when written"
        );
    }

    #[test]
    fn finds_absolute_home_and_relative_paths() {
        assert_eq!(
            found("python build.py --out /w/shop/out/report.html && open ~/Desktop/map.png"),
            vec!["/w/shop/out/report.html", "/Users/dev/Desktop/map.png"]
        );
        assert_eq!(
            found("ls out/../out/data.csv"),
            vec!["/w/shop/out/data.csv"]
        );
        assert_eq!(
            found("--output=results/a.parquet"),
            vec!["/w/shop/results/a.parquet"]
        );
    }

    #[test]
    fn finds_quoted_paths_with_spaces() {
        assert_eq!(
            found(r#"open "/w/shop/My Reports/q3 report.pdf""#),
            vec!["/w/shop/My Reports/q3 report.pdf"]
        );
        assert_eq!(found("cp 'a b.xlsx' /tmp/"), vec!["/w/shop/a b.xlsx"]);
    }

    #[test]
    fn skips_urls_and_non_outputs() {
        assert!(found("curl https://example.com/report.html -o x").is_empty());
        assert!(found("see http://127.0.0.1:8000/index.html").is_empty());
        assert!(found("edit src/report.html.rs and main.py").is_empty());
        assert!(found("~bob/report.html").is_empty());
    }

    #[test]
    fn output_text_mentions_are_found() {
        assert_eq!(
            found("Wrote 120 rows to /w/shop/out/rows.csv.\nSaved figure: /w/shop/fig.svg"),
            vec!["/w/shop/out/rows.csv", "/w/shop/fig.svg"]
        );
    }

    #[test]
    fn relative_paths_need_a_folder() {
        let f = finder();
        assert!(
            f.find("out/a.csv", None, Path::new("/Users/dev"))
                .is_empty()
        );
    }

    #[test]
    fn temp_dir_check() {
        let c = Config::default();
        assert!(in_temp_dir(
            Path::new("/private/tmp/claude-501/x/scratchpad/r.html"),
            &c
        ));
        assert!(in_temp_dir(
            Path::new("/private/var/folders/ab/T/r.html"),
            &c
        ));
        assert!(!in_temp_dir(Path::new("/w/shop/r.html"), &c));
    }
}
