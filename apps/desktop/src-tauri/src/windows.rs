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
pub fn show_confirm(app: &AppHandle) {
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
        Ok(()) => state.lock_inner().hotkey = Some(hotkey.to_string()),
        Err(e) => state.set_error(format!("단축키 {hotkey} 등록 실패: {e}")),
    }
}
