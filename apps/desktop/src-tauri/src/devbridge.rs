//! Dev-only test bridge (debug builds only, never compiled into release).
//!
//! WebView2 does not accept synthetic input from automation tools, so GUI
//! tests cannot be driven inside the Tauri window. Instead the same React
//! bundle, opened in an ordinary browser at the Vite dev URL, talks to the
//! *running* app through this tiny HTTP server:
//!
//! * `POST /invoke/<command>`  body: JSON object with the same camelCase
//!   argument names the frontend passes to `invoke` → `{ok, value|error}`.
//! * `GET  /events?since=<n>`  long-polls the events the backend emitted
//!   (`tidy://activity`, `tidy://pending`, …) → `{next, events:[{name,payload}]}`.
//!
//! Bound to 127.0.0.1 only; the frontend selects this transport when
//! `window.__TAURI_INTERNALS__` is absent (see `src/ipc/transport.ts`).

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Listener};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::{about, commands, context_cmds, onboarding, overview, report_cmds, security_cmds};

pub const PORT: u16 = 1421;

const EVENTS: &[&str] = &[
    crate::watch::EV_ACTIVITY,
    crate::watch::EV_PENDING,
    crate::watch::EV_STATUS,
    crate::watch::EV_PROGRESS,
    crate::windows::EV_NAVIGATE,
    crate::windows::EV_FOCUS_SEARCH,
    crate::overview::EV_ORGANIZE,
];

#[derive(Default)]
struct EventLog {
    items: Vec<Value>,
}

type Shared = Arc<(Mutex<EventLog>, Condvar)>;

/// Start the bridge; failures (port in use) are logged and ignored so a
/// second dev instance still runs.
pub fn start(app: &AppHandle) {
    let server = match Server::http(("127.0.0.1", PORT)) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("dev bridge not started on 127.0.0.1:{PORT}: {e}");
            return;
        }
    };
    tracing::info!("dev bridge listening on http://127.0.0.1:{PORT}");

    let shared: Shared = Arc::new((Mutex::new(EventLog::default()), Condvar::new()));
    for name in EVENTS {
        let shared = Arc::clone(&shared);
        let name = (*name).to_string();
        app.listen_any(name.clone(), move |ev| {
            let payload: Value = serde_json::from_str(ev.payload()).unwrap_or(Value::Null);
            let (log, cv) = &*shared;
            log.lock()
                .unwrap_or_else(|e| e.into_inner())
                .items
                .push(json!({ "name": name, "payload": payload }));
            cv.notify_all();
        });
    }

    let app = app.clone();
    std::thread::Builder::new()
        .name("tidy-devbridge".into())
        .spawn(move || {
            for req in server.incoming_requests() {
                let app = app.clone();
                let shared = Arc::clone(&shared);
                // One thread per request so a long-poll never blocks invokes.
                std::thread::spawn(move || handle(req, &app, &shared));
            }
        })
        .expect("spawn dev bridge thread");
}

fn cors(mut r: Response<std::io::Cursor<Vec<u8>>>) -> Response<std::io::Cursor<Vec<u8>>> {
    for (k, v) in [
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Headers", "content-type"),
        ("Access-Control-Allow-Methods", "GET, POST, OPTIONS"),
        ("Content-Type", "application/json; charset=utf-8"),
    ] {
        r.add_header(Header::from_bytes(k, v).expect("static header"));
    }
    r
}

fn reply(req: Request, status: u16, body: Value) {
    let resp = cors(Response::from_string(body.to_string())).with_status_code(status);
    let _ = req.respond(resp);
}

