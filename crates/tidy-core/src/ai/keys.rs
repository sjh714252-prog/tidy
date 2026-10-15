//! API keys in the OS keychain only (CLAUDE.md rule 5): Windows Credential
//! Manager, macOS Keychain, Linux kernel keyutils. Config files only hold
//! the key's name (`key_ref`, "service/user", e.g. "tidy/anthropic").
//! Keys are never logged or returned to the UI.

use super::provider::AiError;

fn entry(key_ref: &str) -> Result<keyring::Entry, AiError> {
    let (service, user) = match key_ref.split_once('/') {
        Some((s, u)) if !s.is_empty() && !u.is_empty() => (s, u),
        _ => ("tidy", key_ref),
    };
    keyring::Entry::new(service, user).map_err(|e| AiError::Keychain(e.to_string()))
}

/// `Ok(None)` when no key is stored under `key_ref`.
pub fn get_key(key_ref: &str) -> Result<Option<String>, AiError> {
    match entry(key_ref)?.get_password() {
        Ok(k) if !k.trim().is_empty() => Ok(Some(k)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AiError::Keychain(e.to_string())),
    }
}

pub fn set_key(key_ref: &str, secret: &str) -> Result<(), AiError> {
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(AiError::Keychain("empty key".into()));
    }
    entry(key_ref)?
        .set_password(secret)
        .map_err(|e| AiError::Keychain(e.to_string()))
}

pub fn delete_key(key_ref: &str) -> Result<(), AiError> {
    match entry(key_ref)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AiError::Keychain(e.to_string())),
    }
}

pub fn has_key(key_ref: &str) -> bool {
    matches!(get_key(key_ref), Ok(Some(_)))
}
