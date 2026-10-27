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
    /// `Some(true)` when this save completes setup (wizard finish, or an
    /// explicit save in the settings tab); `None` leaves the key alone.
    #[serde(default)]
    pub onboarded: Option<bool>,
    /// (M11) `[general] check_updates`.
    #[serde(default = "yes")]
    pub check_updates: bool,
    /// (0.8) `[general] keep_shortcuts`: shortcuts stay where they are.
    #[serde(default = "yes")]
    pub keep_shortcuts: bool,
    /// (M14) `[mover] duplicate_policy`; `None` leaves the key alone.
    #[serde(default)]
    pub duplicate_policy: Option<DuplicatePolicy>,
}

fn yes() -> bool {
    true
}

/// Apply `patch` to the config file at `path` (created if missing), keeping
/// every other key and all comments as they are (`toml_edit`).
/// Only the settings file is written; user files are never touched here.
pub fn write_settings(path: &Path, patch: &SettingsPatch) -> Result<(), ConfigError> {
    use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

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
    general.insert("check_updates", value(patch.check_updates));
    general.insert("keep_shortcuts", value(patch.keep_shortcuts));
    if let Some(o) = patch.onboarded {
        general.insert("onboarded", value(o));
    }
    if let Some(p) = patch.duplicate_policy {
        let mover = doc
            .entry("mover")
            .or_insert(Item::Table(Table::new()))
            .as_table_like_mut()
            .expect("[mover] is a table");
        mover.insert(
            "duplicate_policy",
            value(match p {
                DuplicatePolicy::Skip => "skip",
                DuplicatePolicy::Rename => "rename",
                DuplicatePolicy::Replace => "replace",
            }),
        );
    }

    let mut watches = ArrayOfTables::new();
    for w in &patch.watches {
        let mut t = Table::new();
        t.insert("path", value(w.path.to_string_lossy().into_owned()));
        if let Some(a) = w.alias.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            t.insert("alias", value(a));
        }
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
        if w.mode != OrganizeMode::InPlace {
            t.insert("mode", value(w.mode.as_str()));
        }
        if let Some(r) = w
            .organize_root
            .as_ref()
            .filter(|r| !r.as_os_str().is_empty())
        {
            t.insert("organize_root", value(r.to_string_lossy().into_owned()));
        }
        if !w.exclude.is_empty() {
            let mut ex = Array::new();
            for e in &w.exclude {
                ex.push(e.to_string_lossy().into_owned());
            }
            t.insert("exclude", value(ex));
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
    /// Remembered settings per classify provider (`[ai.profiles.<p>.saved]`).
    /// The current `classify` is added to it on write.
    #[serde(default)]
    pub saved: std::collections::BTreeMap<String, ProviderConfig>,
    /// Remembered settings per embedding provider (`embed_saved`).
    #[serde(default)]
    pub embed_saved: std::collections::BTreeMap<String, ProviderConfig>,
}

/// One provider entry as a TOML inline table (empty fields left out).
fn provider_inline(pc: &ProviderConfig) -> toml_edit::InlineTable {
    let mut t = toml_edit::InlineTable::new();
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
    t
}

/// Apply `patch` to `[ai]` / `[ai.profiles.<active>]` of the config at
/// `path`, keeping everything else (other profiles, comments) as is.
pub fn write_ai_settings(path: &Path, patch: &AiPatch) -> Result<(), ConfigError> {
    use toml_edit::{value, DocumentMut, Item, Table};

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
                profile.insert(key, value(provider_inline(pc)));
            }
            None => {
                profile.remove(key);
            }
        }
    }
    for (key, saved, current) in [
        ("saved", &patch.saved, &patch.classify),
        ("embed_saved", &patch.embed_saved, &patch.embed),
    ] {
        let mut all = saved.clone();
        if let Some(pc) = current {
            all.insert(pc.provider.as_str().to_string(), pc.clone());
        }
        if all.is_empty() {
            profile.remove(key);
            continue;
        }
        let mut t = Table::new();
        for (name, pc) in &all {
            // The map key is the provider; a mismatching entry is skipped.
            if pc.provider.as_str() == name {
                t.insert(name, value(provider_inline(pc)));
            }
        }
        profile.insert(key, Item::Table(t));
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
    /// (M9) Personal information detection + encrypted vault.
    pub security: Security,
    /// (M10) Work-context curation (customer / issue folders, AI).
    pub curate: CurateCfg,
}

