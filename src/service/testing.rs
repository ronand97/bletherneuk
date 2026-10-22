//! A service with fake side effects, for tests.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{App, Effects, Shared};
use crate::awake::Launcher;
use crate::open::{Handlers, OpenError, Runner};
use crate::paths::Paths;
use crate::store::Store;

#[derive(Clone, Default)]
pub struct FakeLauncher(pub Arc<Mutex<bool>>);

impl Launcher for FakeLauncher {
    fn start(&mut self) -> std::io::Result<()> {
        *self.0.lock().unwrap() = true;
        Ok(())
    }
    fn stop(&mut self) {
        *self.0.lock().unwrap() = false;
    }
    fn running(&mut self) -> bool {
        *self.0.lock().unwrap()
    }
}

#[derive(Clone, Default)]
pub struct Recorder(pub Arc<Mutex<Vec<Vec<String>>>>);

impl Runner for Recorder {
    fn open(&self, args: &[String]) -> Result<(), OpenError> {
        self.0.lock().unwrap().push(args.to_vec());
        Ok(())
    }
}

pub struct FixedHandlers;

impl Handlers for FixedHandlers {
    fn for_file(&self, _: &Path) -> Option<String> {
        Some("com.example.Viewer".into())
    }
    fn browser(&self) -> Option<String> {
        Some("com.example.Browser".into())
    }
}

pub struct TestApp {
    pub app: Shared,
    pub receivers: super::tasks::Receivers,
    pub opened: Recorder,
    pub awake: FakeLauncher,
    pub _dir: tempfile::TempDir,
}

thread_local! {
    static PORT: Cell<u16> = const { Cell::new(47_000) };
}

pub fn test_app() -> TestApp {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let home = base.join("home");
    std::fs::create_dir_all(home.join(".claude/projects")).unwrap();
    std::fs::create_dir_all(base.join("w")).unwrap();
    let paths = Paths::from_home(&home, base.join("tmp"));
    let opened = Recorder::default();
    let awake = FakeLauncher::default();
    let effects = Effects {
        launcher: Box::new(awake.clone()),
        runner: Box::new(opened.clone()),
        handlers: Box::new(FixedHandlers),
        pick_file: Box::new(|| None::<PathBuf>),
    };
    let (app, receivers) = App::new(paths, Store::open_in_memory().unwrap(), effects, 0);
    let port = PORT.with(|p| {
        let v = p.get();
        p.set(v + 1);
        v
    });
    app.port.set(port).unwrap();
    {
        let mut c = app.config.write().unwrap();
        c.chats.temp_dirs = vec![base.join("scratch")];
        c.tickets.projects = vec!["ABC".into()];
        c.archive.idle_days = 14;
        c.keep_awake.grace_seconds = 120;
        c.keep_awake.max_hours = 12;
    }
    {
        let db = app.db.lock().unwrap();
        crate::group::store_ticket_prefixes(db.conn(), &app.config()).unwrap();
    }
    TestApp {
        app,
        receivers,
        opened,
        awake,
        _dir: dir,
    }
}

impl TestApp {
    pub fn work_dir(&self) -> PathBuf {
        self.app.paths.home.parent().unwrap().join("w")
    }

    /// Runs any queued snapshot jobs right now.
    pub fn drain_snapshots(&mut self) {
        while let Ok(job) = self.receivers.snapshots.try_recv() {
            super::snapshots::run_job(&self.app, &job);
        }
    }
}
