//! Configuration model (SPEC §7). Loaded from `config.toml`; every field
//! has a default so an empty file is valid.

use std::path::{Path, PathBuf};

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
#[serde(default)]
pub struct WatchRoot {
    pub path: PathBuf,
    pub kind: RootKind,
    pub recursive: bool,
    pub index_content: bool,
    pub poll_interval_s: Option<u64>,
    pub content_index_window: Option<String>,
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
        if let Some(p) = &cfg.store.path {
            cfg.store.path = Some(expand_home(p));
        }
        Ok(cfg)
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
                    ..Default::default()
                },
            ],
            store_path: None,
            quiet_mode: true,
            hotkey: "CmdOrCtrl+Alt+K".into(),
        };
        write_settings(&path, &patch).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# 내 설정") && text.contains("# 언어"));
        let c = Config::load(&path).unwrap();
        assert_eq!(c.watches.len(), 2);
        assert_eq!(c.watches[0].path, patch.watches[0].path);
        assert_eq!(c.watches[1].kind, RootKind::Network);
        assert_eq!(c.watches[1].poll_interval_s, Some(120));
        assert_eq!(c.mover.duplicate_policy, DuplicatePolicy::Rename);
        assert!(c.general.quiet_mode);
        assert_eq!(c.general.hotkey, "CmdOrCtrl+Alt+K");
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
    fn tilde_is_expanded_only_when_leading() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home(Path::new("~/x")), home.join("x"));
        assert_eq!(expand_home(Path::new("a/~/x")), PathBuf::from("a/~/x"));
    }
}
