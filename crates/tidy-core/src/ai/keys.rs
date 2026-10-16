//! API keys in the OS keychain only (CLAUDE.md rule 5): Windows Credential
//! Manager, macOS Keychain, Linux Secret Service (GNOME Keyring / KWallet
//! over D-Bus). Config files only hold the key's name (`key_ref`,
//! "service/user", e.g. "tidy/anthropic"). Keys are never logged or
//! returned to the UI.
//!
//! Linux (SPEC §13, decided in M7): the Secret Service is the store — it
//! persists across reboots and is unlocked with the login session. Where
//! no Secret Service answers on D-Bus (minimal window managers, headless
//! boxes), keys fall back to the kernel keyring (keyutils), which keeps
//! them only until logout; [`store_name`] says which one is in use.

use super::provider::AiError;

fn split(key_ref: &str) -> (&str, &str) {
    match key_ref.split_once('/') {
        Some((s, u)) if !s.is_empty() && !u.is_empty() => (s, u),
        _ => ("tidy", key_ref),
    }
}

fn kc(e: keyring::Error) -> AiError {
    AiError::Keychain(e.to_string())
}

/// `Ok(None)` when no key is stored under `key_ref`.
pub fn get_key(key_ref: &str) -> Result<Option<String>, AiError> {
    let found = imp::get(key_ref)?;
    Ok(found.filter(|k| !k.trim().is_empty()))
}

pub fn set_key(key_ref: &str, secret: &str) -> Result<(), AiError> {
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(AiError::Keychain("empty key".into()));
    }
    imp::set(key_ref, secret)
}

pub fn delete_key(key_ref: &str) -> Result<(), AiError> {
    imp::delete(key_ref)
}

pub fn has_key(key_ref: &str) -> bool {
    matches!(get_key(key_ref), Ok(Some(_)))
}

/// Human-readable name of the store keys go to right now.
pub fn store_name() -> &'static str {
    imp::store_name()
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{kc, split, AiError};

    fn entry(key_ref: &str) -> Result<keyring::Entry, AiError> {
        let (service, user) = split(key_ref);
        keyring::Entry::new(service, user).map_err(kc)
    }

    pub fn get(key_ref: &str) -> Result<Option<String>, AiError> {
        match entry(key_ref)?.get_password() {
            Ok(k) => Ok(Some(k)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(kc(e)),
        }
    }

    pub fn set(key_ref: &str, secret: &str) -> Result<(), AiError> {
        entry(key_ref)?.set_password(secret).map_err(kc)
    }

    pub fn delete(key_ref: &str) -> Result<(), AiError> {
        match entry(key_ref)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(kc(e)),
        }
    }

    pub fn store_name() -> &'static str {
        if cfg!(windows) {
            "Windows 자격 증명 관리자"
        } else if cfg!(target_os = "macos") {
            "macOS 키체인"
        } else {
            "OS 키체인"
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use keyring::keyutils::KeyutilsCredential;
    use keyring::secret_service::SsCredential;
    use keyring::{Entry, Error};

    use super::{kc, split, AiError};

    fn secret_service(key_ref: &str) -> Result<Entry, Error> {
        let (service, user) = split(key_ref);
        SsCredential::new_with_target(None, service, user)
            .map(|c| Entry::new_with_credential(Box::new(c)))
    }

    fn keyutils(key_ref: &str) -> Result<Entry, Error> {
        let (service, user) = split(key_ref);
        KeyutilsCredential::new_with_target(None, service, user)
            .map(|c| Entry::new_with_credential(Box::new(c)))
    }

    /// No Secret Service on the bus (or no bus at all).
    fn unreachable(e: &Error) -> bool {
        matches!(e, Error::PlatformFailure(_) | Error::NoStorageAccess(_))
    }

    pub fn get(key_ref: &str) -> Result<Option<String>, AiError> {
        match secret_service(key_ref).and_then(|e| e.get_password()) {
            Ok(k) => return Ok(Some(k)),
            Err(Error::NoEntry) => {}
            Err(e) if unreachable(&e) => {}
            Err(e) => return Err(kc(e)),
        }
        // Fallback store (also finds keys saved while no Secret Service ran).
        match keyutils(key_ref).and_then(|e| e.get_password()) {
            Ok(k) => Ok(Some(k)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(kc(e)),
        }
    }

    pub fn set(key_ref: &str, secret: &str) -> Result<(), AiError> {
        match secret_service(key_ref).and_then(|e| e.set_password(secret)) {
            Ok(()) => {
                // Don't leave an older copy behind in the fallback store.
                if let Ok(e) = keyutils(key_ref) {
                    let _ = e.delete_credential();
                }
                Ok(())
            }
            Err(e) if unreachable(&e) => {
                tracing::warn!(
                    "Secret Service unavailable ({e}); key kept in the kernel keyring until logout"
                );
                keyutils(key_ref)
                    .and_then(|e| e.set_password(secret))
                    .map_err(|e| match e {
                        // No session keyring either (no login session).
                        Error::NoEntry => AiError::Keychain(
                            "Secret Service(GNOME 키링/KWallet)에 연결할 수 없고 커널 세션 \
                             키링도 없습니다 — 키링 데몬이 실행 중인 데스크톱 세션에서 \
                             저장하세요"
                                .into(),
                        ),
                        e => kc(e),
                    })
            }
            Err(e) => Err(kc(e)),
        }
    }

    pub fn delete(key_ref: &str) -> Result<(), AiError> {
        for entry in [secret_service(key_ref), keyutils(key_ref)] {
            match entry.and_then(|e| e.delete_credential()) {
                Ok(()) | Err(Error::NoEntry) => {}
                Err(e) if unreachable(&e) => {}
                Err(e) => return Err(kc(e)),
            }
        }
        Ok(())
    }

    pub fn store_name() -> &'static str {
        match secret_service("tidy/__probe__").and_then(|e| e.get_password()) {
            Err(e) if unreachable(&e) => "커널 키링(keyutils, 로그아웃 시 삭제)",
            _ => "Secret Service(GNOME 키링/KWallet)",
        }
    }
}
