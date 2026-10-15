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
        source: toml::de::Error,
    },
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
    pub stabilize_secs: u64,
    pub learn_threshold: u32,
    pub ignore_globs: Vec<String>,
}

impl Default for Classify {
    fn default() -> Self {
        Self {
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
    pub max_content_size_mb: u64,
    pub max_hash_size_mb_network: u64,
    pub cpu_limit_percent: u8,
}

impl Default for Index {
    fn default() -> Self {
        Self {
            max_content_size_mb: 200,
            max_hash_size_mb_network: 20,
            cpu_limit_percent: 30,
        }
    }
}

impl Config {
    /// Load from a TOML file. Missing keys fall back to defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Config = toml::from_str(text)?;
        for w in &mut cfg.watches {
            w.path = expand_home(&w.path);
        }
        for r in &mut cfg.index_only {
            r.path = expand_home(&r.path);
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
    fn tilde_is_expanded_only_when_leading() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home(Path::new("~/x")), home.join("x"));
        assert_eq!(expand_home(Path::new("a/~/x")), PathBuf::from("a/~/x"));
    }
}
