//! Configuration model (SPEC §7). Loaded from `config.toml`; every field
//! has a default so an empty file is valid.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("failed to edit config file {path}: {source}")]
    Edit {
        path: PathBuf,
        #[source]
        source: Box<toml_edit::TomlError>,
    },
    #[error("failed to write config file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The parts of `config.toml` the desktop settings screen edits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsPatch {
    pub watches: Vec<WatchRoot>,
    /// `None` = default DB location (the key is removed).
    pub store_path: Option<PathBuf>,
    pub quiet_mode: bool,
    pub hotkey: String,
    /// Start (tray only) when the user logs in.
    pub start_on_login: bool,
}

/// Apply `patch` to the config file at `path` (created if missing), keeping
/// every other key and all comments as they are (`toml_edit`).
/// Only the settings file is written; user files are never touched here.
pub fn write_settings(path: &Path, patch: &SettingsPatch) -> Result<(), ConfigError> {
    use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let mut doc: DocumentMut = text.parse().map_err(|e| ConfigError::Edit {
        path: path.to_path_buf(),
        source: Box::new(e),
    })?;

    let general = doc
        .entry("general")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .expect("[general] is a table");
    general.insert("quiet_mode", value(patch.quiet_mode));
    general.insert("hotkey", value(patch.hotkey.clone()));
    general.insert("start_on_login", value(patch.start_on_login));

    let mut watches = ArrayOfTables::new();
    for w in &patch.watches {
        let mut t = Table::new();
        t.insert("path", value(w.path.to_string_lossy().into_owned()));
        t.insert(
            "kind",
            value(match w.kind {
                RootKind::Local => "local",
                RootKind::Network => "network",
            }),
        );
        t.insert("recursive", value(w.recursive));
        t.insert("index_content", value(w.index_content));
        if let Some(s) = w.poll_interval_s {
            t.insert("poll_interval_s", value(s as i64));
        }
        if let Some(win) = &w.content_index_window {
            t.insert("content_index_window", value(win.clone()));
        }
        if w.poll_only {
            t.insert("poll_only", value(true));
        }
        watches.push(t);
    }
    doc.insert("watch", Item::ArrayOfTables(watches));

    match &patch.store_path {
        Some(p) => {
            let store = doc
                .entry("store")
                .or_insert(Item::Table(Table::new()))
                .as_table_like_mut()
                .expect("[store] is a table");
            store.insert("path", value(p.to_string_lossy().into_owned()));
        }
        None => {
            if let Some(store) = doc.get_mut("store").and_then(Item::as_table_like_mut) {
                store.remove("path");
            }
        }
    }

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, doc.to_string()).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// The `[ai]` fields the desktop settings screen edits: global switches
/// plus the classify model of the active profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiPatch {
    pub enabled: bool,
    pub active_profile: String,
    pub auto_threshold: f64,
    pub send_content: bool,
    pub classify: Option<ProviderConfig>,
    /// Embedding model of the active profile (semantic search); `None`
    /// removes it (search stays FTS-only).
    #[serde(default)]
    pub embed: Option<ProviderConfig>,
}

