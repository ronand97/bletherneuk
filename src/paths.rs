//! Where bletherneuk keeps things. Everything hangs off one home folder so tests
//! (and `BLETHERNEUK_HOME`) can point the whole tool at a temporary one.

use std::path::{Path, PathBuf};

/// Unix socket paths on macOS are limited to 104 bytes including the final NUL.
const MAX_SOCKET_PATH: usize = 103;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    tmpdir: PathBuf,
}

impl Paths {
    pub fn from_home(home: impl Into<PathBuf>, tmpdir: impl Into<PathBuf>) -> Paths {
        Paths {
            home: home.into(),
            tmpdir: tmpdir.into(),
        }
    }

    /// The real home folder, or `BLETHERNEUK_HOME` when set.
    pub fn current() -> anyhow::Result<Paths> {
        let home = std::env::var_os("BLETHERNEUK_HOME")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        anyhow::ensure!(home.is_absolute(), "home folder must be absolute");
        let tmpdir = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Ok(Paths::from_home(home, tmpdir))
    }

    pub fn data(&self) -> PathBuf {
        self.home.join("Library/Application Support/bletherneuk")
    }
    pub fn db(&self) -> PathBuf {
        self.data().join("index.db")
    }
    pub fn backups(&self) -> PathBuf {
        self.data().join("backups")
    }
    pub fn copies(&self) -> PathBuf {
        self.data().join("copies")
    }
    pub fn snapshots(&self) -> PathBuf {
        self.data().join("snapshots")
    }
    pub fn logs(&self) -> PathBuf {
        self.data().join("logs")
    }
    pub fn run_dir(&self) -> PathBuf {
        self.data().join("run")
    }

    /// The service's Unix socket. Falls back to `$TMPDIR/bletherneuk/` when the
    /// default path is too long for a socket address.
    pub fn socket(&self) -> PathBuf {
        let default = self.run_dir().join("blether.sock");
        if default.as_os_str().len() <= MAX_SOCKET_PATH {
            default
        } else {
            self.tmpdir.join("bletherneuk").join("blether.sock")
        }
    }

    pub fn config_dir(&self) -> PathBuf {
        self.home.join(".config/bletherneuk")
    }
    pub fn config_file(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }
    pub fn theme_file(&self) -> PathBuf {
        self.config_dir().join("theme.toml")
    }

    pub fn claude_dir(&self) -> PathBuf {
        self.home.join(".claude")
    }
    pub fn claude_projects(&self) -> PathBuf {
        self.claude_dir().join("projects")
    }
    pub fn claude_settings(&self) -> PathBuf {
        self.claude_dir().join("settings.json")
    }

    pub fn launch_agent(&self) -> PathBuf {
        self.home
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist"))
    }
    pub fn iterm_autolaunch(&self) -> PathBuf {
        self.home
            .join("Library/Application Support/iTerm2/Scripts/AutoLaunch")
    }
    pub fn installed_binary(&self) -> PathBuf {
        self.home.join(".local/bin/blether")
    }

    /// Folders that never hold docs or outputs worth listing (design §10).
    pub fn is_private_tool_dir(&self, path: &Path) -> bool {
        path.starts_with(self.claude_dir()) || path.starts_with(self.home.join("Library"))
    }
}

pub const LAUNCHD_LABEL: &str = "dev.bletherneuk.serve";

/// Creates a folder (and parents) and sets it to mode 0700.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_lives_under_application_support() {
        let p = Paths::from_home("/Users/someone", "/var/folders/xy/T");
        assert_eq!(
            p.db(),
            PathBuf::from("/Users/someone/Library/Application Support/bletherneuk/index.db")
        );
        assert_eq!(
            p.socket(),
            PathBuf::from(
                "/Users/someone/Library/Application Support/bletherneuk/run/blether.sock"
            )
        );
        assert_eq!(
            p.config_file(),
            PathBuf::from("/Users/someone/.config/bletherneuk/config.toml")
        );
    }

    #[test]
    fn long_home_moves_the_socket_to_tmpdir() {
        let long = format!("/Users/{}", "x".repeat(80));
        let p = Paths::from_home(long, "/var/folders/xy/T");
        assert_eq!(
            p.socket(),
            PathBuf::from("/var/folders/xy/T/bletherneuk/blether.sock")
        );
    }

    #[test]
    fn claude_and_library_folders_are_private() {
        let p = Paths::from_home("/Users/someone", "/tmp");
        assert!(p.is_private_tool_dir(Path::new("/Users/someone/.claude/plans/x.md")));
        assert!(p.is_private_tool_dir(Path::new("/Users/someone/Library/Caches/x.html")));
        assert!(!p.is_private_tool_dir(Path::new("/Users/someone/repos/x/report.html")));
    }
}
