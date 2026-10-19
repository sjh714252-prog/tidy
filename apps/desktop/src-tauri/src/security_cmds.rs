//! (M9) Security settings, the vault password and opening vault zips
//! (SPEC §14.2). The password goes from the settings field straight to the
//! OS keychain; it is never written to the config, logged, or sent back.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use tidy_core::config::{write_security_settings, Approval, PiiCfg, SecurityMode, SecurityPatch};
use tidy_core::security::{self, VaultKeys};
use tidy_core::Config;

use crate::commands::{blocking, CmdResult};
use crate::state::AppState;
use crate::watch;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiiDto {
    pub enabled: bool,
    pub rrn: bool,
    pub account: bool,
    pub name_phone: bool,
    pub name_email: bool,
    pub min_rrn: u32,
    pub min_account: u32,
    pub min_name_phone: u32,
    pub min_name_email: u32,
    pub exclude: Vec<String>,
    pub ai_check: bool,
}

impl From<&PiiCfg> for PiiDto {
    fn from(p: &PiiCfg) -> Self {
        Self {
            enabled: p.enabled,
            rrn: p.rrn,
            account: p.account,
            name_phone: p.name_phone,
            name_email: p.name_email,
            min_rrn: p.min_rrn,
            min_account: p.min_account,
            min_name_phone: p.min_name_phone,
            min_name_email: p.min_name_email,
            exclude: p.exclude.clone(),
            ai_check: p.ai_check,
        }
    }
}

impl From<&PiiDto> for PiiCfg {
    fn from(p: &PiiDto) -> Self {
        Self {
            enabled: p.enabled,
            rrn: p.rrn,
            account: p.account,
            name_phone: p.name_phone,
            name_email: p.name_email,
            min_rrn: p.min_rrn.max(1),
            min_account: p.min_account.max(1),
            min_name_phone: p.min_name_phone.max(1),
            min_name_email: p.min_name_email.max(1),
            exclude: p
                .exclude
                .iter()
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect(),
            ai_check: p.ai_check,
        }
    }
}

/// What the settings section edits plus read-only state. Never contains a
/// password.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityDto {
    /// "rules" | "vault"
    pub mode: String,
    pub vault_dir: String,
    /// "confirm" | "auto"
    pub approval: String,
    pub pii: PiiDto,
    // ── read-only ──
    #[serde(default)]
    pub has_password: bool,
    #[serde(default)]
    pub previous_passwords: usize,
    #[serde(default)]
    pub key_store: String,
    #[serde(default)]
    pub key_error: Option<String>,
    /// Personal-information suspects in the index / of which unencrypted.
    #[serde(default)]
    pub suspected: u64,
    #[serde(default)]
    pub unencrypted: u64,
    /// Vault zips in effect (active encrypt moves).
    #[serde(default)]
    pub locked: u64,
}

fn security_dto(app: &AppHandle, cfg: &Config) -> SecurityDto {
    let s = &cfg.security;
    let key = security::vault_key_status();
    let state = app.state::<AppState>();
    let inner = state.lock_inner();
    let (suspected, unencrypted, locked) = inner
        .mover
        .as_ref()
        .map(|m| {
            let (a, b) = m.store().pii_totals(None).unwrap_or((0, 0));
            (a, b, m.store().active_encrypted().unwrap_or(0))
        })
        .unwrap_or((0, 0, 0));
    SecurityDto {
        mode: s.mode.as_str().into(),
        vault_dir: s
            .vault_dir
            .as_deref()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        approval: s.approval.as_str().into(),
        pii: (&s.pii).into(),
        has_password: key.has_password,
        previous_passwords: key.previous,
        key_store: key.store,
        key_error: key.error,
        suspected,
        unencrypted,
        locked,
    }
}

#[tauri::command]
pub async fn get_security_settings(app: AppHandle) -> CmdResult<SecurityDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        Ok(security_dto(&app, &cfg))
    })
    .await
}

