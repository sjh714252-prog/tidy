//! Start at login, tray only (SPEC §9, M7): `general.start_on_login` is
//! mirrored to the OS login items — Windows `HKCU\…\Run`, macOS
//! LaunchAgent, Linux `~/.config/autostart/*.desktop` — through
//! tauri-plugin-autostart. The registered command carries [`ARG`], so a
//! login start never opens a window.
//!
//! Dev builds (`tauri dev`) do not register themselves: the entry would
//! point at `target/debug`. `TIDY_DEV_AUTOSTART=1` overrides that for tests.

use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// Command-line flag of the login item.
pub const ARG: &str = "--autostart";

pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![ARG]))
}

/// This process was started by the OS at login.
pub fn launched_at_login() -> bool {
    std::env::args().any(|a| a == ARG)
}

/// Whether this build touches the OS login items at all.
pub fn applies() -> bool {
    !cfg!(debug_assertions) || std::env::var_os("TIDY_DEV_AUTOSTART").is_some()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartDto {
    /// `general.start_on_login`.
    pub wanted: bool,
    /// Registered with the OS right now (`None` in dev builds).
    pub registered: Option<bool>,
    pub error: Option<String>,
}

/// Make the OS login item match `wanted`.
pub fn sync(app: &AppHandle, wanted: bool) -> AutostartDto {
    if !applies() {
        return AutostartDto {
            wanted,
            registered: None,
            error: None,
        };
    }
    let al = app.autolaunch();
    let res = match (wanted, al.is_enabled()) {
        (true, Ok(true)) | (false, Ok(false)) => Ok(()),
        (true, _) => al.enable(),
        (false, _) => al.disable(),
    };
    if let Err(e) = &res {
        tracing::warn!("autostart: {e}");
    }
    AutostartDto {
        wanted,
        registered: al.is_enabled().ok(),
        error: res.err().map(|e| format!("자동 시작 설정 실패: {e}")),
    }
}

pub fn state(app: &AppHandle, wanted: bool) -> AutostartDto {
    AutostartDto {
        wanted,
        registered: applies().then(|| app.autolaunch().is_enabled().unwrap_or(false)),
        error: None,
    }
}
