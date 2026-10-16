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

use crate::commands;

pub const PORT: u16 = 1421;

const EVENTS: &[&str] = &[
    crate::watch::EV_ACTIVITY,
    crate::watch::EV_PENDING,
    crate::watch::EV_STATUS,
    crate::windows::EV_NAVIGATE,
    crate::windows::EV_FOCUS_SEARCH,
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
        "search" => ok(commands::search(app, arg(&args, "query")?, arg(&args, "limit")?).await?),
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
        "check_rules" => ok(commands::check_rules(arg(&args, "text")?).await?),
        "save_settings" => ok(commands::save_settings(app, arg(&args, "args")?).await?),
        other => Err(format!("unknown command {other}")),
    }
}
