//! Work-context curation (SPEC §14.3, M10): which customer's which issue a
//! file belongs to, and a folder layout built from that
//! (`{entity}/{yyyymmdd}_{issue}/{doc_kind}`, `기타/{topic}` otherwise).
//!
//! The AI reads file names, dates, folders and — only with
//! `ai.send_content` — the start of the text, in batches, together with the
//! context dictionary. Answers are cached per file content in `curation`, so
//! only new or changed files cost a request. Files below `min_confidence`
//! stay "미분류" (not moved unless the user assigns them). Files of the same
//! customer + issue whose dates are within `merge_days` share the earliest
//! date. Nothing here moves a file: plans go through the restructure
//! preview and are applied, on approval, as one undoable batch (CLAUDE.md
//! rules 1, 3, 6); new files in watch folders go through the engine.

pub mod context;
pub mod customers;
pub mod folder;
pub mod template;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use context::{ContextDict, ContextError, ContextSuggestion, DocKind, Entity, IssueType};
pub use customers::{customer_dirs, customer_names, customer_overview, CustomerStat};
pub use folder::{FolderUnit, FOLDER_SCAN_CAP};
pub use template::{clean, FolderTemplate, TemplateVals};

use crate::ai::{estimate_cost_usd, prompts, AiError, Usage};
use crate::classify::{AiCall, AiClassifier};
use crate::config::{Config, CurateCfg, SecurityMode};
use crate::mover::{hash_file, zip_name_for, TMP_MARKER};
use crate::report::restructure::{ContextNote, Plan, PlanSource, PlannedMove};
use crate::store::{Store, StoreError};
use crate::watcher::IgnoreFilter;

#[derive(Debug, Error)]
pub enum CurateError {
    #[error(transparent)]
    Context(#[from] ContextError),
    #[error("폴더 틀: {0}")]
    Template(String),
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("not a folder: {0}")]
    NotADirectory(PathBuf),
    #[error("plan item {0} not found")]
    NoSuchMove(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurationSource {
    Ai,
    /// The user's correction (wins over a later AI answer).
    User,
}

impl CurationSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CurationSource::Ai => "ai",
            CurationSource::User => "user",
        }
    }
}

/// The judgement for one file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curation {
    pub entity: Option<String>,
    pub issue: String,
    /// The issue's date from the text / name (`None` → modified date).
    pub date: Option<NaiveDate>,
    pub doc_kind: String,
    /// "큰 주제/작은 주제" when there is no customer.
    pub topic: Option<String>,
    pub confidence: f64,
    pub reason: String,
    pub source: CurationSource,
}

/// What is known about a file before asking.
#[derive(Debug, Clone)]
pub struct CurateInput {
    pub path: PathBuf,
    /// `curation.key`: "<content hash>|<lower-case name>", or "meta:…"
    /// for files too big to hash.
    pub key: String,
    pub name: String,
    pub ext: String,
    /// (M12) Folder relative to the folder being organized
    /// ("가온누리원 로그/tools/go"; "." directly in it) — path context.
    pub rel_dir: String,
    pub size: u64,
    pub modified: NaiveDate,
    pub excerpt: Option<String>,
    /// (M12) Set for a folder item (moved as a whole).
    pub unit: Option<FolderUnit>,
}

/// Biggest file hashed for the cache key (bigger ones use name/size/mtime).
const MAX_HASH_BYTES: u64 = 512 * 1024 * 1024;
/// Rough characters per token for Korean/English mixes (cost estimate).
const CHARS_PER_TOKEN: f64 = 2.5;
/// Answer tokens per file (estimate).
const OUT_TOKENS_PER_FILE: u64 = 70;
/// Answer budget per file in one curate call (Korean reasons are
/// token-heavy; the batch is halved and retried when it still overflows).
const CURATE_TOKENS_PER_FILE: u32 = 400;
const CURATE_MAX_TOKENS: u32 = 16_000;

/// Time budget for one curate batch: a fixed part plus a few seconds per
/// file (long Korean answers from a slow model), capped well under the
/// HTTP ceiling. A batch that still times out is halved and retried.
fn curate_timeout(files: usize) -> std::time::Duration {
    std::time::Duration::from_secs((60 + 6 * files as u64).min(300))
}
/// Files sent to "바탕화면에서 뽑기".
const MAX_EXTRACT_FILES: usize = 200;

