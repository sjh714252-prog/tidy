//! Reporter (SPEC §5.7). M0 covers: directory scan, tree rendering and
//! basic statistics. Duplicate detection (hash) and stale-file reports are
//! added together with the indexer in M2.
//!
//! This module is strictly read-only on the filesystem.

pub mod scan;
pub mod stats;
pub mod tree;

pub use scan::{scan, ScanOptions, ScanResult};
pub use stats::{Stats, TypeStat};
pub use tree::{render_tree, TreeOptions};