/// Save `[security]` (comments kept). A typed `password` sets or changes
/// the vault password in the OS keychain (the old one is kept for old
/// zips). The watch loop restarts so the new policy applies.
#[tauri::command]
pub async fn save_security_settings(
    app: AppHandle,
    settings: SecurityDto,
    password: Option<String>,
) -> CmdResult<SecurityDto> {
    blocking(move || {
        let mode = match settings.mode.as_str() {
            "vault" => SecurityMode::Vault,
            "rules" | "" => SecurityMode::Rules,
            other => return Err(format!("알 수 없는 보관 모드: {other}")),
        };
        let approval = match settings.approval.as_str() {
            "auto" => Approval::Auto,
            "confirm" | "" => Approval::Confirm,
            other => return Err(format!("알 수 없는 승인 방식: {other}")),
        };
        let vault_dir = Some(settings.vault_dir.trim())
            .filter(|d| !d.is_empty())
            .map(|d| tidy_core::config::expand_home(Path::new(d)));
        if mode == SecurityMode::Vault && vault_dir.is_none() {
            return Err("보안 폴더로 모으려면 보안 폴더를 고르세요".into());
        }
        let pii: PiiCfg = (&settings.pii).into();
        if let Err(e) = tidy_core::pii::Detector::new(&pii) {
            return Err(format!("제외 패턴 오류: {e}"));
        }
        if let Some(pw) = password.as_deref().filter(|p| !p.is_empty()) {
            security::set_vault_password(pw).map_err(|e| e.to_string())?;
        }
        let state = app.state::<AppState>();
        write_security_settings(
            &state.config_path,
            &SecurityPatch {
                mode,
                vault_dir,
                approval,
                pii,
            },
        )
        .map_err(|e| e.to_string())?;
        state.reload();
        watch::restart(&app);
        Ok(security_dto(&app, &state.config()))
    })
    .await
}

/// Is `path` a vault zip Tidy made (index row or active encrypt move)?
pub fn is_vault_zip(app: &AppHandle, path: &Path) -> bool {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        return false;
    }
    let state = app.state::<AppState>();
    let inner = state.lock_inner();
    let Some(m) = inner.mover.as_ref() else {
        return false;
    };
    let store = m.store();
    store.get_file(path).ok().flatten().is_some_and(|f| f.vault)
        || store
            .active_move_to(path)
            .ok()
            .flatten()
            .is_some_and(|e| e.op == tidy_core::store::MoveOp::Encrypt)
}

/// Decrypt a vault zip to `<temp>/tidy-open/<id>/` with the keychain
/// password and return the copy's path (removed after `open_ttl_minutes`).
pub fn decrypt_copy(path: &Path) -> CmdResult<PathBuf> {
    let keys = VaultKeys::from_keychain().map_err(|e| e.to_string())?;
    tidy_core::mover::open_temp_copy(path, &keys).map_err(|e| e.to_string())
}

/// "열기" on a 🔒 result: decrypt a temporary copy and open it. Returns the
/// copy's path.
#[tauri::command]
pub async fn open_vault_file(app: AppHandle, path: String) -> CmdResult<String> {
    let copy = blocking(move || decrypt_copy(Path::new(&path))).await?;
    let shown = copy.display().to_string();
    app.opener()
        .open_path(shown.clone(), None::<&str>)
        .map_err(|e| format!("임시 사본을 만들었지만 열지 못했습니다({shown}): {e}"))?;
    Ok(shown)
}

/// Same as [`open_vault_file`] without launching an app (tests, "사본 위치").
#[tauri::command]
pub async fn vault_copy(path: String) -> CmdResult<String> {
    blocking(move || decrypt_copy(Path::new(&path)).map(|p| p.display().to_string())).await
}

/// Remove decrypted copies older than `open_ttl_minutes`: at start and
/// then every minute.
pub fn spawn_open_sweeper(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("tidy-open-sweep".into())
        .spawn(move || loop {
            let ttl = app.state::<AppState>().config().security.open_ttl_minutes;
            let n = tidy_core::mover::sweep_open_copies(Duration::from_secs(ttl.max(1) * 60));
            if n > 0 {
                tracing::info!(removed = n, "temporary vault copies removed");
            }
            std::thread::sleep(Duration::from_secs(60));
        })
        .ok();
}
