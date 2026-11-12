//! (0.16.1, SPEC §14.22) "정리 위치 목록 내보내기": one file that describes
//! everything inside an organize root — so a cleanup can be planned
//! elsewhere (by Claude, from the file the user sends) and come back as a
//! plan file ([`super::import`]: preview, tick, apply as one undoable
//! batch). Read only: nothing moves.
//!
//! What it holds per file: the path below the root, size, modified date,
//! the folder it originally came from (when Tidy moved it), and the
//! indexed excerpt (up to [`EXCERPT_CHARS`], never for files with personal
//! information or vault zips). Development projects are one entry with a
//! file count (never split); hidden folders, ignored and protected paths
//! are left out. Also: the dictionary's customers and the rules, so the
//! plan can follow them.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::review::{name_of, rel, visible};
use super::scan::ScanError;
use crate::store::Store;
use crate::watcher::IgnoreFilter;

/// Most files listed.
pub const MAX_FILES: usize = 150_000;
/// Characters of indexed text per file.
pub const EXCERPT_CHARS: usize = 160;

#[derive(Debug, Serialize)]
struct Entry {
    /// Path below the root, `/`-separated.
    p: String,
    /// Bytes (a project folder: all its files).
    s: u64,
    /// Modified, `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "String::is_empty")]
    m: String,
    /// Where Tidy took it from (the folder's path, last two parts).
    #[serde(skip_serializing_if = "String::is_empty")]
    from: String,
    /// Indexed text, first characters.
    #[serde(skip_serializing_if = "String::is_empty")]
    t: String,
    /// A development project folder: moved only whole; `n` files.
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u64>,
}

#[derive(Debug, Serialize)]
struct Header<'a> {
    tidy_listing: u32,
    root: String,
    created: String,
    customers: Vec<String>,
    rules: String,
    files: usize,
    capped: bool,
    entries: &'a [Entry],
}

/// What was written.
#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub capped: bool,
}

fn date(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t)
        .format("%Y-%m-%d")
        .to_string()
}

/// Two last parts of a folder path ("바탕 화면/가람정보 로그").
fn tail2(p: &Path) -> String {
    let parts: Vec<String> = p
        .components()
        .rev()
        .take(2)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

/// List `root` into the file `out` (JSON).
#[allow(clippy::too_many_arguments)]
pub fn write_listing(
    root: &Path,
    store: &Store,
    filter: Option<&IgnoreFilter>,
    protected: &[PathBuf],
    customers: Vec<String>,
    rules_text: String,
    out: &Path,
) -> Result<Listing, ScanError> {
    let root =
        dunce::canonicalize(root).map_err(|_| ScanError::NotADirectory(root.to_path_buf()))?;
    let out_canon = out.to_path_buf();
    let mut entries: Vec<Entry> = Vec::new();
    let mut capped = false;
    let mut total = 0u64;
    let skip = |p: &Path| {
        !visible(&name_of(p))
            || protected.iter().any(|x| p.starts_with(x))
            || filter.is_some_and(|f| f.is_ignored(p))
            || *p == out_canon
    };
    let mut walker = walkdir::WalkDir::new(&root)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter();
    while let Some(e) = walker.next() {
        let Ok(e) = e else { continue };
        let p = e.path();
        if skip(p) {
            if e.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }
        if entries.len() >= MAX_FILES {
            capped = true;
            break;
        }
        if e.file_type().is_dir() {
            if super::relocate::is_dev_project(p) {
                walker.skip_current_dir();
                let (n, b) = super::review::count_files(p);
                total += b;
                entries.push(Entry {
                    p: rel(&root, p),
                    s: b,
                    m: String::new(),
                    from: String::new(),
                    t: String::new(),
                    n: Some(n),
                });
            }
            continue;
        }
        if !e.file_type().is_file() {
            continue;
        }
        let meta = e.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        total += size;
        let m = meta
            .and_then(|m| m.modified().ok())
            .map(date)
            .unwrap_or_default();
        let rec = store.get_file(p).ok().flatten();
        let t = rec
            .as_ref()
            .filter(|r| !r.vault && r.pii_flags.unwrap_or(0) == 0)
            .and_then(|r| r.excerpt.as_deref())
            .map(|x| {
                x.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(EXCERPT_CHARS)
                    .collect()
            })
            .unwrap_or_default();
        let from = store
            .active_move_to(p)
            .ok()
            .flatten()
            .and_then(|mv| mv.src_path.parent().map(tail2))
            .unwrap_or_default();
        entries.push(Entry {
            p: rel(&root, p),
            s: size,
            m,
            from,
            t,
            n: None,
        });
    }
    let files = entries.iter().map(|e| e.n.unwrap_or(1) as usize).sum();
    let head = Header {
        tidy_listing: 1,
        root: root.display().to_string(),
        created: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        customers,
        rules: rules_text,
        files,
        capped,
        entries: &entries,
    };
    let write = || -> std::io::Result<()> {
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::io::BufWriter::new(std::fs::File::create(out)?);
        serde_json::to_writer(&mut f, &head)?;
        f.flush()
    };
    write().map_err(|e| ScanError::Io {
        path: out.to_path_buf(),
        source: e,
    })?;
    Ok(Listing {
        path: out.to_path_buf(),
        files,
        bytes: total,
        capped,
    })
}

/// Where the app saves a listing: Downloads (else Documents, else home),
/// never a watched folder like the desktop.
pub fn default_out(cfg: &crate::config::Config) -> PathBuf {
    let name = format!(
        "Tidy_정리위치_목록_{}.json",
        chrono::Local::now().format("%Y%m%d-%H%M")
    );
    let watched = |d: &Path| cfg.watches.iter().any(|w| d.starts_with(&w.path));
    for d in [dirs::download_dir(), dirs::document_dir(), dirs::home_dir()]
        .into_iter()
        .flatten()
    {
        if d.is_dir() && !watched(&d) {
            return d.join(name);
        }
    }
    std::env::temp_dir().join(name)
}

/// [`write_listing`] with the dictionary, rules and ignore filter from the
/// configuration.
pub fn run(
    cfg: &crate::config::Config,
    root: &Path,
    store: &Store,
    protected: &[PathBuf],
    out: Option<PathBuf>,
) -> Result<Listing, ScanError> {
    let filter = cfg.ignore_filter().ok();
    let customers = crate::curate::Curator::from_config(cfg)
        .map(|c| crate::curate::customer_names(c.dict(), Some(store)))
        .unwrap_or_default();
    let rules_text = std::fs::read_to_string(&cfg.classify.rules).unwrap_or_default();
    let out = out.unwrap_or_else(|| default_out(cfg));
    write_listing(
        root,
        store,
        filter.as_ref(),
        protected,
        customers,
        rules_text,
        &out,
    )
}
