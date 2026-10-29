//! Walk a root and keep `files` + FTS current (SPEC §5.5).
//!
//! Incremental: a file whose size and mtime are unchanged is only marked
//! as seen. Files over `max_content_bytes` get metadata only (no hash, no
//! text). Extraction failures keep the metadata row (`text_status =
//! failed`). Rows for files that disappeared are removed after the walk.
//! Nothing here writes to the filesystem.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use thiserror::Error;
use walkdir::WalkDir;

use super::extract::Extractors;
use chrono::NaiveTime;

use crate::config::{Config, RootKind, WatchRoot};
use crate::mover::{hash_file, TMP_MARKER};
use crate::paths::ext_lower;
use crate::store::{NewFile, OcrState, Store, StoreError, TextStatus, VisionState};
use crate::watcher::IgnoreFilter;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("root does not exist or is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("not a file: {0}")]
    NotAFile(PathBuf),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid ignore globs: {0}")]
    Ignore(#[from] crate::watcher::WatchError),
}

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub recursive: bool,
    pub kind: RootKind,
    /// The root is also a watch folder (informational, `roots.watch`).
    pub watch: bool,
    /// Extract body text (SPEC: NAS roots default to metadata only).
    pub index_content: bool,
    /// Above this size: metadata only.
    pub max_content_bytes: u64,
    /// Above this size: no hash either (network roots: SPEC §5.9 "해시는
    /// 작은 파일만", `index.max_hash_size_mb_network`).
    pub max_hash_bytes: Option<u64>,
    /// Stored text is cut here (characters).
    pub max_text_chars: usize,
    pub ignore_globs: Vec<String>,
    /// Configured network folders (single files inside them are indexed
    /// with their settings, see [`IndexOptions::for_file`]).
    pub network_roots: Vec<NetworkRoot>,
    pub skip_hidden: bool,
    /// How many files per write transaction.
    pub batch_size: usize,
    /// (M9) Judge personal information from the extracted text
    /// (`[security.pii]`); `None` = no verdicts stored.
    pub pii: Option<Arc<crate::pii::Detector>>,
    /// (M15) Images up to this size are marked for text recognition.
    pub ocr_max_image_bytes: u64,
    /// (M16) Pictures up to this size are marked for a description.
    pub vision_max_image_bytes: u64,
}

impl Default for IndexOptions {
    fn default() -> Self {
        let idx = crate::config::Index::default();
        Self {
            recursive: true,
            kind: RootKind::Local,
            watch: false,
            index_content: true,
            max_content_bytes: idx.max_content_size_mb * 1024 * 1024,
            max_hash_bytes: None,
            max_text_chars: idx.max_text_chars,
            ignore_globs: crate::config::Classify::default().ignore_globs,
            network_roots: Vec::new(),
            skip_hidden: true,
            batch_size: 500,
            pii: None,
            ocr_max_image_bytes: crate::config::OcrCfg::default().max_image_mb * 1024 * 1024,
            vision_max_image_bytes: crate::config::VisionCfg::default().max_image_mb * 1024 * 1024,
        }
    }
}