/// `[curate]` (SPEC §14.3, M10): group files by customer and issue with
/// the AI. Needs the classify model of `[ai]`; without it nothing here runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CurateCfg {
    /// Also place NEW files in the watch folders by work context (files no
    /// rule takes; with `prefer`, before the rules).
    pub enabled: bool,
    /// "맥락 정리 우선": try the work context before the rules.
    pub prefer: bool,
    /// The context dictionary (customers, issue types, kinds of material).
    /// Relative to the config file's folder.
    pub context: PathBuf,
    /// Folder layout for files with a customer.
    pub template: String,
    /// Folder layout for files without one.
    pub other_template: String,
    /// Below this confidence a file is left as "미분류".
    pub min_confidence: f64,
    /// Files per AI request.
    pub batch_size: usize,
    /// Same customer + issue within this many days → one folder, earliest date.
    pub merge_days: u32,
    /// Characters of body text per file (only with `ai.send_content`).
    pub excerpt_chars: usize,
    /// (M12) A sub-folder becomes a folder item only after nothing inside
    /// changed for this long (not while it is being copied / unpacked).
    pub folder_settle_secs: u64,
}

impl Default for CurateCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            prefer: false,
            context: PathBuf::from("context.toml"),
            template: "{entity}/{yyyymmdd}_{issue}/{doc_kind}".into(),
            other_template: "기타/{topic}".into(),
            min_confidence: 0.6,
            batch_size: 30,
            merge_days: 14,
            excerpt_chars: 400,
            folder_settle_secs: 60,
        }
    }
}

/// The `[curate]` fields the settings screen edits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratePatch {
    pub enabled: bool,
    pub prefer: bool,
    pub template: String,
    pub other_template: String,
    pub min_confidence: f64,
}

/// Apply `patch` to `[curate]` of the config at `path`, keeping the rest.
pub fn write_curate_settings(path: &Path, patch: &CuratePatch) -> Result<(), ConfigError> {
    use toml_edit::{value, DocumentMut, Item, Table};
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
    let t = doc
        .entry("curate")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("[curate] is a table");
    t.insert("enabled", value(patch.enabled));
    t.insert("prefer", value(patch.prefer));
    t.insert("template", value(patch.template.trim()));
    t.insert("other_template", value(patch.other_template.trim()));
    t.insert(
        "min_confidence",
        value(patch.min_confidence.clamp(0.0, 1.0)),
    );
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
    /// Settings last used with each classify provider, keyed by
    /// `ProviderKind::as_str()`, so switching back to a provider in the
    /// settings screen restores its model / key name / address.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub saved: std::collections::BTreeMap<String, ProviderConfig>,
    /// Same for the embedding model (semantic search).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub embed_saved: std::collections::BTreeMap<String, ProviderConfig>,
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
    /// First-run wizard finished (SPEC §14.1). Absent in configs from
    /// before M8: see [`Config::onboarded`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onboarded: Option<bool>,
    /// (M11) Look for a new release once a day in the background.
    pub check_updates: bool,
    /// Leave shortcut files (`.lnk`, `.url`, `.webloc`, …) where they are:
    /// a desktop shortcut is a launcher, not a document to file away.
    pub keep_shortcuts: bool,
}

/// File-name globs of shortcut / launcher files, by platform convention.
pub const SHORTCUT_GLOBS: &[&str] = &[
    "*.lnk",
    "*.url",
    "*.website",
    "*.appref-ms",
    "*.webloc",
    "*.desktop",
];

impl Default for General {
    fn default() -> Self {
        Self {
            language: "ko".into(),
            hotkey: "CmdOrCtrl+Shift+Space".into(),
            quiet_mode: false,
            start_on_login: true,
            onboarded: None,
            check_updates: true,
            keep_shortcuts: true,
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
    /// (M13, SPEC §14.8) Name shown instead of the path ("바탕화면",
    /// "PC백업"). `None` = default (OS name of the well-known folders, else
    /// the last folder name), worked out by the UI.
    pub alias: Option<String>,
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
    /// How files of this folder are organized (SPEC §14.1, M8).
    pub mode: OrganizeMode,
    /// `collect` mode: the folder the category folders are made under
    /// (`{root}` and relative rule destinations).
    pub organize_root: Option<PathBuf>,
    /// Sub-folders left alone entirely (SPEC §14.5): nothing inside them
    /// is classified, moved, proposed by a plan or counted by a preview.
    /// They are still indexed for search.
    pub exclude: Vec<PathBuf>,
}

/// Where a watch folder's files go (SPEC §14.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OrganizeMode {
    /// Sub-folders inside this folder; relative destinations stay inside.
    #[default]
    InPlace,
    /// Category folders under `organize_root`.
    Collect,
}

impl OrganizeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            OrganizeMode::InPlace => "in_place",
            OrganizeMode::Collect => "collect",
        }
    }
}

