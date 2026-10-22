//! tidy-core: UI-agnostic core of Tidy.
//!
//! Module map follows `docs/SPEC.md` §4.1. Only modules needed for the
//! current milestone exist; the rest are added milestone by milestone.
//!
//! Invariant (SPEC §4): nothing in this crate changes the filesystem except
//! the `mover` module, and every move is recorded in `move_log`.

pub mod ai;
pub mod classify;
pub mod config;
pub mod curate;
pub mod engine;
pub mod index;
pub mod mover;
pub mod netio;
pub mod paths;
pub mod pending;
pub mod pii;
pub mod report;
pub mod search;
pub mod security;
pub mod service;
pub mod store;
pub mod watcher;

pub use config::Config;