impl IndexOptions {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            max_content_bytes: cfg.index.max_content_size_mb * 1024 * 1024,
            max_text_chars: cfg.index.max_text_chars,
            ignore_globs: cfg.classify.ignore_globs.clone(),
            network_roots: cfg
                .watches
                .iter()
                .filter(|w| w.kind == RootKind::Network)
                .map(|w| NetworkRoot {
                    root: w.clone(),
                    max_hash_bytes: cfg.index.max_hash_size_mb_network * 1024 * 1024,
                    canon: Arc::default(),
                })
                .collect(),
            pii: cfg
                .security
                .pii
                .enabled
                .then(|| Arc::new(crate::pii::Detector::lenient(&cfg.security.pii))),
            ocr_max_image_bytes: cfg.ocr.max_image_mb * 1024 * 1024,
            vision_max_image_bytes: cfg.vision.max_image_mb * 1024 * 1024,
            ..Default::default()
        }
    }

    /// Options for a `[[watch]]` folder. `manual` = an explicit `tidy
    /// index` run, which ignores `content_index_window` (SPEC §5.5 "수동
    /// 트리거"); background runs honour it at local time `now`.
    pub fn for_watch(cfg: &Config, w: &WatchRoot, manual: bool, now: NaiveTime) -> Self {
        let network = w.kind == RootKind::Network;
        Self {
            recursive: w.recursive,
            kind: w.kind,
            watch: true,
            index_content: if manual {
                w.index_content
            } else {
                w.content_now(now)
            },
            max_hash_bytes: network.then(|| cfg.index.max_hash_size_mb_network * 1024 * 1024),
            ..Self::from_config(cfg)
        }
    }

    fn reads_network(&self) -> bool {
        self.kind == RootKind::Network
    }

    /// Options for one file: inside a configured network folder, that
    /// folder's rules apply (metadata by default, content window, small
    /// hashes only) whatever options the caller passed.
    fn for_file(&self, path: &Path) -> std::borrow::Cow<'_, IndexOptions> {
        use std::borrow::Cow;
        if self.kind == RootKind::Network {
            return Cow::Borrowed(self);
        }
        let hit = self
            .network_roots
            .iter()
            .filter(|n| {
                path.starts_with(&n.root.path) || n.canonical().is_some_and(|c| path.starts_with(c))
            })
            .max_by_key(|n| n.root.path.components().count());
        match hit {
            None => Cow::Borrowed(self),
            Some(n) => Cow::Owned(IndexOptions {
                kind: RootKind::Network,
                index_content: n.root.content_now(chrono::Local::now().time()),
                max_hash_bytes: Some(n.max_hash_bytes),
                ..self.clone()
            }),
        }
    }
}

/// A configured network watch folder, for [`IndexOptions::for_file`].
#[derive(Debug, Clone)]
pub struct NetworkRoot {
    root: WatchRoot,
    max_hash_bytes: u64,
    /// Resolved on first use only (canonicalizing an offline share can
    /// be slow).
    canon: Arc<OnceLock<Option<PathBuf>>>,
}

impl NetworkRoot {
    fn canonical(&self) -> Option<&Path> {
        self.canon
            .get_or_init(|| dunce::canonicalize(&self.root.path).ok())
            .as_deref()
    }
}

/// Every configured root with its options: `[[watch]]` folders by their
/// own settings, `[[index_only]]` folders recursive with content.
/// `manual`: see [`IndexOptions::for_watch`].
pub fn jobs_from_config(cfg: &Config, manual: bool) -> Vec<(PathBuf, IndexOptions)> {
    let now = chrono::Local::now().time();
    let mut jobs = Vec::new();
    for w in &cfg.watches {
        jobs.push((w.path.clone(), IndexOptions::for_watch(cfg, w, manual, now)));
    }
    for r in &cfg.index_only {
        jobs.push((
            r.path.clone(),
            IndexOptions {
                recursive: r.recursive,
                ..IndexOptions::from_config(cfg)
            },
        ));
    }
    jobs
}

/// What happened to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOutcome {
    Added(TextStatus),
    Updated(TextStatus),
    Unchanged,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IndexReport {
    pub root: PathBuf,
    pub scanned: u64,
    pub added: u64,
    pub updated: u64,
    pub unchanged: u64,
    /// Rows dropped because the file is gone.
    pub removed: u64,
    pub text_ok: u64,
    pub text_failed: u64,
    pub text_unsupported: u64,
    pub text_skipped: u64,
    pub errors: Vec<String>,
    pub elapsed_ms: u64,
}

pub struct Indexer<'a> {
    store: &'a Store,
    extractors: Extractors,
}

