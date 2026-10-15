//! Reporter (SPEC §5.7): scan → tree, statistics, duplicates (hash),
//! long-unused files, empty folders; text / JSON / HTML; and restructure
//! plans (rules- or AI-based) shown as a before/after diff and applied only
//! on explicit approval, as one undoable batch.
//!
//! Everything here is read-only on the filesystem except
//! [`restructure::apply`], which moves files exclusively through the
//! [`crate::mover::Mover`] (logged in `move_log`).

pub mod dup;
pub mod html;
pub mod restructure;
pub mod scan;
pub mod stats;
pub mod tree;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

pub use dup::{find_duplicates, DupGroup, DupOptions};
pub use scan::{scan, DirNode, ScanError, ScanOptions, ScanResult};
pub use stats::{StaleFile, Stats, TypeStat};
pub use tree::{render_tree, TreeOptions};

use crate::store::Store;

#[derive(Debug, Clone)]
pub struct ReportOptions {
    pub scan: ScanOptions,
    pub top_n: usize,
    pub stale_days: u64,
    /// `None` = skip duplicate detection (it reads file contents).
    pub duplicates: Option<DupOptions>,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            scan: ScanOptions::default(),
            top_n: 10,
            stale_days: 90,
            duplicates: Some(DupOptions::default()),
        }
    }
}

/// A full folder report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub root: PathBuf,
    pub generated_at: i64,
    pub stale_days: u64,
    pub tree: DirNode,
    pub stats: Stats,
    pub duplicates: Vec<DupGroup>,
    /// Entries that could not be read (permissions etc.).
    pub errors: Vec<String>,
}

/// Build a report. `index` (optional) lets duplicate detection reuse
/// stored hashes.
pub fn build(
    root: &Path,
    opts: &ReportOptions,
    index: Option<&Store>,
) -> Result<Report, ScanError> {
    let res = scan(root, &opts.scan)?;
    let stats = Stats::compute(
        &res.tree,
        opts.top_n,
        Duration::from_secs(opts.stale_days * 86_400),
    );
    let duplicates = opts
        .duplicates
        .as_ref()
        .map(|d| find_duplicates(&res.root, &res.tree, d, index))
        .unwrap_or_default();
    Ok(Report {
        root: res.root,
        generated_at: chrono::Utc::now().timestamp(),
        stale_days: opts.stale_days,
        tree: res.tree,
        stats,
        duplicates,
        errors: res.errors,
    })
}
