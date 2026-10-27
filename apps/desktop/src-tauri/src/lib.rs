//! Tidy desktop app (SPEC §9, M3): tray-resident, global-hotkey search
//! window, confirm popup, pending queue and settings. All logic lives in
//! `tidy-core`; this crate only wires it to windows, tray and IPC.

mod about;
mod autostart;
mod commands;
mod context_cmds;
#[cfg(debug_assertions)]
mod devbridge;
mod onboarding;
mod overview;
mod report_cmds;
mod security_cmds;
mod semantic;
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
    // Console + `<local data>/tidy/logs/tidy.log` (M11: "로그 폴더 열기").
    {
        use tracing_subscriber::fmt::writer::MakeWriterExt;
        let base = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false);
        match about::open_log_file() {
            Some(f) => base
                .with_ansi(false)
                .with_writer(std::io::stdout.and(std::sync::Mutex::new(f)))
                .init(),
            None => base.init(),
        }
    }

    let config_path = state::config_path_from_env();
    tracing::info!(config = %config_path.display(), "starting");

    tauri::Builder::default()
        // Register state before any window exists: Tauri 2 creates the
        // windows from tauri.conf.json *before* `setup` runs, so the webview
        // can invoke commands while setup is still executing. Managing the
        // state here (not in setup) avoids "state() called before manage()".
        .manage(AppState::new(config_path.clone()))
        // A second launch just brings the running instance forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_start(app);
        }))
        .plugin(autostart::plugin())
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
            watch::init(&handle);
            tray::create(&handle)?;
            let hotkey = handle.state::<AppState>().config().general.hotkey;
            windows::register_hotkey(&handle, &hotkey);
            let cfg = handle.state::<AppState>().config();
            // First run = setup not finished ([general] onboarded), not "no
            // config file": an installer or another feature may have
            // written one (SPEC §14.1).
            let first_run = !cfg.onboarded();
            if !first_run {
                // Keep the OS login item in line with the config (it may have
                // been edited by hand, or the app moved).
                let auto = autostart::sync(&handle, cfg.general.start_on_login);
                if let Some(e) = auto.error {
                    handle.state::<AppState>().set_error(e);
                }
            }
            watch::restart(&handle);
            watch::spawn_index(&handle); // starts embedding when done
            watch::spawn_content_window_timer(&handle);
            security_cmds::spawn_open_sweeper(&handle);
            about::spawn_daily_check(&handle);
            watch::pending_changed(&handle, true);
            #[cfg(debug_assertions)]
            devbridge::start(&handle);
            if first_run {
                // Nothing is watched (or moved) until the wizard is done.
                windows::show_wizard(&handle);
            } else if autostart::launched_at_login() {
                // Login start: tray only, never a window.
            } else if cfg!(debug_assertions) {
                // Dev builds (`tauri dev`) open the main window right away so a
                // fresh process is never confused with a stale one still
                // sitting in the tray; release builds start tray-only.
                windows::show_main(&handle, "activity");
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
            commands::list_ignored,
            commands::unignore,
            commands::show_window,
            commands::search,
            commands::search_semantic,
            onboarding::wizard_info,
            onboarding::preview_rules,
            onboarding::finish_onboarding,
            onboarding::rules_editor,
            onboarding::rules_from_cards,
            onboarding::ai_rule_drafts,
            onboarding::fs_roots,
            onboarding::list_dirs,
            onboarding::make_dir,
            onboarding::open_wizard,
            commands::semantic_status,
            commands::open_path,
            commands::reveal_path,
            commands::list_pending,
            commands::resolve_pending,
            commands::resolve_pending_batch,
            commands::undo_last,
            commands::pick_folder,
            commands::is_network_path,
            commands::get_settings,
            commands::check_rules,
            commands::save_settings,
            commands::get_ai_settings,
            commands::save_ai_settings,
            commands::delete_api_key,
            commands::test_ai,
            security_cmds::get_security_settings,
            security_cmds::save_security_settings,
            security_cmds::open_vault_file,
            security_cmds::vault_copy,
            commands::list_learn,
            commands::accept_learn,
            commands::dismiss_learn,
            context_cmds::get_curate_settings,
            context_cmds::save_curate_settings,
            context_cmds::curate_estimate,
            context_cmds::plan_context,
            context_cmds::edit_context_move,
            context_cmds::apply_context_suggestions,
            context_cmds::extract_context,
            context_cmds::context_groups,
            context_cmds::split_context_folder,
            context_cmds::customer_overview,
            context_cmds::customer_names,
            report_cmds::report_roots,
            report_cmds::organize_target,
            report_cmds::build_report,
            report_cmds::plan_rules,
            report_cmds::plan_ai,
            report_cmds::preview_plan,
            report_cmds::apply_plan,
            report_cmds::list_plans,
            report_cmds::open_plan,
            report_cmds::list_batches,
            report_cmds::batch_moves,
            report_cmds::undo_batch,
            report_cmds::count_moves_since,
            report_cmds::undo_since,
            report_cmds::today_moves,
            report_cmds::trash_duplicates,
            report_cmds::open_trash,
            overview::watch_overview,
            about::about_info,
            about::open_url,
            about::open_folder,
            about::check_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tidy");
}