/// `[[watch]]` as written in the file: `index_content` defaults by `kind`.
#[derive(Deserialize)]
#[serde(default)]
struct RawWatchRoot {
    path: PathBuf,
    alias: Option<String>,
    kind: RootKind,
    recursive: bool,
    index_content: Option<bool>,
    poll_interval_s: Option<u64>,
    content_index_window: Option<String>,
    poll_only: bool,
    mode: OrganizeMode,
    organize_root: Option<PathBuf>,
    exclude: Vec<PathBuf>,
}

impl Default for RawWatchRoot {
    fn default() -> Self {
        let d = WatchRoot::default();
        Self {
            path: d.path,
            alias: None,
            kind: d.kind,
            recursive: d.recursive,
            index_content: None,
            poll_interval_s: None,
            content_index_window: None,
            poll_only: false,
            mode: OrganizeMode::InPlace,
            organize_root: None,
            exclude: Vec::new(),
        }
    }
}

impl From<RawWatchRoot> for WatchRoot {
    fn from(r: RawWatchRoot) -> Self {
        Self {
            index_content: r.index_content.unwrap_or(r.kind == RootKind::Local),
            path: r.path,
            alias: r
                .alias
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty()),
            kind: r.kind,
            recursive: r.recursive,
            poll_interval_s: r.poll_interval_s,
            content_index_window: r.content_index_window,
            poll_only: r.poll_only,
            mode: r.mode,
            organize_root: r.organize_root,
            exclude: r.exclude,
        }
    }
}

impl Default for WatchRoot {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            alias: None,
            kind: RootKind::Local,
            recursive: false,
            index_content: true,
            poll_interval_s: None,
            content_index_window: None,
            poll_only: false,
            mode: OrganizeMode::InPlace,
            organize_root: None,
            exclude: Vec::new(),
        }
    }
}

/// Default polling period of network folders (SPEC §5.1).
pub const DEFAULT_POLL_SECS: u64 = 120;
/// Shortest accepted polling period (a NAS should not be hammered).
pub const MIN_POLL_SECS: u64 = 5;

impl WatchRoot {
    /// Folder that relative rule destinations and `{root}` resolve against
    /// (SPEC §14.1): the watch folder itself (`in_place`), or
    /// `organize_root` (`collect`; the watch folder if it is missing).
    pub fn anchor(&self) -> &Path {
        match (self.mode, &self.organize_root) {
            (OrganizeMode::Collect, Some(r)) if !r.as_os_str().is_empty() => r,
            _ => &self.path,
        }
    }

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

/// `[security]` (SPEC §14.2, M9): personal-information detection and the
/// encrypted vault. Passwords are never here — only in the OS keychain
/// (`tidy/vault`, CLAUDE.md rule 5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Security {
    /// `rules`: only rules with `encrypt = "pii" | "always"` encrypt
    /// (mode B). `vault`: files judged personal go to `vault_dir` before
    /// any rule, encrypted (mode A); rule options still apply to the rest.
    pub mode: SecurityMode,
    /// Where mode A collects encrypted files (e.g. `D:\정리\보안`).
    pub vault_dir: Option<PathBuf>,
    /// `confirm` (default): every encryption waits in the confirm popup /
    /// queue. `auto`: encrypt and move without asking.
    pub approval: Approval,
    pub pii: PiiCfg,
    /// Opened vault files are decrypted to `<temp>/tidy-open` and the copy
    /// is removed after this many minutes.
    pub open_ttl_minutes: u64,
}

impl Default for Security {
    fn default() -> Self {
        Self {
            mode: SecurityMode::Rules,
            vault_dir: None,
            approval: Approval::Confirm,
            pii: PiiCfg::default(),
            open_ttl_minutes: 10,
        }
    }
}