fn handle(mut req: Request, app: &AppHandle, shared: &Shared) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));

    if *req.method() == Method::Options {
        return reply(req, 204, Value::Null);
    }

    if *req.method() == Method::Get && path == "/events" {
        let since: usize = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("since="))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let (log, cv) = &**shared;
        let mut guard = log.lock().unwrap_or_else(|e| e.into_inner());
        if guard.items.len() <= since {
            let (g, _) = cv
                .wait_timeout(guard, Duration::from_secs(10))
                .unwrap_or_else(|e| e.into_inner());
            guard = g;
        }
        let events: Vec<Value> = guard.items.iter().skip(since).cloned().collect();
        let next = guard.items.len();
        drop(guard);
        return reply(req, 200, json!({ "next": next, "events": events }));
    }

    if *req.method() == Method::Post {
        if let Some(cmd) = path.strip_prefix("/invoke/") {
            let mut body = String::new();
            if req.as_reader().read_to_string(&mut body).is_err() {
                return reply(req, 400, json!({ "ok": false, "error": "unreadable body" }));
            }
            let args: Value = if body.trim().is_empty() {
                json!({})
            } else {
                match serde_json::from_str(&body) {
                    Ok(v) => v,
                    Err(e) => {
                        return reply(req, 400, json!({ "ok": false, "error": e.to_string() }))
                    }
                }
            };
            let app = app.clone();
            let cmd = cmd.to_string();
            let result = tauri::async_runtime::block_on(dispatch(app, &cmd, args));
            return match result {
                Ok(v) => reply(req, 200, json!({ "ok": true, "value": v })),
                Err(e) => reply(req, 200, json!({ "ok": false, "error": e })),
            };
        }
    }

    reply(req, 404, json!({ "ok": false, "error": "not found" }));
}

fn arg<T: for<'de> Deserialize<'de>>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("argument `{key}`: {e}"))
}

