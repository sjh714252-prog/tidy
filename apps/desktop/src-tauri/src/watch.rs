//! Background work: the watch loop (tidy_core::service::run_watch) and the
//! startup/initial index of all configured roots.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tidy_core::config::RootKind;
use tidy_core::engine::Action;
use tidy_core::index::{jobs_from_config, Indexer};
use tidy_core::mover::{ProgressFn, TransferProgress};
use tidy_core::service::{run_watch, WatchEvent, WatchRunOptions};
use tidy_core::store::Store;

use crate::state::{AppState, WatchHandle};
use crate::{tray, windows};

pub const EV_ACTIVITY: &str = "tidy://activity";
pub const EV_PENDING: &str = "tidy://pending";
pub const EV_STATUS: &str = "tidy://status";
/// Progress of a long copy-verify move (NAS), payload `TransferProgress`.
pub const EV_PROGRESS: &str = "tidy://progress";

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Remember the app handle for callbacks that run on core threads.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// Progress callback for every mover in the app (watch loop and UI
/// actions): forwards to the windows as `EV_PROGRESS`.
pub fn progress_fn() -> ProgressFn {
    Arc::new(|p: &TransferProgress| {
        if let Some(app) = APP.get() {
            let _ = app.emit(EV_PROGRESS, p);
        }
    })
}

/// One line of the activity feed (main window) — what happened to a file.
#[derive(Debug, Clone, Serialize)]
pub struct Activity {
    /// moved | pending | duplicate | no_rule | skipped | error | info
    pub kind: &'static str,
    pub path: String,
    pub detail: String,
    pub at: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn pct(c: Option<f64>) -> String {
    c.map(|c| format!(" {:.0}%", c * 100.0)).unwrap_or_default()
}

pub fn emit_activity(app: &AppHandle, kind: &'static str, path: &Path, detail: String) {
    let _ = app.emit(
        EV_ACTIVITY,
        Activity {
            kind,
            path: path.display().to_string(),
            detail,
            at: now(),
        },
    );
}

/// Pending count changed: update tray + windows; pop the confirm window up
/// unless quiet mode is on.
pub fn pending_changed(app: &AppHandle, popup: bool) {
    let state = app.state::<AppState>();
    let (count, quiet) = {
        let inner = state.lock_inner();
        let count = inner
            .mover
            .as_ref()
            .and_then(|m| m.store().pending_count().ok())
            .unwrap_or(0);
        (count, inner.cfg.general.quiet_mode)
    };
    tray::set_pending(app, count);
    let _ = app.emit(EV_PENDING, count);
    if popup && !quiet && count > 0 {
        windows::show_confirm(app);
    }
}

/// Serializes restarts so two watch loops never run at once.
static RESTART_LOCK: Mutex<()> = Mutex::new(());

/// Ask the watch loop to stop without waiting (used on quit).
pub fn signal_stop(app: &AppHandle) {
    let state = app.state::<AppState>();
    let guard = state.watch.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = guard.as_ref() {
        h.stop.store(true, Ordering::Relaxed);
    }
}

/// Stop the current loop (if any), wait for it, then start a new one with
/// the current config (unless paused). Runs on a helper thread: callers are
/// often the main thread (tray menu), and the watch thread itself calls
/// main-thread APIs (tray text, window show), so joining it from the main
/// thread could deadlock.
pub fn restart(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = RESTART_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        stop_and_wait(&app);
        start(&app);
    });
}

fn stop_and_wait(app: &AppHandle) {
    let state = app.state::<AppState>();
    let handle = state.watch.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(h) = handle {
        h.stop.store(true, Ordering::Relaxed);
        let _ = h.thread.join();
    }
}

fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    *state.ai_state.lock().unwrap_or_else(|e| e.into_inner()) = None;
    state
        .offline
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    if state.is_paused() {
        let _ = app.emit(EV_STATUS, ());
        return;
    }
    let cfg = state.config();
    if cfg.watches.is_empty() {
        let _ = app.emit(EV_STATUS, ());
        return;
    }
    let stop_flag = Arc::new(AtomicBool::new(false));
    let expected = Arc::clone(&state.expected);
    let app2 = app.clone();
    let flag = Arc::clone(&stop_flag);
    let thread = std::thread::Builder::new()
        .name("tidy-watch".into())
        .spawn(move || {
            let opts = WatchRunOptions {
                index: true,
                ai: true,
                progress: Some(progress_fn()),
                ..Default::default()
            };
            let res = run_watch(&cfg, &opts, expected, &flag, |ev| on_event(&app2, ev));
            let state = app2.state::<AppState>();
            match res {
                Ok(()) => tracing::info!("watch loop stopped"),
                Err(e) => {
                    state.set_error(format!("감시를 시작할 수 없습니다: {e}"));
                    emit_activity(&app2, "error", Path::new(""), e.to_string());
                }
            }
            let _ = app2.emit(EV_STATUS, ());
        })
        .expect("spawn watch thread");
    *state.watch.lock().unwrap_or_else(|e| e.into_inner()) = Some(WatchHandle {
        stop: stop_flag,
        thread,
    });
}

fn on_event(app: &AppHandle, ev: WatchEvent<'_>) {
    match ev {
        WatchEvent::Warning(w) => emit_activity(app, "warning", Path::new(""), w),
        WatchEvent::Root(r) => {
            {
                let state = app.state::<AppState>();
                let mut offline = state.offline.lock().unwrap_or_else(|e| e.into_inner());
                offline.retain(|p| p != &r.path);
                if !r.online {
                    offline.push(r.path.clone());
                }
            }
            let secs = tidy_core::watcher::RECONNECT_INTERVAL.as_secs();
            if r.online {
                emit_activity(
                    app,
                    "info",
                    &r.path,
                    format!(
                        "다시 연결됨 — 끊긴 동안 바뀐 파일을 처리합니다{}",
                        if r.events { "" } else { " (폴링만 사용)" }
                    ),
                );
            } else {
                emit_activity(
                    app,
                    "warning",
                    &r.path,
                    format!("연결 끊김 — 이 폴더만 일시정지하고 {secs}초마다 다시 확인합니다"),
                );
            }
            let _ = app.emit(EV_STATUS, ());
        }
        WatchEvent::Recovered(r) => {
            for p in &r.completed {
                emit_activity(app, "info", p, "중단됐던 이동을 마저 끝냈습니다".into());
            }
            for p in &r.rolled_back {
                emit_activity(
                    app,
                    "info",
                    p,
                    "중단됐던 이동을 취소했습니다 (파일은 원래 자리)".into(),
                );
            }
            for a in &r.attention {
                emit_activity(app, "warning", Path::new(""), format!("확인 필요: {a}"));
            }
            if r.unresolved > 0 {
                emit_activity(
                    app,
                    "warning",
                    Path::new(""),
                    format!(
                        "중단된 이동 {}건은 NAS가 다시 연결되면 정리합니다",
                        r.unresolved
                    ),
                );
            }
        }
        WatchEvent::Started(roots) => {
            app.state::<AppState>().clear_error();
            for r in roots {
                emit_activity(app, "info", r, "감시 시작".into());
            }
            let _ = app.emit(EV_STATUS, ());
        }
        WatchEvent::Ai(state) => {
            let detail = match &state {
                Ok(m) => format!("AI 사용: {m}"),
                Err(e) => format!("AI 꺼짐 (규칙만 사용): {e}"),
            };
            *app.state::<AppState>()
                .ai_state
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(state);
            emit_activity(app, "info", Path::new(""), detail);
        }
        WatchEvent::Handled(path, res) => match res {
            Ok(Action::Moved {
                dst,
                rule,
                confidence,
                ..
            }) => emit_activity(
                app,
                "moved",
                path,
                format!("→ {} [{rule}{}]", dst.display(), pct(*confidence)),
            ),
            Ok(Action::Pending {
                suggested,
                rule,
                confidence,
                reason,
                ..
            }) => {
                emit_activity(
                    app,
                    "pending",
                    path,
                    format!(
                        "확인 필요 → {} [{rule}{}]{}",
                        suggested.display(),
                        pct(*confidence),
                        reason
                            .as_deref()
                            .map(|r| format!(" {r}"))
                            .unwrap_or_default()
                    ),
                );
                pending_changed(app, true);
            }
            Ok(Action::AiFailed { error, .. }) => emit_activity(
                app,
                "no_rule",
                path,
                format!("맞는 규칙 없음 · AI 실패: {error}"),
            ),
            Ok(Action::Duplicate { existing, .. }) => emit_activity(
                app,
                "duplicate",
                path,
                format!("동일한 파일이 이미 있음: {}", existing.display()),
            ),
            Ok(Action::NoRule(_)) => emit_activity(app, "no_rule", path, "맞는 규칙 없음".into()),
            Ok(Action::Skipped { reason, .. }) => {
                emit_activity(app, "skipped", path, (*reason).to_string())
            }
            Ok(Action::AlreadyInPlace { .. } | Action::WouldMove { .. }) => {}
            Err(e) => emit_activity(app, "error", path, e.to_string()),
        },
    }
}

