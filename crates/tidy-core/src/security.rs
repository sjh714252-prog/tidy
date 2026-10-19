//! Vault password in the OS keychain only (SPEC §14.2, CLAUDE.md rule 5).
//!
//! The current password is `tidy/vault`. When it changes, the old one is
//! kept in `tidy/vault-previous` (a JSON list) so zips locked earlier can
//! still be opened and undone; nothing is re-encrypted. Passwords never go
//! to the config, the database, logs or the UI.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;
use zeroize::Zeroizing;

use crate::config::{Approval, Config};
use crate::pii::Detector;

/// What the engine needs from `[security]` (SPEC §14.2).
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    pub detector: Arc<Detector>,
    /// Mode A: personal files go here (encrypted) before any rule.
    pub vault_dir: Option<PathBuf>,
    pub approval: Approval,
    /// Text extraction limits (same as the indexer).
    pub max_bytes: u64,
    pub max_chars: usize,
    /// Ask the AI to confirm a regex hit (when an AI is configured).
    pub ai_check: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl SecurityPolicy {
    pub fn from_config(cfg: &Config) -> Self {
        let s = &cfg.security;
        Self {
            detector: Arc::new(Detector::lenient(&s.pii)),
            vault_dir: s.collects().map(crate::paths::normalize_lexical),
            approval: s.approval,
            max_bytes: cfg.index.max_content_size_mb * 1024 * 1024,
            max_chars: cfg.index.max_text_chars,
            ai_check: s.pii.ai_check,
        }
    }
}

pub const VAULT_KEY_REF: &str = "tidy/vault";
pub const VAULT_PREVIOUS_REF: &str = "tidy/vault-previous";
/// Older passwords kept (newest first).
const MAX_PREVIOUS: usize = 20;
pub const MIN_PASSWORD_CHARS: usize = 8;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("보관 비밀번호가 설정되지 않았습니다 — 설정 → 보안(개인정보)에서 지정하세요")]
    NoPassword,
    #[error("비밀번호는 {MIN_PASSWORD_CHARS}자 이상이어야 합니다")]
    TooShort,
    #[error("키체인: {0}")]
    Keychain(String),
}

/// The vault passwords: the current one first, then older ones.
#[derive(Clone, Default)]
pub struct VaultKeys {
    current: Option<Zeroizing<String>>,
    previous: Vec<Zeroizing<String>>,
}

impl fmt::Debug for VaultKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultKeys")
            .field("current", &self.current.as_ref().map(|_| "<set>"))
            .field("previous", &self.previous.len())
            .finish()
    }
}

impl VaultKeys {
    pub fn new(current: &str) -> Self {
        Self {
            current: Some(Zeroizing::new(current.to_string())),
            previous: Vec::new(),
        }
    }

    pub fn with_previous(mut self, previous: &[&str]) -> Self {
        self.previous = previous
            .iter()
            .map(|p| Zeroizing::new((*p).to_string()))
            .collect();
        self
    }

    pub fn none() -> Self {
        Self::default()
    }

    /// The password new zips are locked with.
    pub fn current(&self) -> Result<&str, SecurityError> {
        self.current
            .as_deref()
            .map(String::as_str)
            .ok_or(SecurityError::NoPassword)
    }

    pub fn has_current(&self) -> bool {
        self.current.is_some()
    }

    pub fn previous_count(&self) -> usize {
        self.previous.len()
    }

    /// Every password worth trying to open a zip, current first.
    pub fn candidates(&self) -> impl Iterator<Item = &str> {
        self.current
            .iter()
            .chain(self.previous.iter())
            .map(|s| s.as_str())
    }

    /// Read both entries from the keychain. A missing password is not an
    /// error here (`current()` reports it when one is needed).
    pub fn from_keychain() -> Result<Self, SecurityError> {
        let current = crate::ai::keys::get_key(VAULT_KEY_REF)
            .map_err(|e| SecurityError::Keychain(e.to_string()))?
            .map(Zeroizing::new);
        let previous = crate::ai::keys::get_key(VAULT_PREVIOUS_REF)
            .map_err(|e| SecurityError::Keychain(e.to_string()))?
            .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
            .unwrap_or_default()
            .into_iter()
            .map(Zeroizing::new)
            .collect();
        Ok(Self { current, previous })
    }
}

/// Set (or change) the vault password. The old one moves to the
/// "previous" list so existing zips stay readable.
pub fn set_vault_password(new: &str) -> Result<(), SecurityError> {
    if new.chars().count() < MIN_PASSWORD_CHARS {
        return Err(SecurityError::TooShort);
    }
    let kc = |e: crate::ai::AiError| SecurityError::Keychain(e.to_string());
    let keys = VaultKeys::from_keychain()?;
    if let Some(old) = keys.current.as_deref().filter(|o| o.as_str() != new) {
        let mut prev: Vec<String> = vec![old.to_string()];
        prev.extend(
            keys.previous
                .iter()
                .map(|p| p.to_string())
                .filter(|p| p != new && p != old.as_str()),
        );
        prev.truncate(MAX_PREVIOUS);
        let json = Zeroizing::new(serde_json::to_string(&prev).unwrap_or_default());
        crate::ai::keys::set_key(VAULT_PREVIOUS_REF, &json).map_err(kc)?;
    }
    crate::ai::keys::set_key(VAULT_KEY_REF, new).map_err(kc)
}

/// What the settings screen shows (never the password itself).
#[derive(Debug, Clone, serde::Serialize)]
pub struct VaultKeyStatus {
    pub has_password: bool,
    pub previous: usize,
    /// Where it is stored ("Windows 자격 증명 관리자", …).
    pub store: String,
    /// The keychain could not be read.
    pub error: Option<String>,
}

pub fn vault_key_status() -> VaultKeyStatus {
    let store = crate::ai::keys::store_name().to_string();
    match VaultKeys::from_keychain() {
        Ok(k) => VaultKeyStatus {
            has_password: k.has_current(),
            previous: k.previous_count(),
            store,
            error: None,
        },
        Err(e) => VaultKeyStatus {
            has_password: false,
            previous: 0,
            store,
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_passwords() {
        let k = VaultKeys::new("비밀번호1234").with_previous(&["옛비밀번호99"]);
        let d = format!("{k:?}");
        assert!(!d.contains("1234") && !d.contains("옛"), "{d}");
        assert_eq!(
            k.candidates().collect::<Vec<_>>(),
            ["비밀번호1234", "옛비밀번호99"]
        );
        assert!(matches!(
            VaultKeys::none().current(),
            Err(SecurityError::NoPassword)
        ));
    }

    #[test]
    fn short_passwords_are_refused_before_the_keychain() {
        assert!(matches!(
            set_vault_password("짧음"),
            Err(SecurityError::TooShort)
        ));
    }
}