impl CurateInput {
    /// Gather facts about `path`; the start of the text only when
    /// `excerpt_chars > 0` (i.e. `ai.send_content`). Uses the index where
    /// it is current (hash, text), reads the file otherwise. Read-only.
    pub fn from_path(
        path: &Path,
        root: Option<&Path>,
        store: Option<&Store>,
        excerpt_chars: usize,
        max_bytes: u64,
    ) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        let size = meta.len();
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        let mtime_s = mtime
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let modified = DateTime::<Local>::from(mtime).date_naive();
        let row = store
            .and_then(|s| s.get_file(path).ok().flatten())
            .filter(|f| f.size == size && f.mtime == mtime_s);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The name is part of what the AI judges, so it is part of the key:
        // identical copies under different names are analysed separately.
        let key = match row.as_ref().and_then(|f| f.hash.clone()) {
            Some(h) => format!("{h}|{}", name.to_lowercase()),
            None if size <= MAX_HASH_BYTES => match hash_file(path) {
                Ok(h) => format!("{h}|{}", name.to_lowercase()),
                Err(_) => format!("meta:{}|{size}|{mtime_s}", path.display()),
            },
            None => format!("meta:{}|{size}|{mtime_s}", path.display()),
        };
        let excerpt = if excerpt_chars == 0 {
            None
        } else {
            let stored = row
                .as_ref()
                .and_then(|f| store.and_then(|s| s.file_content(f.id).ok().flatten()));
            let text = match stored {
                Some(t) => Some(t),
                None if size <= max_bytes => {
                    match crate::index::Extractors::default_set().extract(path) {
                        Some(Ok(t)) => Some(t),
                        _ => None,
                    }
                }
                None => None,
            };
            text.map(|t| t.chars().take(excerpt_chars).collect())
        };
        Ok(Self {
            path: path.to_path_buf(),
            key,
            name,
            ext: crate::paths::ext_lower(path),
            rel_dir: rel_dir(path, root),
            size,
            modified,
            excerpt,
            unit: None,
        })
    }

    /// (M12) A folder item.
    pub fn from_unit(u: FolderUnit) -> Self {
        Self {
            path: u.path.clone(),
            key: u.key.clone(),
            name: u.name.clone(),
            ext: String::new(),
            rel_dir: u.rel.clone(),
            size: u.bytes,
            modified: u.newest_date(),
            excerpt: u.doc.clone(),
            unit: Some(u),
        }
    }

    pub fn is_folder(&self) -> bool {
        self.unit.is_some()
    }
}