/// Incrementally index every configured root on a background thread
/// (startup, settings change, tray "지금 인덱싱"). Unchanged files cost
/// one stat each. Uses its own DB connection and small write batches so the
/// watch loop is never blocked for long.
pub fn spawn_index(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.indexing.swap(true, Ordering::SeqCst) {
        return; // already running
    }
    let cfg = state.config();
    let app = app.clone();
    std::thread::Builder::new()
        .name("tidy-index".into())
        .spawn(move || {
            let _ = app.emit(EV_STATUS, ());
            let result = (|| -> Result<(u64, u64), String> {
                let store = Store::open(&cfg.store.db_path()).map_err(|e| e.to_string())?;
                let indexer = Indexer::new(&store);
                // Background run: NAS folders follow their content window.
                let jobs = jobs_from_config(&cfg, false).into_iter().map(|(p, mut o)| {
                    o.batch_size = 50;
                    (p, o)
                });
                let (mut files, mut changed) = (0, 0);
                for (path, opts) in jobs {
                    if !path.is_dir() {
                        continue;
                    }
                    match indexer.index_root(&path, &opts, |_, _| {}) {
                        Ok(r) => {
                            files += r.scanned;
                            changed += r.added + r.updated + r.removed;
                        }
                        Err(e) => tracing::warn!("index {}: {e}", path.display()),
                    }
                }
                Ok((files, changed))
            })();
            match result {
                Ok((files, changed)) => emit_activity(
                    &app,
                    "info",
                    Path::new(""),
                    format!("인덱싱 완료: {files}개 파일 (변경 {changed})"),
                ),
                Err(e) => emit_activity(&app, "error", Path::new(""), format!("인덱싱 실패: {e}")),
            }
            app.state::<AppState>()
                .indexing
                .store(false, Ordering::SeqCst);
            let _ = app.emit(EV_STATUS, ());
        })
        .expect("spawn index thread");
}

/// NAS folders with a `content_index_window` get their body text indexed
/// inside that window: re-run the (incremental) background index hourly
/// while such a folder is configured.
pub fn spawn_content_window_timer(app: &AppHandle) {
    let app = app.clone();
    std::thread::Builder::new()
        .name("tidy-index-timer".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(3600));
            let cfg = app.state::<AppState>().config();
            let windowed = cfg.watches.iter().any(|w| {
                w.kind == RootKind::Network && w.index_content && w.content_index_window.is_some()
            });
            if windowed {
                spawn_index(&app);
            }
        })
        .expect("spawn index timer thread");
}
