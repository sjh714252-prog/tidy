//! (M15) Text recognition of images and scanned PDFs in the app (SPEC
//! §14.11): the background thread and the settings card's commands.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tidy_core::index::ocr;
use tidy_core::service::run_ocr_loop;
use tidy_core::store::Store;

use crate::commands::{blocking, CmdResult};
use crate::state::AppState;
use crate::watch::{emit_activity, EV_STATUS};

/// Seconds between passes once the queue is empty (new files are picked
/// up within this time).
const OCR_INTERVAL: Duration = Duration::from_secs(20);

/// Stop flag of the running recognition thread.
static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// (Re)start background recognition for the current config (stops the
/// old thread; it ends after the file it is reading).
pub fn restart(app: &AppHandle) {
    let cfg = app.state::<AppState>().config();
    let mut slot = STOP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(stop) = slot.take() {
        stop.store(true, Ordering::Relaxed);
    }
    if !cfg.ocr.enabled {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    *slot = Some(Arc::clone(&stop));
    drop(slot);
    let app = app.clone();
    std::thread::Builder::new()
        .name("tidy-ocr".into())
        .spawn(move || {
            let res = run_ocr_loop(&cfg, &stop, OCR_INTERVAL, |r| {
                match r {
                    Ok(r) => emit_activity(
                        &app,
                        "info",
                        Path::new(""),
                        format!(
                            "사진·스캔 PDF 글자 인식: 글자 찾음 {} · 글자 없음 {} · 실패 {} · 남음 {}",
                            r.recognized, r.empty, r.failed, r.remaining
                        ),
                    ),
                    Err(e) => emit_activity(
                        &app,
                        "warning",
                        Path::new(""),
                        format!("사진·스캔 PDF 글자 인식을 쓸 수 없음 (검색은 그대로 동작): {e}"),
                    ),
                }
                let _ = app.emit(EV_STATUS, ());
            });
            if let Err(e) = res {
                tracing::warn!("ocr loop: {e}");
            }
        })
        .expect("spawn ocr thread");
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrStatusDto {
    pub enabled: bool,
    /// "Windows 내장 OCR · 한국어"; `None` = no engine.
    pub engine: Option<String>,
    pub note: Option<String>,
    pub pending: u64,
    pub ok: u64,
    pub empty: u64,
    pub failed: u64,
}

fn status_of(app: &AppHandle) -> CmdResult<OcrStatusDto> {
    let cfg = app.state::<AppState>().config();
    let store = Store::open(&cfg.store.db_path()).map_err(|e| e.to_string())?;
    let st = ocr::status(&cfg, &store).map_err(|e| e.to_string())?;
    Ok(OcrStatusDto {
        enabled: st.enabled,
        engine: st.engine,
        note: st.note,
        pending: st.counts.pending,
        ok: st.counts.ok,
        empty: st.counts.empty,
        failed: st.counts.failed,
    })
}

#[tauri::command]
pub async fn ocr_status(app: AppHandle) -> CmdResult<OcrStatusDto> {
    blocking(move || status_of(&app)).await
}

/// The settings card's switch: written to `[ocr] enabled` right away.
#[tauri::command]
pub async fn set_ocr_enabled(app: AppHandle, enabled: bool) -> CmdResult<OcrStatusDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        tidy_core::config::write_ocr_enabled(&state.config_path, enabled)
            .map_err(|e| e.to_string())?;
        state.reload();
        restart(&app);
        status_of(&app)
    })
    .await
}

/// Failed files go back in the queue.
#[tauri::command]
pub async fn ocr_retry(app: AppHandle) -> CmdResult<OcrStatusDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(|e| e.to_string())?;
        store.ocr_retry_failed().map_err(|e| e.to_string())?;
        restart(&app);
        status_of(&app)
    })
    .await
}