impl Security {
    /// Mode A is in effect (needs a vault folder).
    pub fn collects(&self) -> Option<&Path> {
        match self.mode {
            SecurityMode::Vault => self
                .vault_dir
                .as_deref()
                .filter(|d| !d.as_os_str().is_empty()),
            SecurityMode::Rules => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    #[default]
    Rules,
    Vault,
}

impl SecurityMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityMode::Rules => "rules",
            SecurityMode::Vault => "vault",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Auto,
    #[default]
    Confirm,
}

impl Approval {
    pub fn as_str(self) -> &'static str {
        match self {
            Approval::Auto => "auto",
            Approval::Confirm => "confirm",
        }
    }
}

/// `[security.pii]`: what counts as personal information (offline regex).
/// A file is personal when any enabled item reaches its minimum count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PiiCfg {
    /// Detect at all (index + engine). Off = every file is "not judged".
    pub enabled: bool,
    pub rrn: bool,
    pub account: bool,
    pub name_phone: bool,
    pub name_email: bool,
    pub min_rrn: u32,
    pub min_account: u32,
    pub min_name_phone: u32,
    pub min_name_email: u32,
    /// Regexes; a found value (as written, or its digits only) matching
    /// one of them is not counted (test numbers, the company's own
    /// account, …).
    pub exclude: Vec<String>,
    /// Ask the AI (when on) to confirm a regex hit. Off by default; an AI
    /// failure keeps the regex verdict.
    pub ai_check: bool,
}

impl Default for PiiCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            rrn: true,
            account: true,
            name_phone: true,
            name_email: true,
            min_rrn: 1,
            min_account: 1,
            min_name_phone: 1,
            min_name_email: 1,
            exclude: vec!["^010-?0000-?0000$".into()],
            ai_check: false,
        }
    }
}

/// The `[security]` fields the desktop settings screen edits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPatch {
    pub mode: SecurityMode,
    pub vault_dir: Option<PathBuf>,
    pub approval: Approval,
    pub pii: PiiCfg,
}