/// Apply `patch` to `[ai]` / `[ai.profiles.<active>]` of the config at
/// `path`, keeping everything else (other profiles, comments) as is.
pub fn write_ai_settings(path: &Path, patch: &AiPatch) -> Result<(), ConfigError> {
    use toml_edit::{value, DocumentMut, InlineTable, Item, Table};

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let mut doc: DocumentMut = text.parse().map_err(|e| ConfigError::Edit {
        path: path.to_path_buf(),
        source: Box::new(e),
    })?;
    let ai = doc
        .entry("ai")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("[ai] is a table");
    ai.insert("enabled", value(patch.enabled));
    ai.insert("active_profile", value(patch.active_profile.clone()));
    ai.insert("auto_threshold", value(patch.auto_threshold));
    ai.insert("send_content", value(patch.send_content));
    let profiles = ai
        .entry("profiles")
        .or_insert({
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_mut()
        .expect("[ai.profiles] is a table");
    let profile = profiles
        .entry(&patch.active_profile)
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .expect("profile is a table");
    for (key, pc) in [("classify", &patch.classify), ("embed", &patch.embed)] {
        match pc {
            Some(pc) => {
                let mut t = InlineTable::new();
                t.insert("provider", pc.provider.as_str().into());
                t.insert("model", pc.model.as_str().into());
                if let Some(k) = pc.key_ref.as_deref().filter(|k| !k.is_empty()) {
                    t.insert("key_ref", k.into());
                }
                if let Some(u) = pc.base_url.as_deref().filter(|u| !u.is_empty()) {
                    t.insert("base_url", u.into());
                }
                if let Some(e) = pc.effort.as_deref().filter(|e| !e.is_empty()) {
                    t.insert("effort", e.into());
                }
                profile.insert(key, value(t));
            }
            None => {
                profile.remove(key);
            }
        }
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, doc.to_string()).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: General,
    #[serde(rename = "watch")]
    pub watches: Vec<WatchRoot>,
    #[serde(rename = "index_only")]
    pub index_only: Vec<IndexRoot>,
    pub classify: Classify,
    pub mover: Mover,
    pub index: Index,
    pub store: Store,
    pub ai: Ai,
    pub report: ReportCfg,
    pub search: SearchCfg,
}

/// `[search]` (SPEC §5.6): semantic search on top of FTS.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchCfg {
    /// Use the active profile's `embed` model (when there is one) to rank
    /// by meaning as well. Off, or without a model: FTS only.
    pub semantic: bool,
    /// Budget for embedding the query; on timeout the search is FTS-only.
    pub query_timeout_ms: u64,
    /// Vector candidates merged with the FTS results.
    pub vector_k: usize,
    /// Cosine similarity below which a vector-only hit is dropped.
    pub min_similarity: f32,
    /// Texts per embedding request while indexing.
    pub embed_batch: usize,
    /// Characters of body text embedded per file (after the name). With
    /// `send_content = false` only the name is embedded.
    pub embed_chars: usize,
}

impl Default for SearchCfg {
    fn default() -> Self {
        Self {
            semantic: true,
            query_timeout_ms: 1500,
            vector_k: 50,
            min_similarity: 0.3,
            embed_batch: 32,
            embed_chars: 1000,
        }
    }
}

/// `[report]` (SPEC §5.7).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportCfg {
    /// "건드리지 말 폴더": restructure plans never move anything in or
    /// into these (in addition to folders given per request).
    pub protected: Vec<PathBuf>,
    pub stale_days: u64,
    pub top_n: usize,
    /// Upper bound of moves accepted from one AI restructure answer.
    pub max_moves: usize,
    /// Scan only this many folder levels below the root (0 = its direct
    /// children). Unset: everything — slow on a huge desktop or share;
    /// `tidy report --scan-depth N` overrides it per run.
    pub scan_depth: Option<usize>,
}

impl Default for ReportCfg {
    fn default() -> Self {
        Self {
            protected: Vec::new(),
            stale_days: 90,
            top_n: 10,
            max_moves: 500,
            scan_depth: None,
        }
    }
}

/// `[ai]` (SPEC §5.8, §7). AI is optional: with `enabled = false`, no
/// profile, or no key, everything runs in rules + search mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Ai {
    pub enabled: bool,
    pub active_profile: String,
    /// AI suggestions at or above this confidence may move files on their
    /// own — only into folders of rules marked `ai_auto = true`.
    pub auto_threshold: f64,
    /// Send the first `content_chars` characters of extracted text. A
    /// profile's own `send_content` overrides this (e.g. a work profile).
    pub send_content: bool,
    pub content_chars: usize,
    /// Per-request timeout, retries included (SPEC: default 20 s).
    pub timeout_secs: u64,
    /// Retries for 429 / 5xx / network errors.
    pub max_retries: u32,
    /// Restructure proposals are long answers: their own timeout.
    pub restructure_timeout_secs: u64,
    /// Depth of the candidate folder tree sent to the model.
    pub tree_depth: usize,
    /// How many past user decisions to include as examples.
    pub history_k: usize,
    /// Consecutive failed calls after which the AI counts as unavailable
    /// (status bar; files skip the AI until a probe succeeds).
    pub unavailable_after: u32,
    /// Extra top folders for the candidate tree (rule destinations and
    /// `[[index_only]]` folders are always included).
    pub candidate_roots: Vec<PathBuf>,
    pub profiles: std::collections::BTreeMap<String, AiProfile>,
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            enabled: false,
            active_profile: "default".into(),
            auto_threshold: 0.9,
            send_content: true,
            content_chars: 1500,
            timeout_secs: 20,
            max_retries: 2,
            restructure_timeout_secs: 180,
            tree_depth: 2,
            history_k: 5,
            unavailable_after: 3,
            candidate_roots: Vec::new(),
            profiles: Default::default(),
        }
    }
}

