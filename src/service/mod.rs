//! `blether serve`: the index service. Holds the shared state and starts the socket
//! server, the HTTP server, the transcript watcher and the background jobs.

pub mod actions;
pub mod bridge;
pub mod hooks;
pub mod snapshots;
pub mod tasks;
pub mod views;

#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::awake::{self, Launcher};
use crate::config::Config;
use crate::open::{Handlers, Runner};
use crate::paths::Paths;
use crate::store::Store;
use crate::theme::{ItermColors, Theme};
use crate::worktrees::Locator;

/// Something pages should know about, sent over server-sent events.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Changed {
        blocks: Vec<i64>,
        board: bool,
    },
    Theme(Value),
    Focus {
        block: Option<i64>,
        chat: Option<String>,
    },
    Bridge {
        connected: bool,
    },
}

impl UiEvent {
    pub fn name(&self) -> &'static str {
        match self {
            UiEvent::Changed { .. } => "changed",
            UiEvent::Theme(_) => "theme",
            UiEvent::Focus { .. } => "focus",
            UiEvent::Bridge { .. } => "bridge",
        }
    }

    pub fn data(&self) -> Value {
        match self {
            UiEvent::Changed { blocks, board } => json!({"blocks": blocks, "board": board}),
            UiEvent::Theme(vars) => json!({"vars": vars}),
            UiEvent::Focus { block, chat } => json!({"block": block, "chat": chat}),
            UiEvent::Bridge { connected } => json!({"connected": connected}),
        }
    }
}

/// An iTerm2 session the bridge has told us about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    pub window: Option<String>,
    pub tab: Option<String>,
    /// Set only for new chats the bridge started from a block (design §9). The chat
    /// that starts in this session joins that block.
    pub block: Option<i64>,
    /// The block whose window holds the session. Used only to work out focus.
    pub window_block: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Focus {
    pub session: Option<String>,
    pub window: Option<String>,
    pub block: Option<i64>,
    pub chat: Option<String>,
}

pub struct Code {
    pub expires: Instant,
    pub next: String,
}

/// Secrets created fresh on every start (design §20.2).
pub struct Secrets {
    pub session: String,
    pub write_key: String,
}

