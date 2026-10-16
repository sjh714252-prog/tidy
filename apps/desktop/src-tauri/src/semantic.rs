//! Semantic search in the app (SPEC §5.6, M7): the `SemanticSearch` of the
//! current config with its own DB connection (a query may wait on the
//! embedding model for up to `search.query_timeout_ms`; the UI connection
//! is never held that long), and the background embedding thread.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tidy_core::ai::AiError;
use tidy_core::search::SemanticSearch;
use tidy_core::service::run_embed_loop;
use tidy_core::store::Store;
use tidy_core::Config;

use crate::state::AppState;
use crate::watch::{emit_activity, EV_STATUS};

pub struct Semantic {
    pub sem: SemanticSearch,
    pub store: Mutex<Store>,
}

/// What the app knows about semantic search for the current config.
#[derive(Default)]
pub struct Slot {
    pub handle: Option<Arc<Semantic>>,
    /// Why there is none (AI off / no embed model / Anthropic / no key…).
    pub off: Option<String>,
    /// Stop flag of the running embedding thread.
    pub stop: Option<Arc<AtomicBool>>,
}

/// Seconds between embedding passes once the backlog is done.
const EMBED_INTERVAL: Duration = Duration::from_secs(30);

pub fn build(cfg: &Config) -> (Option<Arc<Semantic>>, Option<String>) {
    match SemanticSearch::from_config(cfg) {
        Ok(sem) => match Store::open(&cfg.store.db_path()) {
            Ok(store) => (
                Some(Arc::new(Semantic {
                    sem,
                    store: Mutex::new(store),
                })),
                None,
            ),
            Err(e) => (None, Some(e.to_string())),
        },
        Err(AiError::Disabled) => (None, Some("AI가 꺼져 있습니다".into())),
        Err(AiError::NoEmbedModel(_)) => (None, Some("임베딩 모델이 설정되지 않았습니다".into())),
        Err(e) => (None, Some(e.to_string())),
    }
}

pub fn handle(app: &AppHandle) -> Option<Arc<Semantic>> {
    app.state::<AppState>()
        .semantic
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .handle
        .clone()
}

/// (Re)start background embedding for the current handle. The old thread
/// is only told to stop: it may be inside a request and ends on its own.
pub fn restart_embed(app: &AppHandle) {
    let state = app.state::<AppState>();
    let cfg = state.config();
    let mut slot = state.semantic.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(stop) = slot.stop.take() {
        stop.store(true, Ordering::Relaxed);
    }
    let Some(h) = slot.handle.clone() else {
        return;
    };
    let stop = Arc::new(AtomicBool::new(false));
    slot.stop = Some(Arc::clone(&stop));
    drop(slot);
    let app = app.clone();
    std::thread::Builder::new()
        .name("tidy-embed".into())
        .spawn(move || {
            let res = run_embed_loop(&cfg, &h.sem, &stop, EMBED_INTERVAL, |r| {
                match &r.error {
                    Some(e) => emit_activity(
                        &app,
                        "warning",
                        Path::new(""),
                        format!("의미검색 임베딩 중단 (단어 검색은 그대로 동작): {e}"),
                    ),
                    None => emit_activity(
                        &app,
                        "info",
                        Path::new(""),
                        format!(
                            "의미검색 임베딩: 새로 {} · 재사용 {} · 남음 {}",
                            r.embedded, r.reused, r.remaining
                        ),
                    ),
                }
                let _ = app.emit(EV_STATUS, ());
            });
            if let Err(e) = res {
                tracing::warn!("embedding loop: {e}");
            }
        })
        .expect("spawn embed thread");
}
