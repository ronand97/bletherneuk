//! `blether`: the bletherneuk command line (design §16).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use bletherneuk::paths::Paths;

#[derive(Parser)]
#[command(
    name = "blether",
    version,
    about = "Keeps your Claude Code chats in order, inside iTerm2."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the index service (launchd normally starts it).
    Serve,
    /// Claude Code hook entry point. Reads the event from stdin; prints nothing.
    Hook,
    /// Install hooks, the iTerm2 bridge and the background service.
    Install {
        /// Print what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove everything `install` added.
    Uninstall {
        /// Also delete the database, snapshots and copies.
        #[arg(long)]
        purge: bool,
    },
    /// Check the setup.
    Doctor {
        /// Also open and close a browser pane in iTerm2.
        #[arg(long)]
        live: bool,
    },
    /// Attach a link to the block of the chat in this tab (or --block).
    Link {
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        block: Option<i64>,
    },
    /// Add a file by hand to the block of the chat in this tab (or --block).
    File {
        path: PathBuf,
        #[arg(long)]
        block: Option<i64>,
    },
    /// Open the home board.
    Board,
    /// Re-read all transcripts. Everything you entered by hand is kept.
    Rescan,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // The hook must never print or fail, so it runs before anything else is set up.
    if let Command::Hook = cli.command {
        bletherneuk::hook::run();
        return ExitCode::SUCCESS;
    }
    match run(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("blether: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn send(paths: &Paths, msg: Value) -> anyhow::Result<Value> {
    let reply = bletherneuk::socket::request(paths, &msg, Duration::from_secs(30))?;
    if reply["ok"].as_bool() != Some(true) {
        anyhow::bail!(
            "{}",
            reply["error"].as_str().unwrap_or("the service refused")
        );
    }
    Ok(reply)
}

fn iterm_session() -> Option<String> {
    std::env::var("ITERM_SESSION_ID")
        .ok()
        .and_then(|s| bletherneuk::hook::iterm_session_uuid(&s))
}

fn run(command: Command) -> anyhow::Result<ExitCode> {
    let paths = Paths::current()?;
    match command {
        Command::Hook => unreachable!(),
        Command::Serve => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_env("BLETHERNEUK_LOG")
                        .unwrap_or_else(|_| "info".into()),
                )
                .with_ansi(false)
                .init();
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(bletherneuk::service::serve())?;
        }
        Command::Install { dry_run } => {
            let installer = bletherneuk::install::Installer {
                paths: &paths,
                binary: paths.installed_binary(),
                dry_run,
                launchctl: &bletherneuk::install::SystemLaunchctl,
                uid: unsafe { libc::getuid() },
                path_env: bletherneuk::install::service_path_env(),
            };
            if !dry_run && !installer.binary.is_file() {
                anyhow::bail!(
                    "{} doesn't exist; install with `make install`, which copies it there first",
                    installer.binary.display()
                );
            }
            installer.install(&mut std::io::stdout(), bletherneuk::util::now_ms())?;
            if !dry_run {
                println!("\nDone. Run `blether doctor` to check everything.");
            }
        }
        Command::Uninstall { purge } => {
            let installer = bletherneuk::install::Installer {
                paths: &paths,
                binary: paths.installed_binary(),
                dry_run: false,
                launchctl: &bletherneuk::install::SystemLaunchctl,
                uid: unsafe { libc::getuid() },
                path_env: String::new(),
            };
            installer.uninstall(&mut std::io::stdout(), purge, bletherneuk::util::now_ms())?;
        }
        Command::Doctor { live } => {
            let problems = bletherneuk::install::doctor::run(&paths, live, &mut std::io::stdout());
            if problems > 0 {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Link { url, title, block } => {
            let reply = send(
                &paths,
                json!({
                    "type": "link", "url": url, "title": title, "block": block,
                    "iterm_session": iterm_session(),
                    "by_claude": std::env::var_os("CLAUDECODE").is_some(),
                }),
            )?;
            println!("Linked to block {}.", reply["block"]);
        }
        Command::File { path, block } => {
            let path = std::path::absolute(&path)?;
            send(
                &paths,
                json!({"type": "add_file", "path": path, "block": block, "iterm_session": iterm_session()}),
            )?;
            println!("Added.");
        }
        Command::Board => {
            let reply = send(
                &paths,
                json!({"type": "open_page", "page": "board", "near_session": iterm_session()}),
            )?;
            if reply["via"] == "browser" {
                println!("Opened the board in your default browser.");
            }
        }
        Command::Rescan => {
            send(&paths, json!({"type": "rescan"}))?;
            println!("Re-reading all transcripts in the background.");
        }
    }
    Ok(ExitCode::SUCCESS)
}