impl Ai {
    pub fn active(&self) -> Option<&AiProfile> {
        self.profiles.get(&self.active_profile)
    }

    /// Whether file text may leave the machine for the active profile.
    pub fn send_content(&self) -> bool {
        self.active()
            .and_then(|p| p.send_content)
            .unwrap_or(self.send_content)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AiProfile {
    /// Model used to classify files.
    pub classify: Option<ProviderConfig>,
    /// Embedding model (semantic search, M7): openai(-compatible) or
    /// ollama; Anthropic has no embeddings API.
    pub embed: Option<ProviderConfig>,
    pub send_content: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    /// Any OpenAI-compatible `/chat/completions` endpoint.
    #[serde(alias = "openai-compatible", alias = "openai_compatible")]
    Openai,
    Ollama,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Openai => "openai",
            ProviderKind::Ollama => "ollama",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider: ProviderKind,
    pub model: String,
    /// Name of the API key in the OS keychain ("service/user", e.g.
    /// "tidy/anthropic"). Never the key itself.
    #[serde(default)]
    pub key_ref: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Anthropic `output_config.effort` (e.g. "low"); omitted when unset.
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub language: String,
    pub hotkey: String,
    pub quiet_mode: bool,
    pub start_on_login: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "ko".into(),
            hotkey: "CmdOrCtrl+Shift+Space".into(),
            quiet_mode: false,
            start_on_login: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RootKind {
    #[default]
    Local,
    Network,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "RawWatchRoot")]
pub struct WatchRoot {
    pub path: PathBuf,
    pub kind: RootKind,
    pub recursive: bool,
    /// Extract body text for search. Default: on for local folders, off
    /// for network folders (SPEC §5.9: NAS = metadata only).
    pub index_content: bool,
    /// Polling period. Network folders poll every 120 s by default; local
    /// folders only poll when this is set.
    pub poll_interval_s: Option<u64>,
    /// "HH:MM-HH:MM" (may wrap midnight): body text of this folder is only
    /// extracted by background indexing inside the window. `tidy index`
    /// (a manual run) ignores it.
    pub content_index_window: Option<String>,
    /// Network folders: skip OS change events and rely on polling only
    /// (some SMB/NFS servers send none, or bogus ones).
    pub poll_only: bool,
}

/// `[[watch]]` as written in the file: `index_content` defaults by `kind`.
#[derive(Deserialize)]
#[serde(default)]
struct RawWatchRoot {
    path: PathBuf,
    kind: RootKind,
    recursive: bool,
    index_content: Option<bool>,
    poll_interval_s: Option<u64>,
    content_index_window: Option<String>,
    poll_only: bool,
}

impl Default for RawWatchRoot {
    fn default() -> Self {
        let d = WatchRoot::default();
        Self {
            path: d.path,
            kind: d.kind,
            recursive: d.recursive,
            index_content: None,
            poll_interval_s: None,
            content_index_window: None,
            poll_only: false,
        }
    }
}

impl From<RawWatchRoot> for WatchRoot {
    fn from(r: RawWatchRoot) -> Self {
        Self {
            index_content: r.index_content.unwrap_or(r.kind == RootKind::Local),
            path: r.path,
            kind: r.kind,
            recursive: r.recursive,
            poll_interval_s: r.poll_interval_s,
            content_index_window: r.content_index_window,
            poll_only: r.poll_only,
        }
    }
}

impl Default for WatchRoot {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            kind: RootKind::Local,
            recursive: false,
            index_content: true,
            poll_interval_s: None,
            content_index_window: None,
            poll_only: false,
        }
    }
}

/// Default polling period of network folders (SPEC §5.1).
pub const DEFAULT_POLL_SECS: u64 = 120;
/// Shortest accepted polling period (a NAS should not be hammered).
pub const MIN_POLL_SECS: u64 = 5;

