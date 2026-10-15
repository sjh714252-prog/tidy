//! Tidy desktop app (SPEC §9, M3): tray-resident, global-hotkey search
//! window, confirm popup, pending queue and settings. All logic lives in
//! `tidy-core`; this crate only wires it to windows, tray and IPC.

mod commands;
mod state;
mod tray;
mod watch;
mod windows;

use tauri::{Manager, WindowEvent};
use tauri_plugin_global_shortcut::ShortcutState;

use state::AppState;

pub fn run() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let config_path = state::config_path_from_env();
    tracing::info!(config = %config_path.display(), "starting");

    tauri::Builder::default()
        // A second launch just brings the running instance forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_main(app, "activity");
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        windows::toggle_search(app);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            let handle = app.handle().clone();
            let first_run = !config_path.exists();
            app.manage(AppState::new(config_path.clone()));
            tray::create(&handle)?;
            let hotkey = handle.state::<AppState>().config().general.hotkey;
            windows::register_hotkey(&handle, &hotkey);
            watch::restart(&handle);
            watch::spawn_index(&handle);
            watch::pending_changed(&handle, true);
            if first_run {
                // Nothing is watched until folders are configured.
                windows::show_main(&handle, "settings");
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Windows only hide; the app lives in the tray until "종료".
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The search window behaves like a launcher: gone on blur.
            WindowEvent::Focused(false) if window.label() == "search" => {
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::set_paused,
            commands::index_now,
            commands::show_window,
            commands::search,
            commands::open_path,
            commands::reveal_path,
            commands::list_pending,
            commands::resolve_pending,
            commands::undo_last,
            commands::pick_folder,
            commands::get_settings,
            commands::check_rules,
            commands::save_settings,
            commands::get_ai_settings,
            commands::save_ai_settings,
            commands::delete_api_key,
            commands::test_ai,
            commands::list_learn,
            commands::accept_learn,
            commands::dismiss_learn,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tidy");
}
