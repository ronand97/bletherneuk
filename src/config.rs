//! `~/.config/bletherneuk/config.toml`: personal settings. Every key is optional.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Extensions that may never be opened, whatever the config says (design §20.5).
pub const DANGEROUS_EXTENSIONS: &[&str] = &[
    "command", "terminal", "tool", "app", "pkg", "fileloc", "webloc", "jar", "py", "sh",
];

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub tickets: Tickets,
    pub chats: Chats,
    pub outputs: Outputs,
    pub snapshots: Snapshots,
    pub archive: Archive,
    pub github: Github,
    pub keep_awake: KeepAwake,
    pub worktrees: Worktrees,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Tickets {
    pub projects: Vec<String>,
    pub url_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Chats {
    pub temp_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Outputs {
    pub extensions: Vec<String>,
    pub copy_max_mb: u64,
    pub copies_max_total_gb: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Snapshots {
    pub new_file_max_mb: u64,
    pub tracked_max_mb: u64,
    pub max_store_gb: u64,
    pub keep_days_after_archive: u64,
    pub skip_patterns: Vec<String>,
    pub max_new_data_mb: u64,
    pub max_files: u64,
    pub max_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Archive {
    pub idle_days: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Github {
    pub poll_minutes: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeepAwake {
    pub grace_seconds: u64,
    pub max_hours: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Worktrees {
    pub status_interval_seconds: u64,
}

impl Default for Chats {
    fn default() -> Self {
        Chats {
            temp_dirs: ["/private/tmp", "/tmp", "/private/var/folders"]
                .iter()
                .map(PathBuf::from)
                .collect(),
        }
    }
}

impl Default for Outputs {
    fn default() -> Self {
        Outputs {
            extensions: [
                "html", "csv", "geojson", "parquet", "png", "svg", "xlsx", "pdf",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            copy_max_mb: 200,
            copies_max_total_gb: 5,
        }
    }
}

impl Default for Snapshots {
    fn default() -> Self {
        Snapshots {
            new_file_max_mb: 1,
            tracked_max_mb: 5,
            max_store_gb: 2,
            keep_days_after_archive: 30,
            skip_patterns: [
                ".env*",
                "*.pem",
                "*.key",
                "*.p12",
                "*.pfx",
                "id_rsa*",
                "id_ed25519*",
                "*credentials*.json",
                "*service-account*.json",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            max_new_data_mb: 500,
            max_files: 200_000,
            max_seconds: 60,
        }
    }
}

impl Default for Archive {
    fn default() -> Self {
        Archive { idle_days: 14 }
    }
}

impl Default for Github {
    fn default() -> Self {
        Github { poll_minutes: 5 }
    }
}

impl Default for KeepAwake {
    fn default() -> Self {
        KeepAwake {
            grace_seconds: 120,
            max_hours: 12,
        }
    }
}

impl Default for Worktrees {
    fn default() -> Self {
        Worktrees {
            status_interval_seconds: 30,
        }
    }
}

/// A config file that parsed, plus anything in it that was ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
}

impl Config {
    /// Parses config text. Invalid values are dropped with a warning rather than failing,
    /// so one typo never stops the service.
    pub fn parse(text: &str) -> Result<Loaded, toml::de::Error> {
        let mut config: Config = toml::from_str(text)?;
        let mut warnings = Vec::new();

        config.tickets.projects.retain(|p| {
            let ok = is_ticket_prefix(p);
            if !ok {
                warnings.push(format!(
                    "tickets.projects: ignoring {p:?}; prefixes are capital letters and digits, starting with a letter"
                ));
            }
            ok
        });
        if let Some(t) = &config.tickets.url_template
            && !is_valid_url_template(t)
        {
            warnings.push(
                "tickets.url_template: ignored; it must be an https:// or http:// URL containing {key}"
                    .to_string(),
            );
            config.tickets.url_template = None;
        }
        config.outputs.extensions = config
            .outputs
            .extensions
            .iter()
            .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| {
                let ok = !e.is_empty()
                    && e.chars().all(|c| c.is_ascii_alphanumeric())
                    && !DANGEROUS_EXTENSIONS.contains(&e.as_str())
                    && e != "md";
                if !ok {
                    warnings.push(format!("outputs.extensions: ignoring {e:?}"));
                }
                ok
            })
            .collect();
        config.chats.temp_dirs.retain(|d| {
            let ok = d.is_absolute();
            if !ok {
                warnings.push(format!(
                    "chats.temp_dirs: ignoring {d:?}; folders must be absolute"
                ));
            }
            ok
        });
        Ok(Loaded { config, warnings })
    }

    /// Reads the config file. A missing file means all defaults.
    pub fn load(path: &Path) -> anyhow::Result<Loaded> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Config::parse(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
                config: Config::default(),
                warnings: Vec::new(),
            }),
            Err(e) => Err(e.into()),
        }
    }

    pub fn is_output_extension(&self, ext: &str) -> bool {
        let ext = ext.to_ascii_lowercase();
        self.outputs.extensions.contains(&ext)
    }
}

pub fn is_ticket_prefix(p: &str) -> bool {
    let mut chars = p.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && p.len() <= 16
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn is_valid_url_template(t: &str) -> bool {
    (t.starts_with("https://") || t.starts_with("http://"))
        && t.contains("{key}")
        && url::Url::parse(&t.replace("{key}", "ABC-1")).is_ok()
}

/// Starter file written by `blether install`. Everything is commented out, so the defaults apply.
pub const STARTER: &str = r#"# bletherneuk settings. Every key is optional; the values shown are the defaults.
# See the design doc, section 17.

# [tickets]
# projects = ["ABC"]                                          # ticket-key prefixes to recognise
# url_template = "https://example.atlassian.net/browse/{key}"

# [chats]
# temp_dirs = ["/private/tmp", "/tmp", "/private/var/folders"]

# [outputs]
# extensions = ["html", "csv", "geojson", "parquet", "png", "svg", "xlsx", "pdf"]
# copy_max_mb = 200
# copies_max_total_gb = 5

# [snapshots]
# new_file_max_mb = 1
# tracked_max_mb = 5
# max_store_gb = 2
# keep_days_after_archive = 30
# max_new_data_mb = 500
# max_files = 200000
# max_seconds = 60

# [archive]
# idle_days = 14

# [github]
# poll_minutes = 5

# [keep_awake]
# grace_seconds = 120
# max_hours = 12

# [worktrees]
# status_interval_seconds = 30
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let loaded = Config::parse("").unwrap();
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn starter_file_parses_to_defaults() {
        let loaded = Config::parse(STARTER).unwrap();
        assert_eq!(loaded.config, Config::default());
    }

    #[test]
    fn reads_every_section() {
        let text = r#"
            [tickets]
            projects = ["ABC", "XY2"]
            url_template = "https://example.atlassian.net/browse/{key}"
            [chats]
            temp_dirs = ["/scratch"]
            [outputs]
            extensions = [".HTML", "csv"]
            copy_max_mb = 10
            copies_max_total_gb = 1
            [snapshots]
            new_file_max_mb = 2
            tracked_max_mb = 3
            max_store_gb = 4
            keep_days_after_archive = 5
            skip_patterns = ["*.secret"]
            max_new_data_mb = 6
            max_files = 7
            max_seconds = 8
            [archive]
            idle_days = 9
            [github]
            poll_minutes = 10
            [keep_awake]
            grace_seconds = 11
            max_hours = 12
            [worktrees]
            status_interval_seconds = 13
        "#;
        let c = Config::parse(text).unwrap().config;
        assert_eq!(c.tickets.projects, vec!["ABC", "XY2"]);
        assert_eq!(
            c.tickets.url_template.as_deref(),
            Some("https://example.atlassian.net/browse/{key}")
        );
        assert_eq!(c.chats.temp_dirs, vec![PathBuf::from("/scratch")]);
        assert_eq!(c.outputs.extensions, vec!["html", "csv"]);
        assert_eq!(c.outputs.copy_max_mb, 10);
        assert_eq!(c.outputs.copies_max_total_gb, 1);
        assert_eq!(c.snapshots.new_file_max_mb, 2);
        assert_eq!(c.snapshots.tracked_max_mb, 3);
        assert_eq!(c.snapshots.max_store_gb, 4);
        assert_eq!(c.snapshots.keep_days_after_archive, 5);
        assert_eq!(c.snapshots.skip_patterns, vec!["*.secret"]);
        assert_eq!(c.snapshots.max_new_data_mb, 6);
        assert_eq!(c.snapshots.max_files, 7);
        assert_eq!(c.snapshots.max_seconds, 8);
        assert_eq!(c.archive.idle_days, 9);
        assert_eq!(c.github.poll_minutes, 10);
        assert_eq!(c.keep_awake.grace_seconds, 11);
        assert_eq!(c.keep_awake.max_hours, 12);
        assert_eq!(c.worktrees.status_interval_seconds, 13);
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert!(Config::parse("[tickets]\nprojets = [\"ABC\"]").is_err());
    }

    #[test]
    fn bad_ticket_prefixes_are_dropped_with_a_warning() {
        let loaded =
            Config::parse("[tickets]\nprojects = [\"ABC\", \"abc\", \"1AB\", \"A-B\"]").unwrap();
        assert_eq!(loaded.config.tickets.projects, vec!["ABC"]);
        assert_eq!(loaded.warnings.len(), 3);
    }

    #[test]
    fn url_template_must_be_http_and_contain_key() {
        for bad in [
            "javascript:alert({key})",
            "https://example.atlassian.net/browse/",
            "file:///{key}",
        ] {
            let text = format!("[tickets]\nurl_template = {bad:?}");
            let loaded = Config::parse(&text).unwrap();
            assert_eq!(loaded.config.tickets.url_template, None, "{bad}");
            assert_eq!(loaded.warnings.len(), 1);
        }
    }

    #[test]
    fn dangerous_output_extensions_are_dropped() {
        let loaded = Config::parse(
            "[outputs]\nextensions = [\"html\", \"sh\", \"app\", \"\", \"md\", \"a/b\"]",
        )
        .unwrap();
        assert_eq!(loaded.config.outputs.extensions, vec!["html"]);
        assert_eq!(loaded.warnings.len(), 5);
    }

    #[test]
    fn relative_temp_dirs_are_dropped() {
        let loaded = Config::parse("[chats]\ntemp_dirs = [\"tmp\", \"/tmp\"]").unwrap();
        assert_eq!(loaded.config.chats.temp_dirs, vec![PathBuf::from("/tmp")]);
        assert_eq!(loaded.warnings.len(), 1);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(loaded.config, Config::default());
    }
}