/// (M12) Folders that are organized results, never folder items: rule
/// destinations (resolved against `anchor`) and every "모으기" root.
pub fn skip_dirs(cfg: &Config, rules: &crate::classify::RuleSet, anchor: &Path) -> Vec<PathBuf> {
    let mut out = rules.dest_roots_for(&[anchor.to_path_buf()]);
    for r in crate::report::backlog::organize_roots(&cfg.watches) {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b || matches!((dunce::canonicalize(a), dunce::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// `path`'s folder relative to `root` ("." directly in it); without a
/// root, just the parent folder's name.
fn rel_dir(path: &Path, root: Option<&Path>) -> String {
    let parent = path.parent().unwrap_or(Path::new(""));
    if let Some(r) = root {
        if let Ok(rel) = parent.strip_prefix(r) {
            let s = rel.to_string_lossy().replace('\\', "/");
            return if s.is_empty() { ".".into() } else { s };
        }
    }
    parent
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

/// What a work-context run looks at in one folder (SPEC §14.7): the files
/// directly in it and its sub-folders as whole items.
#[derive(Debug, Default)]
pub struct Items {
    pub inputs: Vec<CurateInput>,
    /// Sub-folders still changing (copy / unpack in progress): left out.
    pub unsettled: Vec<PathBuf>,
}

impl Items {
    pub fn folders(&self) -> usize {
        self.inputs.iter().filter(|i| i.is_folder()).count()
    }
}

/// "파일 N개, 예상 토큰/비용" before analysing (SPEC §14.3).
#[derive(Debug, Clone, Serialize)]
pub struct Estimate {
    pub files: usize,
    /// Already analysed (cache hit): free.
    pub cached: usize,
    pub to_analyze: usize,
    pub requests: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `None` when the model's price is unknown.
    pub cost_usd: Option<f64>,
    pub provider: String,
    pub model: String,
}

/// Candidates from "바탕화면에서 뽑기" (nothing saved until confirmed).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextCandidates {
    pub entities: Vec<EntityCandidate>,
    pub issue_types: Vec<NameCandidate>,
    pub doc_kinds: Vec<KindCandidate>,
    pub files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityCandidate {
    pub name: String,
    pub aliases: Vec<String>,
    pub files: u64,
    pub reason: String,
    /// Already in the dictionary.
    pub known: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameCandidate {
    pub name: String,
    pub known: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KindCandidate {
    pub name: String,
    pub ext: Vec<String>,
    pub known: bool,
}

/// Where a single new file would go (watch-time curation).
#[derive(Debug, Clone)]
pub struct Placement {
    pub curation: Curation,
    pub dest_dir: PathBuf,
    /// Date after merging with earlier files of the same issue.
    pub date: NaiveDate,
    /// The customer / issue folder already exists (auto move allowed).
    pub known_folder: bool,
    /// The customer is in the dictionary.
    pub known_entity: bool,
}

pub struct Curator {
    dict: ContextDict,
    cfg: CurateCfg,
    template: FolderTemplate,
    other: FolderTemplate,
    send_content: bool,
    vault_mode: bool,
    max_bytes: u64,
}

impl std::fmt::Debug for Curator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Curator")
            .field("template", &self.template.as_str())
            .field("entities", &self.dict.entities.len())
            .finish()
    }
}

impl Curator {
    pub fn new(dict: ContextDict, cfg: CurateCfg, send_content: bool) -> Result<Self, CurateError> {
        let template = FolderTemplate::parse(&cfg.template).map_err(CurateError::Template)?;
        let other = FolderTemplate::parse(&cfg.other_template).map_err(CurateError::Template)?;
        Ok(Self {
            dict,
            cfg,
            template,
            other,
            send_content,
            vault_mode: false,
            max_bytes: 200 * 1024 * 1024,
        })
    }

    /// From `[curate]`, the dictionary file, `[ai] send_content` and the
    /// vault mode of `[security]`.
    pub fn from_config(cfg: &Config) -> Result<Self, CurateError> {
        let dict = ContextDict::load(&cfg.curate.context)?;
        let mut c = Self::new(dict, cfg.curate.clone(), cfg.ai.send_content())?;
        c.vault_mode = cfg.security.mode == SecurityMode::Vault;
        c.max_bytes = cfg.index.max_content_size_mb * 1024 * 1024;
        Ok(c)
    }

    pub fn dict(&self) -> &ContextDict {
        &self.dict
    }

    pub fn cfg(&self) -> &CurateCfg {
        &self.cfg
    }

    /// Characters of text sent per file (0 without `send_content`).
    pub fn excerpt_chars(&self) -> usize {
        if self.send_content {
            self.cfg.excerpt_chars
        } else {
            0
        }
    }

    pub fn input(&self, path: &Path, store: Option<&Store>) -> std::io::Result<CurateInput> {
        CurateInput::from_path(path, None, store, self.excerpt_chars(), self.max_bytes)
    }

    /// (M12) Like [`Curator::input`], with the folder relative to `root`
    /// (the watch folder / the folder being organized) as path context.
    pub fn input_in(
        &self,
        path: &Path,
        root: &Path,
        store: Option<&Store>,
    ) -> std::io::Result<CurateInput> {
        CurateInput::from_path(
            path,
            Some(root),
            store,
            self.excerpt_chars(),
            self.max_bytes,
        )
    }

    /// Settle time for folder items.
    pub fn settle(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.cfg.folder_settle_secs)
    }

    /// Folder names results of work-context organizing get directly in an
    /// `in_place` watch folder: customers (dictionary + cache) and the
    /// fixed first part of the "no customer" layout ("기타").
    pub fn output_names(&self, store: Option<&Store>) -> Vec<String> {
        let mut out: Vec<String> = self.dict.entities.iter().map(|e| clean(&e.name)).collect();
        if let Some(s) = store {
            for (e, _) in s.curation_groups().unwrap_or_default() {
                let c = clean(&e);
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        if let Some(first) = self.cfg.other_template.split(['/', '\\']).next() {
            if !first.contains('{') && !first.trim().is_empty() {
                out.push(clean(first));
            }
        }
        out
    }

    /// (M12) Sub-folders of `root` that are organizing candidates: not
    /// hidden / excluded / protected / in `skip`, not an organized result
    /// (Tidy moved files into it, or — when results land in `root` itself —
    /// named like a customer or "기타"), not empty. Unsettled ones come
    /// back separately.
    #[allow(clippy::too_many_arguments)]
    pub fn folder_units(
        &self,
        root: &Path,
        anchor: &Path,
        filter: &IgnoreFilter,
        protected: &[PathBuf],
        skip: &[PathBuf],
        store: Option<&Store>,
        now: std::time::SystemTime,
    ) -> (Vec<FolderUnit>, Vec<PathBuf>) {
        let names = if same_path(root, anchor) {
            self.output_names(store)
        } else {
            Vec::new()
        };
        let mut units = Vec::new();
        let mut unsettled = Vec::new();
        let Ok(rd) = std::fs::read_dir(root) else {
            return (units, unsettled);
        };
        let mut dirs: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
        dirs.sort();
        for d in dirs {
            let name = d
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with('.')
                || name.contains(TMP_MARKER)
                || filter.in_excluded_dir(&d)
                || filter.is_ignored(&d)
                || protected
                    .iter()
                    .chain(skip)
                    .any(|p| d.starts_with(p) || p.starts_with(&d))
                || names.iter().any(|n| n == &name)
                || store.is_some_and(|s| s.has_moves_into(&d).unwrap_or(false))
            {
                continue;
            }
            let doc = if self.send_content { 300 } else { 0 };
            let Ok(u) = FolderUnit::scan(&d, root, filter, doc) else {
                continue;
            };
            if u.files == 0 {
                continue;
            }
            if !u.is_settled(self.settle(), now) {
                unsettled.push(d);
                continue;
            }
            units.push(u);
        }
        (units, unsettled)
    }

    /// (M12) Everything a work-context run looks at in `root`: files
    /// directly in it, and its sub-folders as whole items.
    #[allow(clippy::too_many_arguments)]
    pub fn items_in(
        &self,
        root: &Path,
        anchor: &Path,
        filter: &IgnoreFilter,
        protected: &[PathBuf],
        skip: &[PathBuf],
        store: &Store,
    ) -> Items {
        let mut inputs: Vec<CurateInput> = Self::files_in(root, false, filter, protected)
            .iter()
            .filter(|p| !store.get_file(p).ok().flatten().is_some_and(|f| f.vault))
            .filter_map(|p| self.input_in(p, root, Some(store)).ok())
            .collect();
        let (units, unsettled) = self.folder_units(
            root,
            anchor,
            filter,
            protected,
            skip,
            Some(store),
            std::time::SystemTime::now(),
        );
        inputs.extend(units.into_iter().map(CurateInput::from_unit));
        Items { inputs, unsettled }
    }

    /// Files directly in `root` (and below with `recursive`), skipping
    /// hidden files, scratch copies, ignored and protected paths.
    pub fn files_in(
        root: &Path,
        recursive: bool,
        filter: &IgnoreFilter,
        protected: &[PathBuf],
    ) -> Vec<PathBuf> {
        walkdir::WalkDir::new(root)
            .min_depth(1)
            .max_depth(if recursive { usize::MAX } else { 1 })
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                let n = e.file_name().to_string_lossy();
                !n.starts_with('.') && !n.contains(TMP_MARKER) && !filter.in_excluded_dir(e.path())
            })
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| !filter.is_ignored(p) && !protected.iter().any(|x| p.starts_with(x)))
            .collect()
    }

    fn file_line(&self, id: usize, i: &CurateInput) -> prompts::CurateFileLine {
        prompts::CurateFileLine {
            id,
            name: i.name.clone(),
            rel_dir: i.rel_dir.clone(),
            size: i.size,
            modified: i.modified.format("%Y-%m-%d").to_string(),
            excerpt: i.excerpt.clone(),
            folder: i.unit.as_ref().map(FolderUnit::line),
        }
    }

    /// Cost before analysing: cached files are free.
    pub fn estimate(
        &self,
        store: &Store,
        inputs: &[CurateInput],
        provider: &str,
        model: &str,
    ) -> Estimate {
        let misses: Vec<&CurateInput> = inputs
            .iter()
            .filter(|i| !matches!(store.get_curation(&i.key), Ok(Some(_))))
            .collect();
        let batch = self.cfg.batch_size.max(1);
        let requests = misses.len().div_ceil(batch);
        let fixed = (prompts::CURATE_SYSTEM.chars().count()
            + self.dict.prompt_text().chars().count()) as u64
            * requests as u64;
        let per_file: u64 = misses
            .iter()
            .enumerate()
            .map(|(n, i)| {
                prompts::curate_user("", &[self.file_line(n + 1, i)])
                    .chars()
                    .count() as u64
            })
            .sum();
        let input_tokens = ((fixed + per_file) as f64 / CHARS_PER_TOKEN).ceil() as u64;
        let output_tokens = misses.len() as u64 * OUT_TOKENS_PER_FILE;
        Estimate {
            files: inputs.len(),
            cached: inputs.len() - misses.len(),
            to_analyze: misses.len(),
            requests,
            input_tokens,
            output_tokens,
            cost_usd: estimate_cost_usd(
                provider,
                model,
                Usage {
                    input_tokens: Some(input_tokens),
                    output_tokens: Some(output_tokens),
                },
            ),
            provider: provider.into(),
            model: model.into(),
        }
    }

    /// Judge every input: cached answers first, the rest in batches. A
    /// failed request is an error (earlier batches stay cached).
    pub fn analyze(
        &self,
        ai: &AiClassifier,
        store: &Store,
        inputs: &[CurateInput],
    ) -> Result<Vec<Option<Curation>>, CurateError> {
        let mut out: Vec<Option<Curation>> = Vec::with_capacity(inputs.len());
        let mut misses: Vec<usize> = Vec::new();
        for (n, i) in inputs.iter().enumerate() {
            match store.get_curation(&i.key)? {
                Some(c) => out.push(Some(self.canonical(c))),
                None => {
                    out.push(None);
                    misses.push(n);
                }
            }
        }
        for chunk in misses.chunks(self.cfg.batch_size.max(1)) {
            self.analyze_chunk(ai, store, inputs, chunk, &mut out)?;
        }
        Ok(out)
    }

    /// One AI call for `chunk`. When the answer is cut off at `max_tokens`
    /// (long reasons, many files) the chunk is halved and both halves
    /// retried; a single file that still does not fit is left
    /// unclassified instead of failing the whole run.
    fn analyze_chunk(
        &self,
        ai: &AiClassifier,
        store: &Store,
        inputs: &[CurateInput],
        chunk: &[usize],
        out: &mut [Option<Curation>],
    ) -> Result<(), CurateError> {
        if chunk.is_empty() {
            return Ok(());
        }
        let lines: Vec<prompts::CurateFileLine> = chunk
            .iter()
            .enumerate()
            .map(|(k, &n)| self.file_line(k + 1, &inputs[n]))
            .collect();
        let user = prompts::curate_user(&self.dict.prompt_text(), &lines);
        let call = AiCall {
            purpose: "curate",
            system: prompts::CURATE_SYSTEM,
            user: &user,
            schema: &prompts::curate_schema(),
            max_tokens: (chunk.len() as u32 * CURATE_TOKENS_PER_FILE)
                .clamp(2048, CURATE_MAX_TOKENS),
            timeout: curate_timeout(chunk.len()),
        };
        let n_files = chunk.len();
        let answers = match ai.call(store, &call, |v| {
            let m = parse_answer(&v, n_files);
            let summary = format!("{}/{n_files}개 분석", m.len());
            Ok((m, Some(summary)))
        }) {
            Ok(a) => a,
            Err(e @ (AiError::Truncated | AiError::Timeout(_))) if chunk.len() > 1 => {
                let (lo, hi) = chunk.split_at(chunk.len() / 2);
                tracing::warn!(files = chunk.len(), error = %e, "curate batch too big; retrying in halves");
                self.analyze_chunk(ai, store, inputs, lo, out)?;
                return self.analyze_chunk(ai, store, inputs, hi, out);
            }
            Err(AiError::Truncated) => {
                tracing::warn!(path = %inputs[chunk[0]].path.display(), "curate answer truncated for a single file; left unclassified");
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        for (k, &n) in chunk.iter().enumerate() {
            if let Some(c) = answers.get(&(k + 1)) {
                let c = self.canonical(c.clone());
                store.put_curation(&inputs[n].key, &inputs[n].path, &c, Some(ai.model()))?;
                out[n] = Some(c);
            }
        }
        Ok(())
    }

    /// Use the dictionary's name for a known customer or alias.
    fn canonical(&self, mut c: Curation) -> Curation {
        if let Some(e) = c.entity.as_deref() {
            if let Some(canon) = self.dict.canonical_entity(e) {
                c.entity = Some(canon.to_string());
            }
        }
        c
    }

    pub fn is_classified(&self, c: &Curation) -> bool {
        c.source == CurationSource::User || c.confidence >= self.cfg.min_confidence
    }

    /// The date a file's group uses: its own date joined with the dates of
    /// the same customer + issue (`others`, plus earlier files in the
    /// store) — the earliest one within `merge_days` of a chain.
    pub fn group_date(&self, own: NaiveDate, others: &[NaiveDate]) -> NaiveDate {
        let mut all: Vec<NaiveDate> = others.to_vec();
        all.push(own);
        all.sort();
        all.dedup();
        // Walk clusters: a new cluster starts after a gap > merge_days.
        let gap = i64::from(self.cfg.merge_days);
        let mut start = all[0];
        let mut prev = all[0];
        for d in all {
            if (d - prev).num_days() > gap {
                start = d;
            }
            if d == own {
                return start;
            }
            prev = d;
        }
        own
    }

    pub fn dest_dir(&self, c: &Curation, date: NaiveDate, anchor: &Path) -> PathBuf {
        self.dest_for(c, date, anchor, None)
    }

    /// Where an item goes: a file's folder, or (M12) a folder item's new
    /// path — `{folder}` in the layout, else the folder name appended.
    pub fn dest_for(
        &self,
        c: &Curation,
        date: NaiveDate,
        anchor: &Path,
        folder: Option<&str>,
    ) -> PathBuf {
        let (t, vals) = match c.entity.as_deref().filter(|e| !e.trim().is_empty()) {
            Some(e) => (
                &self.template,
                TemplateVals {
                    entity: Some(e),
                    issue: &c.issue,
                    doc_kind: &c.doc_kind,
                    topic: None,
                    date: Some(date),
                    folder,
                },
            ),
            None => (
                &self.other,
                TemplateVals {
                    entity: None,
                    issue: &c.issue,
                    doc_kind: &c.doc_kind,
                    topic: c.topic.as_deref(),
                    date: Some(date),
                    folder,
                },
            ),
        };
        let dir = t.render(&vals, anchor);
        match folder {
            Some(f) if !t.uses("folder") => dir.join(clean(f)),
            _ => dir,
        }
    }

    /// A work-context plan for `root` (nothing moves): the files directly
    /// in it and (M12) its sub-folders as whole items. `skip`: folders that
    /// are organized results (rule destinations, organize roots).
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        &self,
        ai: &AiClassifier,
        store: &Store,
        root: &Path,
        anchor: &Path,
        filter: &IgnoreFilter,
        protected: &[PathBuf],
        skip: &[PathBuf],
    ) -> Result<Plan, CurateError> {
        let root = dunce::canonicalize(root)
            .map_err(|_| CurateError::NotADirectory(root.to_path_buf()))?;
        let items = self.items_in(&root, anchor, filter, protected, skip, store);
        let unsettled = items.unsettled.len();
        let inputs = items.inputs;
        let judged = self.analyze(ai, store, &inputs)?;
        let mut moves = Vec::new();
        for (i, c) in inputs.iter().zip(judged) {
            let note = match c {
                Some(c) => note_for(i, &c, !self.is_classified(&c)),
                None => unanswered(i),
            };
            let encrypt = !i.is_folder()
                && self.vault_mode
                && store
                    .get_file(&i.path)
                    .ok()
                    .flatten()
                    .and_then(|f| f.pii_flags)
                    .is_some_and(|f| f != 0);
            moves.push(planned(moves.len() as u32 + 1, i, note, encrypt));
        }
        let mut plan = Plan {
            id: 0,
            root: root.clone(),
            source: PlanSource::Context,
            created_at: chrono::Utc::now().timestamp(),
            summary: String::new(),
            folders: Vec::new(),
            moves,
            rejected: Vec::new(),
            applied_batch: None,
            anchor: Some(anchor.to_path_buf()),
        };
        self.relayout(&mut plan, Some(store));
        if unsettled > 0 {
            plan.summary.push_str(&format!(
                " 아직 바뀌는 중인 폴더 {unsettled}개(복사·압축 해제 중)는 뺐습니다."
            ));
        }
        Ok(plan)
    }

    /// (M12) "파일별로 나누기": replace folder item `id` by its files (all
    /// of them, any depth), each judged on its own with its path inside
    /// the organized folder as context.
    pub fn split_folder(
        &self,
        ai: &AiClassifier,
        store: &Store,
        plan: &mut Plan,
        id: u32,
        filter: &IgnoreFilter,
    ) -> Result<usize, CurateError> {
        let pos = plan
            .moves
            .iter()
            .position(|m| m.id == id && m.is_dir)
            .ok_or(CurateError::NoSuchMove(id))?;
        let dir = plan.moves[pos].from.clone();
        let inputs: Vec<CurateInput> = Self::files_in(&dir, true, filter, &[])
            .iter()
            .filter_map(|p| self.input_in(p, &plan.root, Some(store)).ok())
            .collect();
        let judged = self.analyze(ai, store, &inputs)?;
        plan.moves.remove(pos);
        let first = plan.moves.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        for (next, (i, c)) in (first..).zip(inputs.iter().zip(judged)) {
            let note = match c {
                Some(c) => note_for(i, &c, !self.is_classified(&c)),
                None => unanswered(i),
            };
            plan.moves.push(planned(next, i, note, false));
        }
        self.relayout(plan, Some(store));
        Ok(inputs.len())
    }

    /// Recompute folder dates and destinations of every work-context item
    /// (after analysis and after each user edit).
    pub fn relayout(&self, plan: &mut Plan, store: Option<&Store>) {
        let anchor = plan.anchor.clone().unwrap_or_else(|| plan.root.clone());
        let parse = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
        // Own dates per (customer, issue) group in this plan.
        let mut groups: HashMap<(String, String), Vec<NaiveDate>> = HashMap::new();
        for m in &plan.moves {
            let Some(n) = &m.context else { continue };
            if let (Some(e), false) = (n.entity.as_deref(), n.unclassified) {
                if let Some(d) = parse(&n.own_date) {
                    groups
                        .entry((e.to_string(), n.issue.clone()))
                        .or_default()
                        .push(d);
                }
            }
        }
        let mut stored: HashMap<(String, String), Vec<NaiveDate>> = HashMap::new();
        let (mut placed, mut unclassified) = (0, 0);
        let mut entities: Vec<String> = Vec::new();
        let mut issues: Vec<(String, String)> = Vec::new();
        for m in plan.moves.iter_mut() {
            let Some(n) = m.context.as_mut() else {
                continue;
            };
            if n.unclassified {
                m.to = m.from.clone();
                unclassified += 1;
                continue;
            }
            let own = parse(&n.own_date).unwrap_or_else(|| Local::now().date_naive());
            let date = match n.entity.as_deref() {
                Some(e) => {
                    let k = (e.to_string(), n.issue.clone());
                    let mut others = groups.get(&k).cloned().unwrap_or_default();
                    if let Some(s) = store {
                        others.extend(
                            stored
                                .entry(k.clone())
                                .or_insert_with(|| {
                                    s.curation_dates(e, &n.issue).unwrap_or_default()
                                })
                                .iter(),
                        );
                    }
                    if !entities.contains(&k.0) {
                        entities.push(k.0.clone());
                    }
                    if !issues.contains(&k) {
                        issues.push(k);
                    }
                    self.group_date(own, &others)
                }
                None => own,
            };
            n.date = date.format("%Y-%m-%d").to_string();
            let c = Curation {
                entity: n.entity.clone(),
                issue: n.issue.clone(),
                date: Some(date),
                doc_kind: n.doc_kind.clone(),
                topic: n.topic.clone(),
                confidence: n.confidence,
                reason: n.reason.clone(),
                source: CurationSource::Ai,
            };
            let name = m
                .from
                .file_name()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            m.to = if m.is_dir {
                self.dest_for(&c, date, &anchor, Some(&name))
            } else {
                let dir = self.dest_dir(&c, date, &anchor);
                dir.join(if m.encrypt { zip_name_for(&name) } else { name })
            };
            m.reason = n.reason.clone();
            m.warnings.clear();
            if m.to.exists() && m.to != m.from {
                m.warnings.push(if m.is_dir {
                    "목적지에 같은 이름의 폴더가 있어 그 안에 합칩니다(같은 이름 파일은 '(1)')"
                        .into()
                } else {
                    "목적지에 같은 이름이 있어 '(1)'처럼 번호가 붙습니다".into()
                });
            }
            if m.encrypt {
                m.warnings
                    .push("개인정보 파일 — 암호화 zip으로 보관합니다".into());
            }
            if m.to == m.from {
                // Already where it belongs.
                m.warnings.push("이미 이 자리에 있습니다".into());
            } else {
                placed += 1;
            }
        }
        let folders = plan.moves.iter().filter(|m| m.is_dir).count();
        plan.summary = format!(
            "항목 {}개(폴더 {folders}개 통째 포함) 중 {placed}개를 고객사 {}곳 · 이슈 {}건과 기타 주제로 묶습니다. 미분류 {unclassified}개는 그대로 둡니다.",
            plan.moves.len(),
            entities.len(),
            issues.len(),
        );
    }

    /// The user changed an item in the preview: customer (`None` = 고객사
    /// 없음), issue, kind or topic. Re-lays out the plan, remembers the
    /// correction in the cache (`source = user`) and returns what the
    /// dictionary could learn (applied only when the user confirms).
    pub fn edit_move(
        &self,
        plan: &mut Plan,
        id: u32,
        edit: &MoveEdit,
        store: &Store,
    ) -> Result<Vec<ContextSuggestion>, CurateError> {
        let m = plan
            .moves
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(CurateError::NoSuchMove(id))?;
        let n = m.context.get_or_insert_with(Default::default);
        let entity = edit
            .entity
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(|e| self.dict.canonical_entity(e).unwrap_or(e).to_string());
        n.entity = entity.clone();
        if let Some(i) = edit
            .issue
            .as_deref()
            .map(str::trim)
            .filter(|i| !i.is_empty())
        {
            n.issue = i.to_string();
        }
        if n.issue.is_empty() {
            n.issue = "기타 이슈".into();
        }
        if let Some(k) = edit
            .doc_kind
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        {
            n.doc_kind = k.to_string();
        }
        if n.doc_kind.is_empty() {
            n.doc_kind = if m.is_dir {
                "자료".to_string()
            } else {
                self.dict
                    .doc_kind_for_ext(&crate::paths::ext_lower(&m.from))
                    .unwrap_or("기타 자료")
                    .to_string()
            };
        }
        if let Some(t) = edit
            .topic
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            n.topic = Some(t.to_string());
        }
        if n.entity.is_none() && n.topic.is_none() {
            n.topic = Some("미분류".into());
        }
        if let Some(d) = edit
            .date
            .as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
        {
            n.own_date = d.format("%Y-%m-%d").to_string();
        }
        n.unclassified = false;
        n.edited = true;
        let corrected = Curation {
            entity: n.entity.clone(),
            issue: n.issue.clone(),
            date: NaiveDate::parse_from_str(&n.own_date, "%Y-%m-%d").ok(),
            doc_kind: n.doc_kind.clone(),
            topic: n.topic.clone(),
            confidence: 1.0,
            reason: format!("사용자 지정 (AI: {})", n.reason),
            source: CurationSource::User,
        };
        let issue_changed = n.issue != n.ai_issue;
        let suggestions = self.dict.suggestions_for(
            entity.as_deref(),
            n.ai_entity.as_deref(),
            issue_changed.then_some(n.issue.as_str()),
        );
        if !n.key.is_empty() {
            store.put_curation(&n.key, &m.from, &corrected, None)?;
        }
        self.relayout(plan, Some(store));
        Ok(suggestions)
    }

    /// Where a new file in a watch folder goes (`None`: AI had no answer).
    pub fn place(
        &self,
        ai: &AiClassifier,
        store: &Store,
        input: &CurateInput,
        anchor: &Path,
    ) -> Result<Option<Placement>, CurateError> {
        let Some(c) = self
            .analyze(ai, store, std::slice::from_ref(input))?
            .pop()
            .flatten()
        else {
            return Ok(None);
        };
        let own = c.date.unwrap_or(input.modified);
        let date = match c.entity.as_deref() {
            Some(e) => self.group_date(own, &store.curation_dates(e, &c.issue)?),
            None => own,
        };
        let dest_dir = self.dest_dir(&c, date, anchor);
        let known_folder =
            dest_dir.is_dir() || dest_dir.parent().is_some_and(|p| p != anchor && p.is_dir());
        let known_entity = c
            .entity
            .as_deref()
            .is_some_and(|e| self.dict.canonical_entity(e).is_some());
        Ok(Some(Placement {
            curation: c,
            dest_dir,
            date,
            known_folder,
            known_entity,
        }))
    }

    /// "바탕화면에서 뽑기": candidate customers / issue types / kinds from
    /// the files' names and (with `send_content`) the start of their text.
    pub fn extract_context(
        &self,
        ai: &AiClassifier,
        store: &Store,
        inputs: &[CurateInput],
        top_folders: &[String],
    ) -> Result<ContextCandidates, CurateError> {
        let lines: Vec<prompts::CurateFileLine> = inputs
            .iter()
            .take(MAX_EXTRACT_FILES)
            .enumerate()
            .map(|(n, i)| {
                let mut l = self.file_line(n + 1, i);
                l.excerpt = l.excerpt.map(|e| e.chars().take(150).collect());
                l
            })
            .collect();
        let user = prompts::context_user(&self.dict.prompt_text(), top_folders, &lines);
        let call = AiCall {
            purpose: "context",
            system: prompts::CONTEXT_SYSTEM,
            user: &user,
            schema: &prompts::context_schema(),
            max_tokens: 4000,
            timeout: std::time::Duration::from_secs(120),
        };
        let dict = &self.dict;
        let mut out = ai.call(store, &call, |v| {
            let c = parse_candidates(&v, dict);
            let s = format!("고객사 후보 {}곳", c.entities.len());
            Ok((c, Some(s)))
        })?;
        out.files = lines.len();
        Ok(out)
    }
}

/// A user's change to one preview item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoveEdit {
    /// `None`/empty = no customer (→ 기타/{topic}).
    pub entity: Option<String>,
    pub issue: Option<String>,
    pub doc_kind: Option<String>,
    pub topic: Option<String>,
    /// (M12) The item's own date, YYYY-MM-DD (a folder's newest file by default).
    pub date: Option<String>,
}

fn note_for(i: &CurateInput, c: &Curation, unclassified: bool) -> ContextNote {
    ContextNote {
        entity: c.entity.clone(),
        issue: c.issue.clone(),
        date: String::new(),
        own_date: c.date.unwrap_or(i.modified).format("%Y-%m-%d").to_string(),
        doc_kind: c.doc_kind.clone(),
        topic: c.topic.clone(),
        confidence: c.confidence,
        reason: c.reason.clone(),
        ai_entity: c.entity.clone(),
        ai_issue: c.issue.clone(),
        unclassified,
        edited: c.source == CurationSource::User,
        key: i.key.clone(),
        tree: i.unit.as_ref().map(|u| inner_tree(&u.path)),
    }
}

/// No answer for this item (left in place).
fn unanswered(i: &CurateInput) -> ContextNote {
    ContextNote {
        issue: String::new(),
        own_date: i.modified.format("%Y-%m-%d").to_string(),
        reason: "AI 응답에 이 항목이 없었습니다".into(),
        unclassified: true,
        key: i.key.clone(),
        tree: i.unit.as_ref().map(|u| inner_tree(&u.path)),
        ..Default::default()
    }
}

fn planned(id: u32, i: &CurateInput, note: ContextNote, encrypt: bool) -> PlannedMove {
    PlannedMove {
        id,
        from: i.path.clone(),
        to: i.path.clone(),
        is_dir: i.is_folder(),
        files: i.unit.as_ref().map_or(1, |u| u.files as u64),
        bytes: i.size,
        reason: note.reason.clone(),
        warnings: Vec::new(),
        context: Some(note),
        encrypt,
    }
}

/// A folder item's inside, three levels deep (preview "펼치기").
fn inner_tree(dir: &Path) -> String {
    let opts = crate::report::ScanOptions {
        max_depth: Some(3),
        ..Default::default()
    };
    match crate::report::scan(dir, &opts) {
        Ok(r) => crate::report::render_tree(
            &r.tree,
            &crate::report::TreeOptions {
                max_depth: Some(3),
                show_files: true,
                show_size: false,
                max_files_per_dir: 6,
            },
        ),
        Err(e) => e.to_string(),
    }
}

/// Record the folder date an applied item got (watch-time files of the
/// same issue join it). Errors are logged only.
pub fn remember_applied(store: &Store, note: &ContextNote, new_path: &Path) {
    if note.key.is_empty() || note.unclassified {
        return;
    }
    let c = Curation {
        entity: note.entity.clone(),
        issue: note.issue.clone(),
        date: NaiveDate::parse_from_str(&note.date, "%Y-%m-%d").ok(),
        doc_kind: note.doc_kind.clone(),
        topic: note.topic.clone(),
        confidence: note.confidence,
        reason: note.reason.clone(),
        source: if note.edited {
            CurationSource::User
        } else {
            CurationSource::Ai
        },
    };
    if let Err(e) = store.put_curation(&note.key, new_path, &c, None) {
        tracing::warn!("curation cache update failed: {e}");
    }
}

fn str_or_null(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "null")
}

/// Validated answers by id (ids outside 1..=n are dropped).
fn parse_answer(v: &Value, n: usize) -> HashMap<usize, Curation> {
    let mut out = HashMap::new();
    let Some(files) = v.get("files").and_then(Value::as_array) else {
        return out;
    };
    for f in files {
        let Some(id) = f.get("id").and_then(Value::as_u64).map(|x| x as usize) else {
            continue;
        };
        if id == 0 || id > n {
            continue;
        }
        let issue = str_or_null(f, "issue")
            .map(|s| clean(&s))
            .unwrap_or_default();
        let entity = str_or_null(f, "entity")
            .map(|s| clean(&s))
            .filter(|s| !s.is_empty());
        let topic = str_or_null(f, "topic");
        out.insert(
            id,
            Curation {
                topic: if entity.is_none() {
                    topic.or_else(|| Some("미분류".into()))
                } else {
                    None
                },
                entity,
                issue: if issue.is_empty() {
                    "기타 이슈".into()
                } else {
                    issue
                },
                date: str_or_null(f, "date").and_then(|d| {
                    NaiveDate::parse_from_str(d.get(..10).unwrap_or(&d), "%Y-%m-%d").ok()
                }),
                doc_kind: str_or_null(f, "doc_kind")
                    .map(|s| clean(&s))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "기타 자료".into()),
                confidence: f
                    .get("confidence")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0),
                reason: str_or_null(f, "reason")
                    .map(|r| r.chars().take(200).collect())
                    .unwrap_or_default(),
                source: CurationSource::Ai,
            },
        );
    }
    out
}