impl<'a> Indexer<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            extractors: Extractors::default_set(),
        }
    }

    pub fn with_extractors(mut self, extractors: Extractors) -> Self {
        self.extractors = extractors;
        self
    }

    pub fn store(&self) -> &Store {
        self.store
    }

    /// Index one root. `progress` is called after every file.
    pub fn index_root(
        &self,
        root: &Path,
        opts: &IndexOptions,
        mut progress: impl FnMut(&Path, &FileOutcome),
    ) -> Result<IndexReport, IndexError> {
        let started = Instant::now();
        let root =
            dunce::canonicalize(root).map_err(|_| IndexError::NotADirectory(root.to_path_buf()))?;
        if !root.is_dir() {
            return Err(IndexError::NotADirectory(root));
        }
        let filter = IgnoreFilter::new(&opts.ignore_globs)?;
        let root_id = self
            .store
            .upsert_root(&root, opts.kind, opts.watch, opts.index_content)?;
        // Strictly increasing per root, so "not seen in this run" is exact
        // even when two runs fall in the same second.
        let seen_at = now().max(self.store.max_last_seen(root_id)? + 1);

        let mut report = IndexReport {
            root: root.clone(),
            ..Default::default()
        };

        let skip_hidden = opts.skip_hidden;
        let walker = WalkDir::new(&root)
            .min_depth(1)
            .max_depth(if opts.recursive { usize::MAX } else { 1 })
            .follow_links(false)
            .into_iter()
            .filter_entry(move |e| {
                let name = e.file_name().to_string_lossy();
                !(skip_hidden && name.starts_with('.')) && !name.contains(TMP_MARKER)
            });

        let mut tx = self
            .store
            .conn()
            .unchecked_transaction()
            .map_err(StoreError::from)?;
        let mut in_batch = 0usize;
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    report.errors.push(e.to_string());
                    continue;
                }
            };
            if !entry.file_type().is_file()
                || filter.is_ignored(entry.path())
                || self.store.is_own_file(entry.path())
            {
                continue;
            }
            report.scanned += 1;
            let path = entry.path();
            match self.index_file_inner(path, root_id, opts, seen_at) {
                Ok(outcome) => {
                    record(&mut report, &outcome);
                    progress(path, &outcome);
                }
                Err(e) => report.errors.push(format!("{}: {e}", path.display())),
            }
            in_batch += 1;
            if in_batch >= opts.batch_size.max(1) {
                tx.commit().map_err(StoreError::from)?;
                tx = self
                    .store
                    .conn()
                    .unchecked_transaction()
                    .map_err(StoreError::from)?;
                in_batch = 0;
            }
        }
        report.removed = self.store.remove_unseen(root_id, seen_at)? as u64;
        tx.commit().map_err(StoreError::from)?;
        report.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// Index (or refresh) a single file. Used by `tidy index <file>` and for
    /// incremental updates after a watcher event or a move. If no registered
    /// root contains the file (e.g. a rule moved it to `~/Pictures/...`), its
    /// folder is registered as a non-watch root so the file gets a home.
    pub fn index_file(&self, path: &Path, opts: &IndexOptions) -> Result<FileOutcome, IndexError> {
        let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let opts = &*opts.for_file(&path);
        if !path.is_file() {
            return Err(IndexError::NotAFile(path));
        }
        let root_id = match self.store.root_for_path(&path)? {
            Some(id) => id,
            None => {
                let parent = path.parent().unwrap_or_else(|| Path::new(""));
                self.store
                    .upsert_root(parent, opts.kind, false, opts.index_content)?
            }
        };
        Ok(self.index_file_inner(&path, root_id, opts, now())?)
    }

    /// Reflect a move in the index: drop the row at `from`, index `to`.
    /// Called after watch/confirm/undo moves; failures are the caller's to
    /// log (a stale index row must never block a move).
    pub fn moved(
        &self,
        from: &Path,
        to: &Path,
        opts: &IndexOptions,
    ) -> Result<FileOutcome, IndexError> {
        self.store.remove_file(from)?;
        if let Ok(canon) = dunce::canonicalize(from.parent().unwrap_or(from)) {
            if let Some(name) = from.file_name() {
                self.store.remove_file(&canon.join(name))?;
            }
        }
        self.index_file(to, opts)
    }

    /// (M9) Reflect an encryption: the row of `from` (indexed right before)
    /// keeps its text, FTS entry and vector and now points at `zip`. If
    /// there was no row, the zip is indexed by name only.
    pub fn vaulted(
        &self,
        from: &Path,
        zip: &Path,
        opts: &IndexOptions,
    ) -> Result<FileOutcome, IndexError> {
        let zip = dunce::canonicalize(zip).unwrap_or_else(|_| zip.to_path_buf());
        let opts = &*opts.for_file(&zip);
        let meta = std::fs::metadata(&zip).map_err(|_| IndexError::NotAFile(zip.clone()))?;
        let root_id = match self.store.root_for_path(&zip)? {
            Some(id) => id,
            None => {
                let parent = zip.parent().unwrap_or_else(|| Path::new(""));
                self.store
                    .upsert_root(parent, opts.kind, false, opts.index_content)?
            }
        };
        let hash = hash_file(&zip).ok();
        let mtime = meta.modified().map(to_unix).unwrap_or(0);
        let mut froms = vec![from.to_path_buf()];
        if let (Ok(dir), Some(name)) = (
            dunce::canonicalize(from.parent().unwrap_or(from)),
            from.file_name(),
        ) {
            froms.push(dir.join(name));
        }
        for f in &froms {
            if self
                .store
                .mark_vaulted(f, &zip, root_id, meta.len(), mtime, hash.as_deref())?
            {
                return Ok(FileOutcome::Updated(TextStatus::Ok));
            }
        }
        let out = self.index_file_inner(&zip, root_id, opts, now())?;
        self.store.set_vault_flag(&zip)?;
        Ok(out)
    }

    fn index_file_inner(
        &self,
        path: &Path,
        root_id: i64,
        opts: &IndexOptions,
        seen_at: i64,
    ) -> Result<FileOutcome, StoreError> {
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                return Err(StoreError::Sqlite {
                    source: rusqlite::Error::ToSqlConversionFailure(Box::new(e)),
                })
            }
        };
        let size = meta.len();
        let mtime = meta.modified().map(to_unix).unwrap_or(0);
        let existing = self.store.file_stamp(path)?;
        let too_big = size > opts.max_content_bytes;
        if let Some(stamp) = &existing {
            // (M9) An encrypted vault zip keeps the text indexed before it
            // was locked; reading the zip would only lose it.
            if stamp.vault {
                self.store.touch_file(stamp.id, seen_at)?;
                return Ok(FileOutcome::Unchanged);
            }
            // Unchanged — unless body text is wanted now and was never
            // extracted (a NAS folder entering its content window).
            let text_wanted = opts.index_content && stamp.text_none && !too_big;
            if stamp.size == size && stamp.mtime == mtime && !text_wanted {
                if let (Some(d), true) = (&opts.pii, stamp.pii_unjudged) {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let text = self.store.file_content(stamp.id)?;
                    self.store
                        .set_file_pii(stamp.id, &d.judge(&name, text.as_deref()))?;
                }
                self.store.touch_file(stamp.id, seen_at)?;
                return Ok(FileOutcome::Unchanged);
            }
        }
        // Content reads of a share take one of the NAS IO slots.
        let _slot = opts
            .reads_network()
            .then(|| crate::netio::network_io().acquire());

        let ext = ext_lower(path);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let hash_too_big = opts.max_hash_bytes.is_some_and(|m| size > m);
        let hash = if too_big || hash_too_big {
            None
        } else {
            hash_file(path).ok()
        };

        let (text_status, content) = if !opts.index_content {
            (TextStatus::None, None)
        } else if too_big {
            (TextStatus::Skipped, None)
        } else {
            match self.extractors.extract(path) {
                None => (TextStatus::Unsupported, None),
                Some(Ok(text)) => (
                    TextStatus::Ok,
                    Some(truncate_chars(text, opts.max_text_chars)),
                ),
                Some(Err(e)) => {
                    tracing::debug!(path = %path.display(), "text extraction failed: {e}");
                    (TextStatus::Failed, None)
                }
            }
        };
        // (M15) Images and scanned PDFs wait for text recognition, unless
        // the same content was recognized before.
        let (mut text_status, mut content) = (text_status, content);
        let mut ocr = None;
        if super::ocr::is_target(
            &ext,
            size,
            text_status,
            content.as_deref(),
            opts.ocr_max_image_bytes,
        ) {
            match hash
                .as_deref()
                .map(|h| self.store.ocr_cache_get(h))
                .transpose()?
                .flatten()
            {
                Some(t) if !t.trim().is_empty() => {
                    text_status = TextStatus::Ok;
                    content = Some(truncate_chars(t, opts.max_text_chars));
                    ocr = Some(OcrState::Ok);
                }
                Some(_) => ocr = Some(OcrState::Empty),
                None => ocr = Some(OcrState::Pending),
            }
        }
        // (M16) Pictures wait for a description, unless the same content
        // was described before; the description follows any recognized text.
        let mut vision = None;
        let mut caption = None;
        if opts.index_content && super::vision::is_target(&ext, size, opts.vision_max_image_bytes) {
            match hash
                .as_deref()
                .map(|h| self.store.vision_cache_get(h))
                .transpose()?
                .flatten()
            {
                Some(c) if !c.is_empty() => {
                    let line = c.line();
                    content = Some(crate::store::compose_body(
                        content.as_deref().unwrap_or(""),
                        Some(&line),
                    ));
                    text_status = TextStatus::Ok;
                    caption = Some(line);
                    vision = Some(VisionState::Ok);
                }
                Some(_) => vision = Some(VisionState::Empty),
                None => vision = Some(VisionState::Pending),
            }
        }
        let excerpt = content
            .as_deref()
            .map(|c| c.chars().take(500).collect::<String>())
            .filter(|e| !e.is_empty());
        let pii = opts
            .pii
            .as_ref()
            .map(|d| d.judge(&name, content.as_deref()));

        self.store.upsert_file(
            &NewFile {
                path: path.to_path_buf(),
                root_id,
                name,
                mime: mime_for_ext(&ext).map(str::to_string),
                ext: (!ext.is_empty()).then_some(ext),
                size,
                mtime,
                ctime: meta.created().ok().map(to_unix),
                hash,
                text_status,
                excerpt,
                content,
                pii,
                ocr,
                vision,
                caption,
            },
            seen_at,
        )?;
        Ok(if existing.is_some() {
            FileOutcome::Updated(text_status)
        } else {
            FileOutcome::Added(text_status)
        })
    }
}

fn record(r: &mut IndexReport, o: &FileOutcome) {
    let status = match o {
        FileOutcome::Added(s) => {
            r.added += 1;
            *s
        }
        FileOutcome::Updated(s) => {
            r.updated += 1;
            *s
        }
        FileOutcome::Unchanged => {
            r.unchanged += 1;
            return;
        }
    };
    match status {
        TextStatus::Ok => r.text_ok += 1,
        TextStatus::Failed => r.text_failed += 1,
        TextStatus::Unsupported => r.text_unsupported += 1,
        TextStatus::Skipped => r.text_skipped += 1,
        TextStatus::None => {}
    }
}

fn truncate_chars(mut s: String, max: usize) -> String {
    if let Some((idx, _)) = s.char_indices().nth(max) {
        s.truncate(idx);
    }
    s
}

fn to_unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now() -> i64 {
    to_unix(SystemTime::now())
}

/// Small ext → MIME map for the common cases; `None` for the rest.
pub fn mime_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "hwp" => "application/x-hwp",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "zip" => "application/zip",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate_chars("가나다라".into(), 2), "가나");
        assert_eq!(truncate_chars("ab".into(), 5), "ab");
    }
}
