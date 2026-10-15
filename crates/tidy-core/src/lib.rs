//! tidy-core: UI-agnostic core of Tidy.
//!
//! Module map follows `docs/SPEC.md` §4.1. Only modules needed for the
//! current milestone exist; the rest are added milestone by milestone.
//!
//! Invariant (SPEC §4): nothing in this crate touches the filesystem
//! destructively except the `mover` module (added in M1).

pub mod config;
pub mod report;

pub use config::Config;