fn parse_candidates(v: &Value, dict: &ContextDict) -> ContextCandidates {
    let strs = |x: Option<&Value>| -> Vec<String> {
        x.and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut c = ContextCandidates::default();
    for e in v
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = str_or_null(e, "name")
            .map(|n| clean(&n))
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        if c.entities.iter().any(|x| x.name == name) {
            continue;
        }
        c.entities.push(EntityCandidate {
            known: dict.canonical_entity(&name).is_some(),
            aliases: strs(e.get("aliases"))
                .into_iter()
                .filter(|a| *a != name)
                .collect(),
            files: e.get("files").and_then(Value::as_u64).unwrap_or(0),
            reason: str_or_null(e, "reason").unwrap_or_default(),
            name,
        });
    }
    for name in strs(v.get("issue_types")) {
        if !c.issue_types.iter().any(|x| x.name == name) {
            c.issue_types.push(NameCandidate {
                known: dict.has_issue_type(&name),
                name,
            });
        }
    }
    for k in v
        .get("doc_kinds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(name) = str_or_null(k, "name") else {
            continue;
        };
        c.doc_kinds.push(KindCandidate {
            known: dict.doc_kinds.iter().any(|d| d.name == name),
            ext: strs(k.get("ext")),
            name,
        });
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cur() -> Curator {
        Curator::new(ContextDict::starter(), CurateCfg::default(), true).unwrap()
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn dates_merge_within_fourteen_days_chained() {
        let c = cur();
        let others = [d("2026-10-01"), d("2026-10-10"), d("2026-11-20")];
        assert_eq!(
            c.group_date(d("2026-10-20"), &others),
            d("2026-10-01"),
            "chain 1→10→20"
        );
        assert_eq!(c.group_date(d("2026-11-25"), &others), d("2026-11-20"));
        assert_eq!(
            c.group_date(d("2026-12-31"), &others),
            d("2026-12-31"),
            "too far"
        );
        assert_eq!(
            c.group_date(d("2026-09-25"), &others),
            d("2026-09-25"),
            "earliest itself"
        );
    }

    #[test]
    fn answers_are_validated() {
        let v = serde_json::json!({"files": [
            {"id": 1, "entity": "가람/정보", "issue": "과금이슈", "date": "2026-10-20T09:00", "doc_kind": "로그", "topic": null, "confidence": 1.7, "reason": "r"},
            {"id": 2, "entity": null, "issue": "", "date": "bad", "doc_kind": "", "topic": null, "confidence": 0.5, "reason": ""},
            {"id": 9, "entity": "x", "issue": "y", "date": null, "doc_kind": "z", "topic": null, "confidence": 1, "reason": ""}
        ]});
        let m = parse_answer(&v, 2);
        assert_eq!(m.len(), 2, "id 9 is not in the batch");
        let a = &m[&1];
        assert_eq!(a.entity.as_deref(), Some("가람 정보"));
        assert_eq!(a.date, Some(d("2026-10-20")));
        assert_eq!(a.confidence, 1.0);
        let b = &m[&2];
        assert_eq!(
            (b.issue.as_str(), b.doc_kind.as_str()),
            ("기타 이슈", "기타 자료")
        );
        assert_eq!(b.topic.as_deref(), Some("미분류"));
        assert_eq!(b.date, None);
    }
}
