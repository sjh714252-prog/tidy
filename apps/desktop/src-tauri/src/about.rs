//! (M11, SPEC §14.6) 도움말 / 정보 / 업데이트 확인, and the app's log file.
//!
//! The update check is one GET with a 10 s limit; failures are shown as
//! "확인하지 못했습니다" or, for the daily background check, ignored. The
//! last result lives next to the config file in `update-check.json` (app
//! state, not a user file).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use tidy_core::update::{self, UpdateInfo, Version};

use crate::commands::{blocking, CmdResult};
use crate::state::AppState;

pub const REPO_URL: &str = "https://github.com/sjh714252-prog/tidy";
/// Short commit of this build (release.yml sets it), "dev" otherwise.
pub const GIT_SHA: &str = match option_env!("TIDY_GIT_SHA") {
    Some(s) => s,
    None => "dev",
};
const DAY: Duration = Duration::from_secs(24 * 3600);
const LOG_MAX: u64 = 5 * 1024 * 1024;

/// Newer release found by the last check (status bar dot).
static AVAILABLE: Mutex<Option<UpdateInfo>> = Mutex::new(None);
/// Dev builds with `TIDY_E2E_NO_OPEN=1`: URLs "opened" (not launched).
static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn available() -> Option<UpdateInfo> {
    AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// ── log file ────────────────────────────────────────────────────────────

/// `<local data>/tidy/logs` (Windows `%LOCALAPPDATA%\tidy\logs`).
pub fn log_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tidy")
        .join("logs")
}

/// `tidy.log` in [`log_dir`], emptied when it grew past 5 MB. `None` when
/// it cannot be opened (logging then goes to the console only).
pub fn open_log_file() -> Option<std::fs::File> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("tidy.log");
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX);
    std::fs::OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(path)
        .ok()
}

// ── 정보 ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutDto {
    pub version: String,
    pub git_sha: String,
    pub repo: String,
    pub license: String,
    pub os: String,
    pub config_path: String,
    pub config_dir: String,
    pub db_path: String,
    pub db_dir: String,
    pub log_dir: String,
    pub check_updates: bool,
}

fn parent(p: &Path) -> String {
    p.parent().unwrap_or(p).display().to_string()
}

#[tauri::command]
pub async fn about_info(app: AppHandle) -> CmdResult<AboutDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config();
        let db = cfg.store.db_path();
        Ok(AboutDto {
            version: app.package_info().version.to_string(),
            git_sha: GIT_SHA.to_string(),
            repo: REPO_URL.to_string(),
            license: "MIT".into(),
            os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            config_path: state.config_path.display().to_string(),
            config_dir: parent(&state.config_path),
            db_path: db.display().to_string(),
            db_dir: parent(&db),
            log_dir: log_dir().display().to_string(),
            check_updates: cfg.general.check_updates,
        })
    })
    .await
}

/// Open a web page in the default browser (https only).
#[tauri::command]
pub async fn open_url(app: AppHandle, url: String) -> CmdResult<()> {
    if !url.starts_with("https://")
        && !(cfg!(debug_assertions) && url.starts_with("http://127.0.0.1"))
    {
        return Err(format!("열 수 없는 주소: {url}"));
    }
    if e2e_record(&url) {
        return Ok(());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Open a folder in the file manager (정보 창의 "폴더 열기"). Creates
/// nothing: a missing folder is an error.
#[tauri::command]
pub async fn open_folder(app: AppHandle, path: String) -> CmdResult<()> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err(format!("폴더가 없습니다: {path}"));
    }
    if e2e_record(&path) {
        return Ok(());
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Dev builds with `TIDY_E2E_NO_OPEN=1`: record `what` instead of opening
/// it (true = recorded, skip the real open).
pub fn e2e_record(what: &str) -> bool {
    if cfg!(debug_assertions) && std::env::var_os("TIDY_E2E_NO_OPEN").is_some() {
        OPENED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(what.to_string());
        return true;
    }
    false
}

/// Dev bridge only: what `open_url` / `open_folder` / `reveal_path` recorded.
#[cfg(debug_assertions)]
pub fn opened() -> Vec<String> {
    OPENED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// ── 업데이트 확인 ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    pub current: String,
    /// `None` when the check failed.
    pub info: Option<UpdateInfo>,
    pub error: Option<String>,
    pub checked_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    checked_at: i64,
    info: Option<UpdateInfo>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn saved_path(app: &AppHandle) -> PathBuf {
    let state = app.state::<AppState>();
    state
        .config_path
        .parent()
        .map(|d| d.join("update-check.json"))
        .unwrap_or_else(|| PathBuf::from("update-check.json"))
}

fn load_saved(app: &AppHandle) -> Saved {
    std::fs::read_to_string(saved_path(app))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn remember(current: &str, info: Option<&UpdateInfo>) {
    let newer = info
        .filter(|i| {
            // Re-check against this build: the saved answer may predate an update.
            matches!((Version::parse(&i.latest), Version::parse(current)), (Some(l), Some(c)) if l > c)
        })
        .cloned();
    *AVAILABLE.lock().unwrap_or_else(|e| e.into_inner()) = newer;
}

async fn run_check(app: &AppHandle) -> UpdateResult {
    let current = app.package_info().version.to_string();
    let res = update::check(&update::api_url(), &current).await;
    let checked_at = now();
    let out = match res {
        Ok(info) => UpdateResult {
            current: current.clone(),
            info: Some(info),
            error: None,
            checked_at,
        },
        Err(e) => UpdateResult {
            current: current.clone(),
            info: None,
            error: Some(e.to_string()),
            checked_at,
        },
    };
    if out.info.is_some() {
        remember(&current, out.info.as_ref());
        let saved = Saved {
            checked_at,
            info: out.info.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&saved) {
            let _ = std::fs::write(saved_path(app), text);
        }
    }
    let _ = app.emit(crate::watch::EV_STATUS, ());
    out
}

/// 도움말 → 업데이트 확인.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> CmdResult<UpdateResult> {
    Ok(run_check(&app).await)
}

/// At start: show what the last check found, then (if enabled) check in
/// the background once a day. Never blocks start-up; errors are dropped.
pub fn spawn_daily_check(app: &AppHandle) {
    let current = app.package_info().version.to_string();
    let saved = load_saved(app);
    remember(&current, saved.info.as_ref());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio_sleep(Duration::from_secs(10)).await;
        loop {
            let cfg = app.state::<AppState>().config();
            let last = load_saved(&app).checked_at;
            if cfg.general.check_updates && now() - last >= DAY.as_secs() as i64 {
                let r = run_check(&app).await;
                if let Some(e) = r.error {
                    tracing::info!("update check failed (ignored): {e}");
                }
            }
            tokio_sleep(Duration::from_secs(3600)).await;
        }
    });
}

async fn tokio_sleep(d: Duration) {
    // tauri's async runtime is tokio; avoid a direct tokio dependency.
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}
