//! Helpers shared by unit tests.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::paths::Paths;
use crate::store::Store;

pub const SESSION: &str = "11111111-1111-4111-8111-111111111111";

/// A throwaway home folder plus a work folder, with explicit settings.
pub struct Env {
    /// Kept so the folder lives as long as the test.
    pub _dir: tempfile::TempDir,
    pub paths: Paths,
    pub config: Config,
    pub store: Store,
}

impl Env {
    pub fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, so paths match what the OS reports (/var → /private/var).
        let base = dir.path().canonicalize().unwrap();
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".claude/projects")).unwrap();
        std::fs::create_dir_all(base.join("w")).unwrap();
        let paths = Paths::from_home(&home, base.join("tmp"));
        let mut config = Config::default();
        // The temp folder that holds the test is itself inside /private/var/folders, so
        // tests choose their own temporary folders.
        config.chats.temp_dirs = vec![base.join("scratch")];
        config.tickets.projects = vec!["ABC".into()];
        Env {
            _dir: dir,
            paths,
            config,
            store: Store::open_in_memory().unwrap(),
        }
    }

    pub fn base(&self) -> PathBuf {
        self.paths.home.parent().unwrap().to_path_buf()
    }

    /// The folder fixtures call `{ROOT}`.
    pub fn root(&self) -> PathBuf {
        self.base().join("w")
    }

    /// A fixture transcript with `{ROOT}` filled in.
    pub fn fixture(&self, name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/transcripts")
            .join(name);
        std::fs::read_to_string(p)
            .unwrap()
            .replace("{ROOT}", self.root().to_str().unwrap())
    }

    /// Where Claude Code would keep the transcript for `session` started in `folder`.
    pub fn transcript_path(&self, folder: &str, session: &str) -> PathBuf {
        let dir = self.paths.claude_projects().join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{session}.jsonl"))
    }

    pub fn write_file(&self, rel: &str, contents: &str) -> PathBuf {
        let p = self.root().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p
    }
}
