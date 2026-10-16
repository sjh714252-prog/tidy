//! Tray icon + menu (SPEC §9.1): 검색 열기 / 대기 N건 / 일시정지 / 지금 인덱싱 /
//! 설정 / 종료.

use std::sync::atomic::Ordering;
use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::state::AppState;
use crate::{watch, windows};

const TRAY_ID: &str = "tidy-tray";

/// Menu items whose text changes at runtime.
pub struct TrayItems {
    pending: MenuItem<Wry>,
    pause: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let search = MenuItem::with_id(app, "search", "검색 열기", true, None::<&str>)?;
    let pending = MenuItem::with_id(app, "pending", "대기 0건", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "일시정지", true, None::<&str>)?;
    let index = MenuItem::with_id(app, "index", "지금 인덱싱", true, None::<&str>)?;
    let report = MenuItem::with_id(app, "report", "리포트", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "설정", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &search, &pending, &sep, &pause, &index, &report, &settings, &sep2, &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Tidy")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "search" => windows::show_search(app),
            "pending" => windows::show_main(app, "pending"),
            "pause" => toggle_pause(app),
            "index" => watch::spawn_index(app),
            "report" => windows::show_main(app, "report"),
            "settings" => windows::show_main(app, "settings"),
            "quit" => {
                watch::signal_stop(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                windows::show_main(tray.app_handle(), "activity");
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(Mutex::new(TrayItems { pending, pause }));
    Ok(())
}

pub fn toggle_pause(app: &AppHandle) {
    let state = app.state::<AppState>();
    let paused = !state.paused.load(Ordering::Relaxed);
    set_paused(app, paused);
}

pub fn set_paused(app: &AppHandle, paused: bool) {
    let state = app.state::<AppState>();
    state.paused.store(paused, Ordering::Relaxed);
    if let Some(items) = app.try_state::<Mutex<TrayItems>>() {
        let items = items.lock().unwrap_or_else(|e| e.into_inner());
        let _ = items.pause.set_text(if paused {
            "감시 재개"
        } else {
            "일시정지"
        });
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(if paused {
            "Tidy (일시정지)"
        } else {
            "Tidy"
        }));
    }
    watch::restart(app);
}

pub fn set_pending(app: &AppHandle, count: u64) {
    if let Some(items) = app.try_state::<Mutex<TrayItems>>() {
        let items = items.lock().unwrap_or_else(|e| e.into_inner());
        let _ = items.pending.set_text(format!("대기 {count}건"));
    }
}