impl WatchRoot {
    /// How often this folder is polled, if at all: network folders always
    /// (default 120 s), local folders only with `poll_interval_s`.
    pub fn poll_interval(&self) -> Option<Duration> {
        let secs = match (self.kind, self.poll_interval_s) {
            (RootKind::Network, s) => s.unwrap_or(DEFAULT_POLL_SECS),
            (RootKind::Local, Some(s)) => s,
            (RootKind::Local, None) => return None,
        };
        Some(Duration::from_secs(secs.max(MIN_POLL_SECS)))
    }

    /// Use OS change events for this folder.
    pub fn uses_events(&self) -> bool {
        !(self.kind == RootKind::Network && self.poll_only)
    }

    /// Whether background indexing extracts body text at local time `now`
    /// (honours `content_index_window`; an invalid window means "always").
    pub fn content_now(&self, now: chrono::NaiveTime) -> bool {
        self.index_content
            && match self.content_index_window.as_deref().map(TimeWindow::parse) {
                Some(Ok(w)) => w.contains(now),
                _ => true,
            }
    }
}

/// A daily time window "HH:MM-HH:MM"; the end is exclusive and the window
/// may wrap midnight ("22:00-05:00").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    pub start: chrono::NaiveTime,
    pub end: chrono::NaiveTime,
}