impl Secrets {
    pub fn fresh() -> Secrets {
        Secrets {
            session: random_hex(32),
            write_key: random_hex(32),
        }
    }
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    for chunk in buf.chunks_mut(256) {
        let rc = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
        assert_eq!(rc, 0, "getentropy failed");
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Side effects that tests replace.
pub struct Effects {
    pub launcher: Box<dyn Launcher>,
    pub runner: Box<dyn Runner + Send + Sync>,
    pub handlers: Box<dyn Handlers + Send + Sync>,
    /// Shows the native "choose file" dialog; returns the chosen path.
    pub pick_file: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
}

impl Effects {
    pub fn real() -> Effects {
        Effects {
            launcher: Box::new(awake::Caffeinate::new(std::process::id())),
            runner: Box::new(crate::open::SystemOpen),
            handlers: Box::new(crate::open::LaunchServices),
            pick_file: Box::new(actions::choose_file_dialog),
        }
    }
}

impl Launcher for Box<dyn Launcher> {
    fn start(&mut self) -> std::io::Result<()> {
        (**self).start()
    }
    fn stop(&mut self) {
        (**self).stop()
    }
    fn running(&mut self) -> bool {
        (**self).running()
    }
}

pub struct App {
    pub paths: Paths,
    pub config: RwLock<Config>,
    pub warnings: RwLock<Vec<String>>,
    pub theme: RwLock<Theme>,
    pub iterm_colors: RwLock<Option<ItermColors>>,
    pub db: Mutex<Store>,
    pub locator: Mutex<Locator>,
    pub events: broadcast::Sender<UiEvent>,
    pub bridge: Mutex<Option<bridge::Handle>>,
    pub sessions: Mutex<HashMap<String, Session>>,
    pub focus: Mutex<Focus>,
    pub secrets: Secrets,
    pub port: OnceLock<u16>,
    pub codes: Mutex<HashMap<String, Code>>,
    pub awake: Mutex<awake::Manager<Box<dyn Launcher>>>,
    pub snap_locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
    /// Files each open turn wrote, from hook events (turn ID → paths).
    pub turn_writes: Mutex<HashMap<i64, BTreeSet<PathBuf>>>,
    pub runner: Box<dyn Runner + Send + Sync>,
    pub handlers: Box<dyn Handlers + Send + Sync>,
    pub pick_file: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    pub gh: Option<PathBuf>,
    pub started_at: i64,
    pub panel_refresh: Mutex<Option<Instant>>,
    pub ingest_tx: mpsc::UnboundedSender<PathBuf>,
    pub snap_tx: mpsc::UnboundedSender<snapshots::Job>,
    /// Asks the worktree job to refresh now (a branch switch, or new repos found).
    pub worktrees_now: Arc<tokio::sync::Notify>,
    /// Repos where a git operation never returned. They're skipped until a restart.
    pub stuck: Mutex<std::collections::HashSet<PathBuf>>,
}

pub type Shared = Arc<App>;

impl App {
    /// Builds the shared state, plus the receiving ends of its work queues.
    pub fn new(
        paths: Paths,
        store: Store,
        effects: Effects,
        now_ms: i64,
    ) -> (Shared, tasks::Receivers) {
        let loaded = Config::load(&paths.config_file());
        let (config, mut warnings) = match loaded {
            Ok(l) => (l.config, l.warnings),
            Err(e) => (Config::default(), vec![format!("config.toml: {e}")]),
        };
        let (theme, theme_warnings) = match Theme::load(&paths.theme_file()) {
            Ok(l) => (l.theme, l.warnings),
            Err(e) => (Theme::default(), vec![format!("theme.toml: {e}")]),
        };
        warnings.extend(theme_warnings);
        let _ = crate::group::store_ticket_prefixes(store.conn(), &config);
        let (events, _) = broadcast::channel(256);
        let (ingest_tx, ingest_rx) = mpsc::unbounded_channel();
        let (snap_tx, snap_rx) = mpsc::unbounded_channel();
        let awake = awake::Manager::new(
            effects.launcher,
            config.keep_awake.grace_seconds,
            config.keep_awake.max_hours,
        );
        let app = App {
            paths,
            config: RwLock::new(config),
            warnings: RwLock::new(warnings),
            theme: RwLock::new(theme),
            iterm_colors: RwLock::new(None),
            db: Mutex::new(store),
            locator: Mutex::new(Locator::default()),
            events,
            bridge: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            focus: Mutex::new(Focus::default()),
            secrets: Secrets::fresh(),
            port: OnceLock::new(),
            codes: Mutex::new(HashMap::new()),
            awake: Mutex::new(awake),
            snap_locks: Mutex::new(HashMap::new()),
            turn_writes: Mutex::new(HashMap::new()),
            runner: effects.runner,
            handlers: effects.handlers,
            pick_file: effects.pick_file,
            gh: find_on_path("gh"),
            started_at: now_ms,
            panel_refresh: Mutex::new(None),
            ingest_tx,
            snap_tx,
            worktrees_now: Arc::new(tokio::sync::Notify::new()),
            stuck: Mutex::new(std::collections::HashSet::new()),
        };
        (
            Arc::new(app),
            tasks::Receivers {
                ingest: ingest_rx,
                snapshots: snap_rx,
            },
        )
    }

    pub fn config(&self) -> Config {
        self.config.read().expect("config lock").clone()
    }

    pub fn port(&self) -> u16 {
        self.port.get().copied().unwrap_or(0)
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }

    pub fn emit(&self, e: UiEvent) {
        let _ = self.events.send(e);
    }

    pub fn changed(&self, blocks: impl IntoIterator<Item = i64>) {
        let blocks: Vec<i64> = blocks.into_iter().collect();
        self.emit(UiEvent::Changed {
            blocks,
            board: true,
        });
    }

    pub fn theme_vars(&self) -> Value {
        let theme = self.theme.read().expect("theme lock");
        let colors = self.iterm_colors.read().expect("colors lock");
        json!(theme.css_vars(colors.as_ref()))
    }

    pub fn bridge_connected(&self) -> bool {
        self.bridge.lock().expect("bridge lock").is_some()
    }

    /// A one-time page URL (design §20.2): the code works once, for 30 seconds.
    pub fn page_url(&self, next: &str) -> String {
        let code = random_hex(24);
        let mut codes = self.codes.lock().expect("codes lock");
        let now = Instant::now();
        codes.retain(|_, c| c.expires > now);
        codes.insert(
            code.clone(),
            Code {
                expires: now + std::time::Duration::from_secs(30),
                next: next.to_string(),
            },
        );
        let next_enc: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
        format!("{}/auth?code={code}&next={next_enc}", self.origin())
    }

    /// Uses up a code, returning where it leads.
    pub fn redeem(&self, code: &str) -> Option<String> {
        let mut codes = self.codes.lock().expect("codes lock");
        let c = codes.remove(code)?;
        (c.expires > Instant::now()).then_some(c.next)
    }

    /// The page path for a page name, checked against the allowed set.
    pub fn page_path(page: &str, block: Option<i64>) -> Option<String> {
        match (page, block) {
            ("board", _) => Some("/".into()),
            ("panel", _) => Some("/panel".into()),
            ("changes", Some(b)) => Some(format!("/changes?block={b}")),
            _ => None,
        }
    }

    pub fn is_stuck(&self, repo_root: &std::path::Path) -> bool {
        self.stuck.lock().expect("stuck lock").contains(repo_root)
    }

    pub fn mark_stuck(&self, repo_root: &std::path::Path) {
        tracing::warn!("a repo stopped responding to git reads; skipping it until restart");
        self.stuck
            .lock()
            .expect("stuck lock")
            .insert(repo_root.to_path_buf());
    }

    pub fn reload_config(&self) {
        let mut warnings = Vec::new();
        match Config::load(&self.paths.config_file()) {
            Ok(l) => {
                warnings.extend(l.warnings);
                let db = self.db.lock().expect("db lock");
                let _ = crate::group::store_ticket_prefixes(db.conn(), &l.config);
                drop(db);
                self.awake.lock().expect("awake lock").set_limits(
                    l.config.keep_awake.grace_seconds,
                    l.config.keep_awake.max_hours,
                );
                *self.config.write().expect("config lock") = l.config;
            }
            Err(e) => warnings.push(format!("config.toml: {e}")),
        }
        match Theme::load(&self.paths.theme_file()) {
            Ok(l) => {
                warnings.extend(l.warnings);
                *self.theme.write().expect("theme lock") = l.theme;
            }
            Err(e) => warnings.push(format!("theme.toml: {e}")),
        }
        *self.warnings.write().expect("warnings lock") = warnings;
        self.emit(UiEvent::Theme(self.theme_vars()));
    }
}

/// Finds an executable on `PATH`.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Runs the service until it is stopped.
pub async fn serve() -> anyhow::Result<()> {
    let paths = Paths::current()?;
    crate::paths::ensure_private_dir(&paths.data())?;
    crate::paths::ensure_private_dir(&paths.run_dir())?;
    crate::paths::ensure_private_dir(&paths.logs())?;
    let now = crate::util::now_ms();
    let store = Store::open(&paths.db(), &paths.backups(), now)?;
    let (app, receivers) = App::new(paths, store, Effects::real(), now);

    let listener = crate::socket::bind(&app.paths)?;
    let http = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = http.local_addr()?.port();
    app.port.set(port).expect("port set once");
    tracing::info!(port, "service started");

    tasks::restore_keep_awake(&app);
    tokio::spawn(crate::socket::accept_loop(app.clone(), listener));
    tokio::spawn(crate::server::run(app.clone(), http));
    tasks::spawn_all(app.clone(), receivers);

    // launchd stops the service with SIGTERM.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    let _ = std::fs::remove_file(app.paths.socket());
    app.awake.lock().expect("awake lock").shutdown();
    Ok(())
}