/// Apply `patch` to `[security]` / `[security.pii]` of the config at `path`,
/// keeping everything else as is. Never writes a password.
pub fn write_security_settings(path: &Path, patch: &SecurityPatch) -> Result<(), ConfigError> {
    use toml_edit::{value, Array, DocumentMut, Item, Table};

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
    let sec = doc
        .entry("security")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("[security] is a table");
    sec.insert("mode", value(patch.mode.as_str()));
    sec.insert("approval", value(patch.approval.as_str()));
    match patch
        .vault_dir
        .as_ref()
        .filter(|d| !d.as_os_str().is_empty())
    {
        Some(d) => {
            sec.insert("vault_dir", value(d.to_string_lossy().into_owned()));
        }
        None => {
            sec.remove("vault_dir");
        }
    }
    let pii = sec
        .entry("pii")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .expect("[security.pii] is a table");
    let p = &patch.pii;
    pii.insert("enabled", value(p.enabled));
    for (k, on, min) in [
        ("rrn", p.rrn, p.min_rrn),
        ("account", p.account, p.min_account),
        ("name_phone", p.name_phone, p.min_name_phone),
        ("name_email", p.name_email, p.min_name_email),
    ] {
        pii.insert(k, value(on));
        pii.insert(&format!("min_{k}"), value(i64::from(min.max(1))));
    }
    let mut ex = Array::new();
    for e in p.exclude.iter().filter(|e| !e.trim().is_empty()) {
        ex.push(e.as_str());
    }
    pii.insert("exclude", value(ex));
    pii.insert("ai_check", value(p.ai_check));

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
        if self.curate.context.is_relative() {
            self.curate.context = base.join(&self.curate.context);
        }
        if let Some(p) = &self.store.path {
            if p.is_relative() {
                self.store.path = Some(base.join(p));
            }
        }
    }

    /// Name globs (`[classify] ignore_globs`) plus every watch folder's
    /// `exclude` sub-folders: what the watcher, previews and plans skip.
    pub fn ignore_filter(
        &self,
    ) -> Result<crate::watcher::IgnoreFilter, crate::watcher::WatchError> {
        let dirs: Vec<PathBuf> = self
            .watches
            .iter()
            .flat_map(|w| w.exclude.iter().cloned())
            .collect();
        Ok(crate::watcher::IgnoreFilter::new(&self.effective_ignore_globs())?.with_dirs(dirs))
    }

    /// `[classify] ignore_globs` plus the shortcut globs when
    /// `[general] keep_shortcuts` is on.
    pub fn effective_ignore_globs(&self) -> Vec<String> {
        let mut globs = self.classify.ignore_globs.clone();
        if self.general.keep_shortcuts {
            globs.extend(SHORTCUT_GLOBS.iter().map(|g| g.to_string()));
        }
        globs
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Config = toml::from_str(text)?;
        for w in &mut cfg.watches {
            w.path = expand_home(&w.path);
            if let Some(r) = &w.organize_root {
                w.organize_root = Some(expand_home(r));
            }
            // Relative excludes are sub-folders of the watch folder.
            let base = w.path.clone();
            for e in &mut w.exclude {
                let x = expand_home(e);
                *e = if x.is_relative() { base.join(x) } else { x };
            }
        }
        if let Some(v) = &cfg.security.vault_dir {
            cfg.security.vault_dir = Some(expand_home(v));
        }
        for r in &mut cfg.index_only {
            r.path = expand_home(&r.path);
        }
        cfg.classify.rules = expand_home(&cfg.classify.rules);
        cfg.curate.context = expand_home(&cfg.curate.context);
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
    /// Has the first-run wizard been completed (SPEC §14.1)? An explicit
    /// `[general] onboarded` wins; without the key (configs from before
    /// M8) a config that already watches folders counts as set up, one
    /// without folders (made by the installer or another feature) does not.
    /// Nothing is moved until this is true.
    pub fn onboarded(&self) -> bool {
        self.general.onboarded.unwrap_or(!self.watches.is_empty())
    }

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
            for e in &w.exclude {
                if !e.starts_with(&w.path) {
                    out.push(format!(
                        "{}: 제외 폴더 {}가 이 감시 폴더 안에 있지 않습니다 — 효과가 없습니다.",
                        w.path.display(),
                        e.display()
                    ));
                }
            }
            if w.mode == OrganizeMode::Collect
                && w.organize_root
                    .as_ref()
                    .is_none_or(|r| r.as_os_str().is_empty())
            {
                out.push(format!(
                    "{}: 정리 방식이 '지정 위치로 모으기'인데 기준 폴더(organize_root)가 없습니다 — 이 폴더 안에서 정리합니다.",
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
        if self.security.mode == SecurityMode::Vault && self.security.collects().is_none() {
            out.push(
                "보안 폴더로 모으기가 켜져 있지만 보안 폴더(security.vault_dir)가 없습니다 — 규칙 옵션으로만 암호화합니다."
                    .into(),
            );
        }
        for (what, t) in [
            ("고객사 폴더 틀", &self.curate.template),
            ("기타 폴더 틀", &self.curate.other_template),
        ] {
            if let Err(e) = crate::curate::FolderTemplate::parse(t) {
                out.push(format!(
                    "업무 맥락 {what} 오류: {e} — 업무 맥락 정리를 쓰지 않습니다"
                ));
            }
        }
        if let Err(e) = crate::pii::Detector::new(&self.security.pii) {
            out.push(format!(
                "개인정보 제외 패턴 오류: {e} (제외 패턴 없이 탐지)"
            ));
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
            onboarded: Some(true),
            check_updates: true,
            keep_shortcuts: true,
            duplicate_policy: None,
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
        // (M14) Setting the policy keeps the rest of [mover].
        write_settings(
            &path,
            &SettingsPatch {
                duplicate_policy: Some(DuplicatePolicy::Replace),
                ..patch.clone()
            },
        )
        .unwrap();
        assert_eq!(
            Config::load(&path).unwrap().mover.duplicate_policy,
            DuplicatePolicy::Replace
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("# 언어"));
        assert!(c.general.quiet_mode);
        assert_eq!(c.general.hotkey, "CmdOrCtrl+Alt+K");
        assert!(!c.general.start_on_login);
        assert_eq!(c.general.onboarded, Some(true));
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
                saved: [(
                    "openai".to_string(),
                    ProviderConfig {
                        provider: ProviderKind::Openai,
                        model: "gpt-5-mini".into(),
                        key_ref: Some("tidy/openai".into()),
                        base_url: Some("https://api.openai.com/v1".into()),
                        effort: None,
                    },
                )]
                .into(),
                embed_saved: Default::default(),
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
        // Remembered per provider: the given one plus the current classify.
        let home = c.ai.active().unwrap();
        assert_eq!(home.saved["openai"].model, "gpt-5-mini");
        assert_eq!(home.saved["anthropic"].model, "claude-haiku-5-5");
        assert_eq!(home.embed_saved["ollama"].model, "nomic-embed-text");
        assert!(c.ai.profiles["work"].saved.is_empty());

        // Switching to OpenAI keeps the Anthropic settings for later.
        let mut openai = home.saved["openai"].clone();
        openai.model = "gpt-5".into();
        write_ai_settings(
            &path,
            &AiPatch {
                enabled: true,
                active_profile: "home".into(),
                auto_threshold: 0.85,
                send_content: true,
                classify: Some(openai),
                embed: None,
                saved: home.saved.clone(),
                embed_saved: home.embed_saved.clone(),
            },
        )
        .unwrap();
        let c = Config::load(&path).unwrap();
        let home = c.ai.active().unwrap();
        assert_eq!(home.classify.as_ref().unwrap().model, "gpt-5");
        assert_eq!(home.saved["openai"].model, "gpt-5");
        assert_eq!(
            home.saved["anthropic"].key_ref.as_deref(),
            Some("tidy/anthropic")
        );
        assert!(home.embed.is_none());
        assert_eq!(home.embed_saved["ollama"].model, "nomic-embed-text");
    }

    #[test]
    fn onboarded_flag_and_legacy_configs() {
        // No file / no key, no folders: wizard.
        assert!(!Config::default().onboarded());
        assert!(!Config::from_toml("[general]\nhotkey = \"Ctrl+K\"\n")
            .unwrap()
            .onboarded());
        // Pre-M8 config that already watches folders: set up.
        assert!(Config::from_toml("[[watch]]\npath = \"/d\"\n")
            .unwrap()
            .onboarded());
        // Explicit flag wins either way.
        assert!(
            !Config::from_toml("[general]\nonboarded = false\n[[watch]]\npath = \"/d\"\n")
                .unwrap()
                .onboarded()
        );
        assert!(Config::from_toml("[general]\nonboarded = true\n")
            .unwrap()
            .onboarded());
    }

    #[test]
    fn organize_mode_round_trip_and_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let patch = SettingsPatch {
            watches: vec![
                WatchRoot {
                    path: PathBuf::from("/home/홍/바탕 화면"),
                    ..Default::default()
                },
                WatchRoot {
                    path: PathBuf::from("/home/홍/다운로드"),
                    mode: OrganizeMode::Collect,
                    organize_root: Some(PathBuf::from("/data/정리 보관함")),
                    ..Default::default()
                },
                WatchRoot {
                    path: PathBuf::from("/x"),
                    mode: OrganizeMode::Collect,
                    ..Default::default()
                },
            ],
            store_path: None,
            quiet_mode: false,
            hotkey: "CmdOrCtrl+Shift+Space".into(),
            start_on_login: true,
            onboarded: Some(true),
            check_updates: true,
            keep_shortcuts: true,
            duplicate_policy: None,
        };
        write_settings(&path, &patch).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("in_place"),
            "default mode is not written: {text}"
        );
        let c = Config::load(&path).unwrap();
        assert!(c.onboarded());
        assert_eq!(c.watches[0].mode, OrganizeMode::InPlace);
        assert_eq!(c.watches[0].anchor(), Path::new("/home/홍/바탕 화면"));
        assert_eq!(c.watches[1].mode, OrganizeMode::Collect);
        assert_eq!(c.watches[1].anchor(), Path::new("/data/정리 보관함"));
        assert_eq!(c.watches[2].anchor(), Path::new("/x"));
        assert!(c.warnings().iter().any(|w| w.contains("organize_root")));
        // A later save without `onboarded` leaves the flag alone.
        write_settings(
            &path,
            &SettingsPatch {
                onboarded: None,
                check_updates: true,
                keep_shortcuts: true,
                duplicate_policy: None,
                ..patch
            },
        )
        .unwrap();
        assert!(Config::load(&path).unwrap().general.onboarded == Some(true));
    }

    #[test]
    fn tilde_is_expanded_only_when_leading() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home(Path::new("~/x")), home.join("x"));
        assert_eq!(expand_home(Path::new("a/~/x")), PathBuf::from("a/~/x"));
    }
}