impl TimeWindow {
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || format!("시간대 형식은 \"HH:MM-HH:MM\"입니다: {s:?}");
        let (a, b) = s.split_once('-').ok_or_else(bad)?;
        let t = |x: &str| chrono::NaiveTime::parse_from_str(x.trim(), "%H:%M").map_err(|_| bad());
        Ok(Self {
            start: t(a)?,
            end: t(b)?,
        })
    }

    pub fn contains(&self, t: chrono::NaiveTime) -> bool {
        if self.start <= self.end {
            self.start <= t && t < self.end
        } else {
            t >= self.start || t < self.end
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexRoot {
    pub path: PathBuf,
    pub recursive: bool,
}

impl Default for IndexRoot {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            recursive: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Classify {
    /// Rules file (SPEC §5.2). Relative paths are resolved against the
    /// directory containing `config.toml`.
    pub rules: PathBuf,
    pub stabilize_secs: u64,
    pub learn_threshold: u32,
    pub ignore_globs: Vec<String>,
}

impl Default for Classify {
    fn default() -> Self {
        Self {
            rules: PathBuf::from("rules.toml"),
            stabilize_secs: 3,
            learn_threshold: 3,
            ignore_globs: vec![
                "*.tmp".into(),
                "*.crdownload".into(),
                "*.part".into(),
                "~$*".into(),
                ".DS_Store".into(),
                "Thumbs.db".into(),
                "desktop.ini".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DuplicatePolicy {
    #[default]
    Skip,
    Rename,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Mover {
    pub undo_retention_days: u32,
    pub duplicate_policy: DuplicatePolicy,
    pub nas_concurrency: u8,
}

impl Default for Mover {
    fn default() -> Self {
        Self {
            undo_retention_days: 30,
            duplicate_policy: DuplicatePolicy::Skip,
            nas_concurrency: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    /// Files larger than this get metadata only (no hash, no text).
    pub max_content_size_mb: u64,
    pub max_hash_size_mb_network: u64,
    pub cpu_limit_percent: u8,
    /// Extracted text is truncated to this many characters before storing.
    pub max_text_chars: usize,
}

impl Default for Index {
    fn default() -> Self {
        Self {
            max_content_size_mb: 200,
            max_hash_size_mb_network: 20,
            cpu_limit_percent: 30,
            max_text_chars: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Store {
    /// SQLite database file. `None` = `<data_local_dir>/tidy/tidy.db`.
    /// Relative paths are resolved against the directory of `config.toml`.
    pub path: Option<PathBuf>,
}

impl Store {
    pub fn default_db_path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("tidy")
            .join("tidy.db")
    }

    pub fn db_path(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(Self::default_db_path)
    }
}

impl Config {
    /// Load from a TOML file. Missing keys fall back to defaults. Relative
    /// `classify.rules` / `store.path` are resolved against the config
    /// file's directory so `tidy watch --config x/config.toml` works from
    /// any working directory.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut cfg = Self::from_toml(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        let base = path.parent().unwrap_or_else(|| Path::new(""));
        cfg.resolve_relative(base);
        Ok(cfg)
    }

    /// Resolve relative file references against `base`.
    pub fn resolve_relative(&mut self, base: &Path) {
        if self.classify.rules.is_relative() {
            self.classify.rules = base.join(&self.classify.rules);
        }
        if let Some(p) = &self.store.path {
            if p.is_relative() {
                self.store.path = Some(base.join(p));
            }
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Config = toml::from_str(text)?;
        for w in &mut cfg.watches {
            w.path = expand_home(&w.path);
        }
        for r in &mut cfg.index_only {
            r.path = expand_home(&r.path);
        }
        cfg.classify.rules = expand_home(&cfg.classify.rules);
        for r in &mut cfg.ai.candidate_roots {
            *r = expand_home(r);
        }
        for r in &mut cfg.report.protected {
            *r = expand_home(r);
        }
        if let Some(p) = &cfg.store.path {
            cfg.store.path = Some(expand_home(p));
        }
        Ok(cfg)
    }

    /// Problems worth telling the user at startup (SPEC §5.9, §6): the DB
    /// on a network share (SQLite needs local file locks), network-looking
    /// folders marked `local`, invalid content windows. Never fatal.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let db = self.store.db_path();
        // Lexical only: resolving a share that is offline can block for
        // a long time, and this runs at startup.
        let under_network_root = self
            .watches
            .iter()
            .any(|w| w.kind == RootKind::Network && db.starts_with(&w.path));
        if under_network_root || crate::paths::looks_like_network(&db) {
            out.push(format!(
                "DB 파일이 네트워크 위치에 있습니다: {} — SQLite는 로컬 디스크에서만 안정적으로 동작합니다. [store] path를 로컬 경로로 바꾸세요.",
                db.display()
            ));
        }
        for w in &self.watches {
            if w.kind == RootKind::Local && crate::paths::looks_like_network(&w.path) {
                out.push(format!(
                    "감시 폴더가 네트워크 위치로 보입니다: {} — kind = \"network\"로 두면 폴링·복사-검증 이동을 씁니다.",
                    w.path.display()
                ));
            }
            if let Some(Err(e)) = w.content_index_window.as_deref().map(TimeWindow::parse) {
                out.push(format!(
                    "{}: content_index_window {e} (무시하고 항상 본문 인덱싱)",
                    w.path.display()
                ));
            }
        }
        out
    }

    /// Default location: `<config_dir>/tidy/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("tidy").join("config.toml"))
    }
}

/// Expand a leading `~` to the user's home directory. Paths are kept as
/// `PathBuf` end to end (CLAUDE.md: no string concatenation for paths).
pub fn expand_home(p: &Path) -> PathBuf {
    let mut comps = p.components();
    match comps.next() {
        Some(std::path::Component::Normal(s)) if s == "~" => {
            if let Some(home) = dirs::home_dir() {
                return home.join(comps.as_path());
            }
            p.to_path_buf()
        }
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_gives_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.general.language, "ko");
        assert_eq!(c.mover.undo_retention_days, 30);
        assert!(c.watches.is_empty());
    }

    #[test]
    fn parses_spec_example() {
        let text = r#"
[general]
language = "ko"

[[watch]]
path = "~/Desktop"
kind = "local"

[[watch]]
path = "Z:/공유/업무"
kind = "network"
recursive = true
poll_interval_s = 120
index_content = false

[[index_only]]
path = "~/Documents"

[mover]
duplicate_policy = "rename"
"#;
        let c = Config::from_toml(text).unwrap();
        assert_eq!(c.watches.len(), 2);
        assert_eq!(c.watches[1].kind, RootKind::Network);
        assert_eq!(c.watches[1].poll_interval_s, Some(120));
        assert!(!c.watches[1].index_content);
        assert_eq!(c.index_only.len(), 1);
        assert_eq!(c.mover.duplicate_policy, DuplicatePolicy::Rename);
        // Korean path survives round trip untouched.
        assert!(c.watches[1].path.to_string_lossy().contains("공유"));
    }

    #[test]
    fn network_roots_default_to_metadata_only_and_poll() {
        let c = Config::from_toml(
            r#"
[[watch]]
path = "Z:/공유/업무"
kind = "network"

[[watch]]
path = "//nas/사진"
kind = "network"
index_content = true
poll_interval_s = 1
poll_only = true
content_index_window = "22:00-05:00"

[[watch]]
path = "~/Desktop"
"#,
        )
        .unwrap();
        let (nas, photos, desk) = (&c.watches[0], &c.watches[1], &c.watches[2]);
        assert!(!nas.index_content);
        assert_eq!(nas.poll_interval(), Some(Duration::from_secs(120)));
        assert!(nas.uses_events());
        assert!(photos.index_content && !photos.uses_events());
        assert_eq!(
            photos.poll_interval(),
            Some(Duration::from_secs(MIN_POLL_SECS))
        );
        let t = |h, m| chrono::NaiveTime::from_hms_opt(h, m, 0).unwrap();
        assert!(photos.content_now(t(23, 0)) && photos.content_now(t(4, 59)));
        assert!(!photos.content_now(t(5, 0)) && !photos.content_now(t(12, 0)));
        assert!(!nas.content_now(t(3, 0)));
        assert!(desk.index_content && desk.poll_interval().is_none());
    }

    #[test]
    fn time_window_parse() {
        let w = TimeWindow::parse("02:00-05:00").unwrap();
        let t = |h| chrono::NaiveTime::from_hms_opt(h, 0, 0).unwrap();
        assert!(w.contains(t(2)) && w.contains(t(4)) && !w.contains(t(5)) && !w.contains(t(1)));
        assert!(TimeWindow::parse("2시-5시").is_err());
        assert!(TimeWindow::parse("02:00").is_err());
    }

    #[test]
    fn warns_about_db_under_network_root() {
        let dir = tempfile::tempdir().unwrap();
        let nas = dir.path().join("나스 공유");
        std::fs::create_dir(&nas).unwrap();
        let mut c = Config::default();
        c.watches.push(WatchRoot {
            path: nas.clone(),
            kind: RootKind::Network,
            ..Default::default()
        });
        assert!(c.warnings().is_empty());
        c.store.path = Some(nas.join("tidy.db"));
        let w = c.warnings();
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("로컬"));
        c.store.path = None;
        c.watches.push(WatchRoot {
            path: PathBuf::from(r"\\nas\공유"),
            content_index_window: Some("밤".into()),
            ..Default::default()
        });
        assert_eq!(c.warnings().len(), 2);
    }

    #[test]
    fn relative_rules_and_db_resolve_against_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("설정 폴더");
        std::fs::create_dir(&sub).unwrap();
        let cfg_path = sub.join("config.toml");
        std::fs::write(&cfg_path, "[store]\npath = \"data/tidy.db\"\n").unwrap();
        let c = Config::load(&cfg_path).unwrap();
        assert_eq!(c.classify.rules, sub.join("rules.toml"));
        assert_eq!(c.store.db_path(), sub.join("data").join("tidy.db"));
    }

    #[test]
    fn write_settings_keeps_comments_and_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("설정 폴더").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# 내 설정\n[general]\nlanguage = \"ko\" # 언어\n\n[[watch]]\npath = \"~/Desktop\"\n\n\
             [mover]\nduplicate_policy = \"rename\"\n\n[store]\npath = \"old.db\"\n",
        )
        .unwrap();
        let patch = SettingsPatch {
            watches: vec![
                WatchRoot {
                    path: PathBuf::from(r"C:\Users\홍 길동\OneDrive\바탕 화면"),
                    ..Default::default()
                },
                WatchRoot {
                    path: PathBuf::from(r"\\nas\공유 폴더"),
                    kind: RootKind::Network,
                    recursive: true,
                    poll_interval_s: Some(120),
                    poll_only: true,
                    content_index_window: Some("02:00-05:00".into()),
                    ..Default::default()
                },
            ],
            store_path: None,
            quiet_mode: true,
            hotkey: "CmdOrCtrl+Alt+K".into(),
            start_on_login: false,
        };
        write_settings(&path, &patch).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# 내 설정") && text.contains("# 언어"));
        let c = Config::load(&path).unwrap();
        assert_eq!(c.watches.len(), 2);
        assert_eq!(c.watches[0].path, patch.watches[0].path);
        assert_eq!(c.watches[1].kind, RootKind::Network);
        assert_eq!(c.watches[1].poll_interval_s, Some(120));
        assert!(c.watches[1].poll_only && c.watches[1].index_content);
        assert_eq!(
            c.watches[1].content_index_window.as_deref(),
            Some("02:00-05:00")
        );
        assert_eq!(c.mover.duplicate_policy, DuplicatePolicy::Rename);
        assert!(c.general.quiet_mode);
        assert_eq!(c.general.hotkey, "CmdOrCtrl+Alt+K");
        assert!(!c.general.start_on_login);
        assert_eq!(c.store.path, None);

        // Missing file: created from scratch.
        let fresh = dir.path().join("새 설정").join("config.toml");
        let patch = SettingsPatch {
            store_path: Some(PathBuf::from("tidy.db")),
            ..patch
        };
        write_settings(&fresh, &patch).unwrap();
        let c = Config::load(&fresh).unwrap();
        assert_eq!(c.store.db_path(), fresh.parent().unwrap().join("tidy.db"));
    }

    #[test]
    fn parses_spec_ai_section() {
        let c = Config::from_toml(
            r#"
[ai]
enabled = true
active_profile = "home"
auto_threshold = 0.9
send_content = true
content_chars = 1500

[ai.profiles.home]
classify = { provider = "anthropic", model = "claude-haiku-5-5", key_ref = "tidy/anthropic" }
embed    = { provider = "ollama", model = "nomic-embed-text", base_url = "http://localhost:11434" }

[ai.profiles.work]
classify = { provider = "openai-compatible", model = "qwen3:8b", base_url = "http://localhost:8000/v1" }
send_content = false
"#,
        )
        .unwrap();
        assert!(c.ai.enabled);
        let home = c.ai.active().unwrap();
        let cl = home.classify.as_ref().unwrap();
        assert_eq!(cl.provider, ProviderKind::Anthropic);
        assert_eq!(cl.key_ref.as_deref(), Some("tidy/anthropic"));
        assert!(c.ai.send_content());
        assert_eq!(
            c.ai.profiles["work"].classify.as_ref().unwrap().provider,
            ProviderKind::Openai
        );
        let mut work = c.ai.clone();
        work.active_profile = "work".into();
        assert!(!work.send_content());
        // AI is off unless configured.
        assert!(!Config::default().ai.enabled);
    }

    #[test]
    fn write_ai_settings_round_trip_keeps_other_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# 설정\n[ai.profiles.work]\n# 회사는 로컬만\nclassify = { provider = \"ollama\", model = \"qwen3:8b\" }\nsend_content = false\n",
        )
        .unwrap();
        write_ai_settings(
            &path,
            &AiPatch {
                enabled: true,
                active_profile: "home".into(),
                auto_threshold: 0.85,
                send_content: true,
                classify: Some(ProviderConfig {
                    provider: ProviderKind::Anthropic,
                    model: "claude-haiku-5-5".into(),
                    key_ref: Some("tidy/anthropic".into()),
                    base_url: None,
                    effort: Some("low".into()),
                }),
                embed: Some(ProviderConfig {
                    provider: ProviderKind::Ollama,
                    model: "nomic-embed-text".into(),
                    key_ref: None,
                    base_url: Some("http://localhost:11434".into()),
                    effort: None,
                }),
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# 회사는 로컬만"));
        assert!(!text.contains("sk-"));
        let c = Config::load(&path).unwrap();
        assert!(c.ai.enabled);
        assert_eq!(c.ai.auto_threshold, 0.85);
        let home = c.ai.active().unwrap().classify.as_ref().unwrap();
        assert_eq!(home.model, "claude-haiku-5-5");
        assert_eq!(home.effort.as_deref(), Some("low"));
        let embed = c.ai.active().unwrap().embed.as_ref().unwrap();
        assert_eq!(embed.model, "nomic-embed-text");
        assert_eq!(c.ai.profiles["work"].send_content, Some(false));
    }

    #[test]
    fn tilde_is_expanded_only_when_leading() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home(Path::new("~/x")), home.join("x"));
        assert_eq!(expand_home(Path::new("a/~/x")), PathBuf::from("a/~/x"));
    }
}
