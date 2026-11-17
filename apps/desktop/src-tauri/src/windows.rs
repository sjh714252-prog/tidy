//! Showing / hiding the three windows defined in tauri.conf.json and the
//! global search hotkey.

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::state::AppState;

pub const EV_NAVIGATE: &str = "tidy://navigate";
pub const EV_FOCUS_SEARCH: &str = "tidy://focus-search";

/// Show the main window on a tab: "pending" | "activity" | "settings".
pub fn show_main(app: &AppHandle, tab: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit(EV_NAVIGATE, tab);
    }
}

/// First-run wizard (SPEC §14.1): its own window.
pub fn show_wizard(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("wizard") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit(EV_NAVIGATE, "wizard");
    }
}

/// Tray left click / second launch: the wizard until setup is done, then
/// the main window.
pub fn show_start(app: &AppHandle) {
    if app.state::<AppState>().config().onboarded() {
        show_main(app, "activity");
    } else {
        show_wizard(app);
    }
}

pub fn show_search(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("search") {
        let _ = w.center();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = w.emit(EV_FOCUS_SEARCH, ());
    }
}

pub fn toggle_search(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("search") {
        if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show_search(app);
        }
    }
}

/// Confirm popup: shown without stealing focus from what the user is doing.
/// Called from the watch thread too: the window work is handed to the main
/// thread without waiting (0.18.5, see `tray::on_main`).
pub fn show_confirm(app: &AppHandle) {
    let app2 = app.clone();
    crate::tray::on_main(app, move || show_confirm_now(&app2));
}

fn show_confirm_now(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("confirm") {
        if !w.is_visible().unwrap_or(false) {
            if let Ok(Some(m)) = w.current_monitor() {
                // Bottom-right corner, above the taskbar.
                let size = m.size();
                let scale = m.scale_factor();
                let (ww, wh) = (460.0 * scale, 420.0 * scale);
                let x = m.position().x as f64 + size.width as f64 - ww - 24.0 * scale;
                let y = m.position().y as f64 + size.height as f64 - wh - 72.0 * scale;
                let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
            }
            let _ = w.show();
        }
        let _ = w.emit(crate::watch::EV_PENDING, ());
    }
}

/// Register `hotkey` (e.g. "CmdOrCtrl+Shift+Space") for the search window,
/// replacing the previous one. Errors (conflict with another app, bad
/// syntax) are reported in the status bar, never fatal.
pub fn register_hotkey(app: &AppHandle, hotkey: &str) {
    let state = app.state::<AppState>();
    let gs = app.global_shortcut();
    let previous = state.lock_inner().hotkey.take();
    if let Some(prev) = previous {
        let _ = gs.unregister(prev.as_str());
    }
    match gs.register(hotkey) {
        Ok(()) => {
            state.lock_inner().hotkey = Some(hotkey.to_string());
            state.set_hotkey_error(None);
        }
        Err(e) => state.set_hotkey_error(Some(format!("단축키 {hotkey} 등록 실패: {e}"))),
    }
}

/// (0.18.5) Logs when the main thread (tray clicks, menus, windows) stops
/// answering, and for how long, so a frozen tray leaves a trace in
/// `tidy.log` ("로그 폴더 열기").
pub fn spawn_ui_watchdog(app: &AppHandle) {
    use std::time::{Duration, Instant};
    const EVERY: Duration = Duration::from_secs(10);
    const LIMIT: Duration = Duration::from_secs(20);
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("tidy-ui-watchdog".into())
        .spawn(move || loop {
            std::thread::sleep(EVERY);
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let sent = Instant::now();
            if app
                .run_on_main_thread(move || {
                    let _ = tx.send(());
                })
                .is_err()
            {
                return; // the app is shutting down
            }
            if rx.recv_timeout(LIMIT).is_ok() {
                continue;
            }
            tracing::warn!(
                "UI thread not answering for {}s (tray clicks and menus do nothing meanwhile)",
                LIMIT.as_secs()
            );
            if rx.recv().is_err() {
                return;
            }
            tracing::warn!(
                "UI thread answering again after {}s",
                sent.elapsed().as_secs()
            );
        });
}
