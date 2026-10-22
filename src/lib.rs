//! bletherneuk: a macOS companion for Claude Code in iTerm2.

pub mod awake;
pub mod config;
pub mod files;
pub mod group;
pub mod hook;
pub mod ingest;
pub mod links;
pub mod paths;
pub mod snapshots;
pub mod status;
pub mod store;
pub mod theme;
pub mod util;
pub mod worktrees;

#[cfg(test)]
mod testutil;
