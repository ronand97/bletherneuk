//! bletherneuk: a macOS companion for Claude Code in iTerm2.

pub mod config;
pub mod files;
pub mod group;
pub mod ingest;
pub mod links;
pub mod paths;
pub mod store;
pub mod theme;
pub mod util;
pub mod worktrees;

#[cfg(test)]
mod testutil;
