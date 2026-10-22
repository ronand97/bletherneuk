//! bletherneuk: a macOS companion for Claude Code in iTerm2.

// SQL rows are read into tuples; naming each one would add noise, not clarity.
#![allow(clippy::type_complexity)]

pub mod awake;
pub mod config;
pub mod files;
pub mod group;
pub mod hook;
pub mod ingest;
pub mod install;
pub mod links;
pub mod open;
pub mod paths;
pub mod server;
pub mod service;
pub mod snapshots;
pub mod socket;
pub mod status;
pub mod store;
pub mod theme;
pub mod util;
pub mod worktrees;

#[cfg(test)]
mod testutil;
