//! (M16) Picture descriptions in the app (SPEC §14.13): the background
//! thread and the settings card's commands.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tidy_core::index::vision;
use tidy_core::service::run_vision_loop;
use tidy_core::store::Store;

use crate::commands::{blocking, CmdResult};
use crate::state::AppState;
use crate::watch::{emit_activity, EV_STATUS};

/// Seconds between passes once the queue is empty.
const VISION_INTERVAL: Duration = Duration::from_secs(30);

/// Stop flag of the running thread.
static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// (Re)start background descriptions for the current config (stops the
/// old thread; it ends after the picture it is sending).
pub fn restart(app: &AppHandle) {
    let cfg = app.state::<AppState>().config();
    let mut slot = STOP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(stop) = slot.take() {
        stop.store(true, Ordering::Relaxed);
    }
    if !cfg.vision.enabled {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    *slot = Some(Arc::clone(&stop));
    drop(slot);
    let app = app.clone();
    std::thread::Builder::new()
        .name("tidy-vision".into())
        .spawn(move || {
            let res = run_vision_loop(&cfg, &stop, VISION_INTERVAL, |r| {
                match r {
                    Ok(r) => {
                        let mut msg = format!(
                            "사진 내용 인식: 설명 붙임 {} · 설명할 것 없음 {} · 실패 {} · 남음 {}",
                            r.described, r.empty, r.failed, r.remaining
                        );
                        if r.skipped > 0 {
                            msg.push_str(&format!(" · 개인정보 의심이라 안 보냄 {}", r.skipped));
                        }
                        if r.limited {
                            msg.push_str(" · 오늘 한도에 닿아 나머지는 내일");
                        }
                        emit_activity(&app, "info", Path::new(""), msg)
                    }
                    Err(e) => emit_activity(
                        &app,
                        "warning",
                        Path::new(""),
                        format!("사진 내용 인식을 잠시 쓸 수 없음 (검색·정리는 그대로 동작): {e}"),
                    ),
                }
                let _ = app.emit(EV_STATUS, ());
            });
            if let Err(e) = res {
                tracing::warn!("vision loop: {e}");
            }
        })
        .expect("spawn vision thread");
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionStatusDto {
    pub enabled: bool,
    /// "anthropic · claude-haiku-4-5"; `None` = no AI model set up.
    pub model: Option<String>,
    /// `[vision] model` as written ("" = the classify model).
    pub model_override: String,
    /// The pictures stay on this PC (Ollama).
    pub local: bool,
    pub note: Option<String>,
    pub pending: u64,
    pub ok: u64,
    pub empty: u64,
    pub failed: u64,
    pub skipped: u64,
    pub today: u64,
    pub daily_limit: u32,
    pub total: u64,
    pub cost_usd: f64,
}

fn status_of(app: &AppHandle) -> CmdResult<VisionStatusDto> {
    let cfg = app.state::<AppState>().config();
    let store = Store::open(&cfg.store.db_path()).map_err(|e| e.to_string())?;
    let st = vision::status(&cfg, &store).map_err(|e| e.to_string())?;
    Ok(VisionStatusDto {
        enabled: st.enabled,
        model: st.model,
        model_override: cfg.vision.model.clone().unwrap_or_default(),
        local: st.local,
        note: st.note,
        pending: st.counts.pending,
        ok: st.counts.ok,
        empty: st.counts.empty,
        failed: st.counts.failed,
        skipped: st.counts.skipped,
        today: st.today,
        daily_limit: st.daily_limit,
        total: st.total,
        cost_usd: st.cost_usd,
    })
}

#[tauri::command]
pub async fn vision_status(app: AppHandle) -> CmdResult<VisionStatusDto> {
    blocking(move || status_of(&app)).await
}

/// The settings card: switch, model ("" = classify model) and daily
/// limit, written to `[vision]` right away. The watch loop restarts so new
/// pictures are classified with (or without) their description.
#[tauri::command]
pub async fn set_vision_settings(
    app: AppHandle,
    enabled: bool,
    model: Option<String>,
    daily_limit: Option<u32>,
) -> CmdResult<VisionStatusDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        tidy_core::config::write_vision_settings(
            &state.config_path,
            enabled,
            model.as_deref(),
            daily_limit.map(|n| n.clamp(1, 100_000)),
        )
        .map_err(|e| e.to_string())?;
        state.reload();
        crate::watch::restart(&app);
        restart(&app);
        status_of(&app)
    })
    .await
}

/// Failed pictures go back in the queue.
#[tauri::command]
pub async fn vision_retry(app: AppHandle) -> CmdResult<VisionStatusDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(|e| e.to_string())?;
        store.vision_retry_failed().map_err(|e| e.to_string())?;
        restart(&app);
        status_of(&app)
    })
    .await
}
