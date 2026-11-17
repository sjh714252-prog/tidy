//! Tray icon + menu (SPEC §9.1, §14.6): 검색 열기 / 확인 대기 N건 / 일시정지 /
//! 지금 인덱싱 / 정리하기 / 설정 / 정보… / 종료.

use std::sync::atomic::Ordering;
use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::state::AppState;
use crate::{watch, windows};

const TRAY_ID: &str = "tidy-tray";

/// Menu items whose text changes at runtime.
#[derive(Clone)]
pub struct TrayItems {
    pending: MenuItem<Wry>,
    pause: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let search = MenuItem::with_id(app, "search", "검색 열기", true, None::<&str>)?;
    let pending = MenuItem::with_id(app, "pending", "확인 대기 0건", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "일시정지", true, None::<&str>)?;
    let index = MenuItem::with_id(app, "index", "지금 인덱싱", true, None::<&str>)?;
    let report = MenuItem::with_id(app, "report", "정리하기", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "설정", true, None::<&str>)?;
    let about = MenuItem::with_id(app, "about", "정보…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &search, &pending, &sep, &pause, &index, &report, &settings, &about, &sep2, &quit,
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
            "about" => windows::show_main(app, "activity?about=1"),
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
                windows::show_start(tray.app_handle());
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
    let pause = items(app).map(|i| i.pause);
    let tray = app.tray_by_id(TRAY_ID);
    on_main(app, move || {
        if let Some(p) = pause {
            let _ = p.set_text(if paused {
                "감시 재개"
            } else {
                "일시정지"
            });
        }
        if let Some(t) = tray {
            let _ = t.set_tooltip(Some(if paused {
                "Tidy (일시정지)"
            } else {
                "Tidy"
            }));
        }
    });
    watch::restart(app);
}

pub fn set_pending(app: &AppHandle, count: u64) {
    if let Some(i) = items(app) {
        on_main(app, move || {
            let _ = i.pending.set_text(format!("확인 대기 {count}건"));
        });
    }
}

/// A copy of the menu handles; the lock is never held while the menu is
/// changed (0.18.5: holding it while another thread waited for the main
/// thread froze the tray — no left click, no right-click menu).
fn items(app: &AppHandle) -> Option<TrayItems> {
    let items = app.try_state::<Mutex<TrayItems>>()?;
    let items = items.lock().unwrap_or_else(|e| e.into_inner());
    Some(items.clone())
}

/// Run a tray/menu change on the main thread without waiting for it. Off
/// the main thread, Tauri's menu setters block until the main thread runs
/// them; a caller that holds a lock (or the main thread itself, still in
/// `setup`) could then wait on each other forever.
pub fn on_main(app: &AppHandle, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = app.run_on_main_thread(f) {
        tracing::warn!("tray update not sent: {e}");
    }
}