fn ok<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Same functions the Tauri IPC handler calls, keyed by command name.
async fn dispatch(app: AppHandle, cmd: &str, args: Value) -> Result<Value, String> {
    match cmd {
        "get_status" => ok(commands::get_status(app).await?),
        "set_paused" => ok(commands::set_paused(app, arg(&args, "paused")?).await?),
        "index_now" => ok(commands::index_now(app).await?),
        "show_window" => ok(commands::show_window(app, arg(&args, "which")?).await?),
        "search" => ok(commands::search(
            app,
            arg(&args, "query")?,
            arg(&args, "limit")?,
            arg(&args, "customer")?,
        )
        .await?),
        // M7: semantic search.
        "search_semantic" => ok(commands::search_semantic(
            app,
            arg(&args, "query")?,
            arg(&args, "limit")?,
            arg(&args, "customer")?,
        )
        .await?),
        "semantic_status" => ok(commands::semantic_status(app).await?),
        // M15: text recognition.
        "ocr_status" => ok(crate::ocr::ocr_status(app).await?),
        "set_ocr_enabled" => ok(crate::ocr::set_ocr_enabled(app, arg(&args, "enabled")?).await?),
        "ocr_retry" => ok(crate::ocr::ocr_retry(app).await?),
        // M16: picture descriptions.
        "vision_status" => ok(crate::vision::vision_status(app).await?),
        "set_vision_settings" => ok(crate::vision::set_vision_settings(
            app,
            arg(&args, "enabled")?,
            arg(&args, "model")?,
            arg(&args, "dailyLimit")?,
        )
        .await?),
        "vision_retry" => ok(crate::vision::vision_retry(app).await?),
        // M8: wizard, rule editor, folder tree.
        "wizard_info" => ok(onboarding::wizard_info(app).await?),
        "preview_rules" => ok(onboarding::preview_rules(
            app,
            arg(&args, "watches")?,
            arg(&args, "cards")?,
            arg(&args, "exclusive")?,
        )
        .await?),
        "finish_onboarding" => ok(onboarding::finish_onboarding(app, arg(&args, "args")?).await?),
        "rules_editor" => {
            ok(onboarding::rules_editor(app, arg(&args, "text")?, arg(&args, "preview")?).await?)
        }
        "rules_from_cards" => ok(onboarding::rules_from_cards(
            app,
            arg(&args, "text")?,
            arg(&args, "cards")?,
            arg(&args, "preview")?,
        )
        .await?),
        "ai_rule_drafts" => ok(onboarding::ai_rule_drafts(app).await?),
        "fs_roots" => ok(onboarding::fs_roots(app).await?),
        "list_dirs" => ok(onboarding::list_dirs(arg(&args, "path")?).await?),
        "make_dir" => ok(onboarding::make_dir(arg(&args, "parent")?, arg(&args, "name")?).await?),
        "open_wizard" => ok(onboarding::open_wizard(app).await?),
        "open_path" => ok(commands::open_path(app, arg(&args, "path")?).await?),
        "reveal_path" => ok(commands::reveal_path(app, arg(&args, "path")?).await?),
        "list_pending" => ok(commands::list_pending(app).await?),
        "resolve_pending" => {
            ok(commands::resolve_pending(app, arg(&args, "id")?, arg(&args, "decision")?).await?)
        }
        "resolve_pending_batch" => {
            ok(
                commands::resolve_pending_batch(app, arg(&args, "ids")?, arg(&args, "decision")?)
                    .await?,
            )
        }
        "undo_last" => ok(commands::undo_last(app).await?),
        // A native folder dialog cannot be driven from a browser tab; the
        // settings view falls back to a typed path when this fails.
        "pick_folder" => {
            Err("dev bridge: 폴더 선택 창을 열 수 없습니다 — 경로를 직접 입력하세요".into())
        }
        "get_settings" => ok(commands::get_settings(app).await?),
        "list_ignored" => ok(commands::list_ignored(app).await?),
        "unignore" => ok(commands::unignore(app, arg(&args, "path")?).await?),
        "check_rules" => ok(commands::check_rules(arg(&args, "text")?).await?),
        "save_settings" => ok(commands::save_settings(app, arg(&args, "args")?).await?),
        // M6: network folder detection for "폴더 추가".
        "is_network_path" => ok(commands::is_network_path(arg(&args, "path")?).await?),
        // M4: AI settings and learner.
        "get_ai_settings" => ok(commands::get_ai_settings(app).await?),
        "save_ai_settings" => ok(commands::save_ai_settings(
            app,
            arg(&args, "settings")?,
            arg(&args, "apiKey")?,
            arg(&args, "embedApiKey")?,
        )
        .await?),
        "delete_api_key" => ok(commands::delete_api_key(app, arg(&args, "keyRef")?).await?),
        "test_ai" => ok(commands::test_ai(app).await?),
        "test_embed" => ok(commands::test_embed(app).await?),
        "api_key_stored" => ok(commands::api_key_stored(arg(&args, "keyRef")?).await?),
        "get_security_settings" => ok(security_cmds::get_security_settings(app).await?),
        "save_security_settings" => ok(security_cmds::save_security_settings(
            app,
            arg(&args, "settings")?,
            arg(&args, "password")?,
        )
        .await?),
        "open_vault_file" => ok(security_cmds::open_vault_file(app, arg(&args, "path")?).await?),
        "vault_copy" => ok(security_cmds::vault_copy(arg(&args, "path")?).await?),
        "list_learn" => ok(commands::list_learn(app).await?),
        "accept_learn" => ok(commands::accept_learn(app, arg(&args, "key")?).await?),
        "dismiss_learn" => ok(commands::dismiss_learn(app, arg(&args, "key")?).await?),
        // M5: report, restructure plans, move history.
        "get_curate_settings" => ok(context_cmds::get_curate_settings(app).await?),
        "save_curate_settings" => {
            ok(context_cmds::save_curate_settings(app, arg(&args, "settings")?).await?)
        }
        "curate_estimate" => ok(context_cmds::curate_estimate(app, arg(&args, "path")?).await?),
        "plan_context" => {
            ok(
                context_cmds::plan_context(app, arg(&args, "path")?, arg(&args, "protect")?)
                    .await?,
            )
        }
        "split_context_folder" => ok(context_cmds::split_context_folder(
            app,
            arg(&args, "id")?,
            arg(&args, "moveId")?,
            arg(&args, "selected")?,
        )
        .await?),
        "customer_overview" => ok(context_cmds::customer_overview(app).await?),
        "customer_names" => ok(context_cmds::customer_names(app).await?),
        "edit_context_move" => ok(context_cmds::edit_context_move(
            app,
            arg(&args, "id")?,
            arg(&args, "moveId")?,
            arg(&args, "edit")?,
            arg(&args, "selected")?,
        )
        .await?),
        "apply_context_suggestions" => {
            ok(context_cmds::apply_context_suggestions(app, arg(&args, "suggestions")?).await?)
        }
        "extract_context" => ok(context_cmds::extract_context(app).await?),
        "context_groups" => ok(context_cmds::context_groups(app).await?),
        "report_roots" => ok(report_cmds::report_roots(app).await?),
        "organize_target" => ok(report_cmds::organize_target(app, arg(&args, "path")?).await?),
        "build_report" => {
            ok(
                report_cmds::build_report(app, arg(&args, "path")?, arg(&args, "duplicates")?)
                    .await?,
            )
        }
        "plan_rules" => ok(report_cmds::plan_rules(
            app,
            arg(&args, "path")?,
            arg(&args, "recursive")?,
            arg(&args, "protect")?,
        )
        .await?),
        "plan_ai" => {
            ok(report_cmds::plan_ai(app, arg(&args, "path")?, arg(&args, "protect")?).await?)
        }
        "run_command" => ok(report_cmds::run_command(
            app,
            arg(&args, "text")?,
            arg(&args, "rulesOnly")?,
        )
        .await?),
        "preview_plan" => {
            ok(report_cmds::preview_plan(app, arg(&args, "id")?, arg(&args, "ids")?).await?)
        }
        "apply_plan" => ok(report_cmds::apply_plan(
            app,
            arg(&args, "id")?,
            arg(&args, "ids")?,
            arg(&args, "protect")?,
        )
        .await?),
        "list_plans" => ok(report_cmds::list_plans(app).await?),
        "open_plan" => ok(report_cmds::open_plan(app, arg(&args, "id")?).await?),
        "list_batches" => ok(report_cmds::list_batches(app, arg(&args, "limit")?).await?),
        "batch_moves" => ok(report_cmds::batch_moves(app, arg(&args, "batchId")?).await?),
        "undo_batch" => ok(report_cmds::undo_batch(app, arg(&args, "batchId")?).await?),
        "count_moves_since" => ok(report_cmds::count_moves_since(
            app,
            arg(&args, "sinceSecsAgo")?,
            arg(&args, "autoOnly")?,
        )
        .await?),
        "undo_since" => {
            ok(
                report_cmds::undo_since(app, arg(&args, "sinceSecsAgo")?, arg(&args, "autoOnly")?)
                    .await?,
            )
        }
        "trash_duplicates" => ok(report_cmds::trash_duplicates(app, arg(&args, "groups")?).await?),
        "open_trash" => ok(report_cmds::open_trash().await?),
        "today_moves" => ok(report_cmds::today_moves(
            app,
            arg(&args, "since")?,
            arg::<Option<usize>>(&args, "limit")?,
        )
        .await?),
        "watch_overview" => ok(overview::watch_overview(
            app,
            arg::<Option<bool>>(&args, "refresh")?.unwrap_or(false),
        )
        .await?),
        "about_info" => ok(about::about_info(app).await?),
        "open_url" => ok(about::open_url(app, arg(&args, "url")?).await?),
        "open_folder" => ok(about::open_folder(app, arg(&args, "path")?).await?),
        "check_update" => ok(about::check_update(app).await?),
        "install_update" => ok(about::install_update(app).await?),
        // Test-only: what open_url / open_folder recorded (TIDY_E2E_NO_OPEN).
        "debug_opened" => ok(about::opened()),
        other => Err(format!("unknown command {other}")),
    }
}
