//! Tauri commands (IPC). Every command is async and does its work on the
//! blocking pool (CLAUDE.md: heavy IO via spawn_blocking), so the UI thread
//! never waits on SQLite or the filesystem. Types mirror `src/ipc/types.ts`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tidy_core::classify::{learner, AiClassifier, RuleSet};
use tidy_core::config::{
    write_ai_settings, write_settings, AiPatch, ProviderConfig, ProviderKind, RootKind,
    SettingsPatch, TimeWindow, WatchRoot,
};
use tidy_core::engine::{new_batch_id, reindex_undo};
use tidy_core::index::IndexOptions;
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::search::{
    hybrid_search, search as fts_search, MatchedIn, Query, SearchHit, SearchMode, SearchOptions,
};
use tidy_core::store::PendingSource;

use crate::state::AppState;
use crate::{autostart, semantic, tray, watch, windows};

type CmdResult<T> = Result<T, String>;

/// Highlight markers (Unicode private use area): the frontend splits on
/// them and renders <mark> without ever using innerHTML.
const MARK_START: &str = "\u{E000}";
const MARK_END: &str = "\u{E001}";

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn name_and_dir(p: &Path) -> (String, String) {
    (
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        p.parent().map(s).unwrap_or_default(),
    )
}

// ── status ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub config_path: String,
    pub config_exists: bool,
    pub rules_path: String,
    pub rules_exists: bool,
    pub db_path: String,
    pub watching: Vec<String>,
    pub paused: bool,
    pub indexing: bool,
    pub pending_count: u64,
    pub files_indexed: u64,
    pub last_error: Option<String>,
    pub hotkey: String,
    /// "off" (not enabled) | "ready" | "unavailable" | "starting"
    pub ai_state: &'static str,
    /// Provider / model when ready, the reason otherwise.
    pub ai_detail: Option<String>,
    /// Network watch folders that are unreachable right now (paused).
    pub offline: Vec<String>,
    /// Semantic search: "off" | "ready" | "unavailable" (repeated failures).
    pub semantic_state: &'static str,
    /// Model when on, why not otherwise.
    pub semantic_detail: Option<String>,
}

fn status(app: &AppHandle) -> StatusDto {
    let state = app.state::<AppState>();
    let (semantic_state, semantic_detail) = {
        let slot = state.semantic.lock().unwrap_or_else(|e| e.into_inner());
        match &slot.handle {
            Some(h) if h.sem.health().is_available() => ("ready", Some(h.sem.describe())),
            Some(h) => (
                "unavailable",
                Some(format!(
                    "{} — 연속 실패: {}",
                    h.sem.describe(),
                    h.sem.health().last_error().unwrap_or_default()
                )),
            ),
            None => ("off", slot.off.clone()),
        }
    };
    let ai = state
        .ai_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let inner = state.lock_inner();
    let (pending_count, files_indexed) = inner
        .mover
        .as_ref()
        .map(|m| {
            (
                m.store().pending_count().unwrap_or(0),
                m.store().index_stats().map(|s| s.files).unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    let running = state
        .watch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    StatusDto {
        config_path: s(&state.config_path),
        config_exists: state.config_path.is_file(),
        rules_path: s(&inner.cfg.classify.rules),
        rules_exists: inner.cfg.classify.rules.is_file(),
        db_path: s(&inner.cfg.store.db_path()),
        watching: if running {
            inner.cfg.watches.iter().map(|w| s(&w.path)).collect()
        } else {
            Vec::new()
        },
        paused: state.is_paused(),
        indexing: state.indexing.load(std::sync::atomic::Ordering::Relaxed),
        pending_count,
        files_indexed,
        last_error: state.last_error(),
        hotkey: inner.cfg.general.hotkey.clone(),
        ai_state: match (&ai, inner.cfg.ai.enabled) {
            (_, false) => "off",
            (Some(Ok(_)), _) => "ready",
            (Some(Err(_)), _) => "unavailable",
            (None, true) => "starting",
        },
        ai_detail: match ai {
            Some(Ok(m)) | Some(Err(m)) if inner.cfg.ai.enabled => Some(m),
            _ => None,
        },
        offline: if running {
            state
                .offline
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|p| s(p))
                .collect()
        } else {
            Vec::new()
        },
        semantic_state,
        semantic_detail,
    }
}

#[tauri::command]
pub async fn get_status(app: AppHandle) -> CmdResult<StatusDto> {
    blocking(move || Ok(status(&app))).await
}

#[tauri::command]
pub async fn set_paused(app: AppHandle, paused: bool) -> CmdResult<StatusDto> {
    blocking(move || {
        tray::set_paused(&app, paused);
        Ok(status(&app))
    })
    .await
}

#[tauri::command]
pub async fn index_now(app: AppHandle) -> CmdResult<()> {
    watch::spawn_index(&app);
    Ok(())
}

/// "search" | "main" | "main:<tab>" | "confirm"
#[tauri::command]
pub async fn show_window(app: AppHandle, which: String) -> CmdResult<()> {
    match which.split_once(':') {
        Some(("main", tab)) => windows::show_main(&app, tab),
        _ if which == "main" => windows::show_main(&app, "activity"),
        _ if which == "search" => windows::show_search(&app),
        _ if which == "confirm" => windows::show_confirm(&app),
        _ => return Err(format!("unknown window {which}")),
    }
    Ok(())
}

// ── search ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub ext: Option<String>,
    pub size: u64,
    pub mtime: i64,
    /// name | content | both | filter | semantic
    pub matched_in: &'static str,
    pub name_highlight: String,
    pub snippet: String,
    pub text_status: &'static str,
    pub exists: bool,
    /// Cosine similarity to the query (semantic search only).
    pub similarity: Option<f32>,
}

fn hit_dto(h: SearchHit) -> HitDto {
    let (name, dir) = name_and_dir(&h.file.path);
    HitDto {
        exists: h.file.path.exists(),
        path: s(&h.file.path),
        name,
        dir,
        ext: h.file.ext,
        size: h.file.size,
        mtime: h.file.mtime,
        matched_in: match h.matched_in {
            MatchedIn::Name => "name",
            MatchedIn::Content => "content",
            MatchedIn::Both => "both",
            MatchedIn::Filter => "filter",
            MatchedIn::Semantic => "semantic",
        },
        name_highlight: h.name_highlight,
        snippet: h.snippet,
        text_status: h.file.text_status.as_str(),
        similarity: h.similarity,
    }
}

fn search_opts(limit: Option<u32>) -> SearchOptions {
    SearchOptions {
        limit: limit.unwrap_or(50) as usize,
        mark_start: MARK_START.into(),
        mark_end: MARK_END.into(),
        snippet_tokens: 24,
    }
}

#[tauri::command]
pub async fn search(app: AppHandle, query: String, limit: Option<u32>) -> CmdResult<Vec<HitDto>> {
    blocking(move || {
        let q = Query::parse(&query);
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let hits = fts_search(mover.store(), &q, &search_opts(limit)).map_err(|e| e.to_string())?;
        Ok(hits.into_iter().map(hit_dto).collect())
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultDto {
    pub hits: Vec<HitDto>,
    /// "fts" (words only) | "hybrid" (words + meaning)
    pub mode: &'static str,
    /// The query read like a sentence/question.
    pub natural: bool,
    /// Why meaning did not take part, when it didn't.
    pub note: Option<String>,
}

/// Words + meaning (SPEC §5.6). Without an embedding model, or when it
/// fails or is slow, this is the plain word search with a note — never an
/// error because of the AI (CLAUDE.md rule 4).
#[tauri::command]
pub async fn search_semantic(
    app: AppHandle,
    query: String,
    limit: Option<u32>,
) -> CmdResult<SearchResultDto> {
    blocking(move || {
        let q = Query::parse(&query);
        if q.is_empty() {
            return Ok(SearchResultDto {
                hits: Vec::new(),
                mode: "fts",
                natural: false,
                note: None,
            });
        }
        let opts = search_opts(limit);
        let out = match semantic::handle(&app) {
            Some(h) => {
                let store = h.store.lock().unwrap_or_else(|e| e.into_inner());
                hybrid_search(&store, Some(&h.sem), &q, &opts).map_err(|e| e.to_string())?
            }
            None => {
                let state = app.state::<AppState>();
                let inner = state.lock_inner();
                let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
                let mut out =
                    hybrid_search(mover.store(), None, &q, &opts).map_err(|e| e.to_string())?;
                out.note = state
                    .semantic
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .off
                    .clone()
                    .map(|why| format!("의미검색 꺼짐 — 단어 검색만: {why}"));
                out
            }
        };
        Ok(SearchResultDto {
            mode: match out.mode {
                SearchMode::Fts => "fts",
                SearchMode::Hybrid => "hybrid",
            },
            natural: out.natural,
            note: out.note,
            hits: out.hits.into_iter().map(hit_dto).collect(),
        })
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticStatusDto {
    /// "off" | "ready" | "unavailable"
    pub state: &'static str,
    pub detail: Option<String>,
    /// Files with a current vector / indexed files.
    pub embedded: u64,
    pub files: u64,
}

#[tauri::command]
pub async fn semantic_status(app: AppHandle) -> CmdResult<SemanticStatusDto> {
    blocking(move || {
        let st = status(&app);
        let (embedded, files) = match semantic::handle(&app) {
            Some(h) => {
                let store = h.store.lock().unwrap_or_else(|e| e.into_inner());
                let v = store
                    .vector_stats(h.sem.model_key())
                    .map_err(|e| e.to_string())?;
                (v.embedded, v.files)
            }
            None => (0, st.files_indexed),
        };
        Ok(SemanticStatusDto {
            state: st.semantic_state,
            detail: st.semantic_detail,
            embedded,
            files,
        })
    })
    .await
}

#[tauri::command]
pub async fn open_path(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reveal_path(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener()
        .reveal_item_in_dir(PathBuf::from(path))
        .map_err(|e| e.to_string())
}

// ── pending / confirm ───────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDto {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub dir: String,
    pub suggested_dest: Option<String>,
    /// Rule name (source "rule").
    pub rule: Option<String>,
    /// "rule" | "ai"
    pub source: &'static str,
    /// AI confidence 0..1 and one-line reason (source "ai").
    pub confidence: Option<f64>,
    pub reason: Option<String>,
    /// AI-suggested new file name.
    pub suggested_name: Option<String>,
    pub created_at: i64,
    pub exists: bool,
    pub size: Option<u64>,
}

#[tauri::command]
pub async fn list_pending(app: AppHandle) -> CmdResult<Vec<PendingDto>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let items = mover.store().list_pending().map_err(|e| e.to_string())?;
        Ok(items
            .into_iter()
            .map(|p| {
                let meta = std::fs::metadata(&p.path).ok();
                let (name, dir) = name_and_dir(&p.path);
                PendingDto {
                    id: p.id,
                    path: s(&p.path),
                    name,
                    dir,
                    suggested_dest: p.suggested_dest.as_deref().map(s),
                    source: p.source.as_str(),
                    rule: (p.source == PendingSource::Rule)
                        .then(|| p.reason.clone())
                        .flatten(),
                    reason: (p.source == PendingSource::Ai)
                        .then_some(p.reason)
                        .flatten(),
                    confidence: p.confidence,
                    suggested_name: p.suggested_name,
                    created_at: p.created_at,
                    exists: meta.as_ref().is_some_and(|m| m.is_file()),
                    size: meta.map(|m| m.len()),
                }
            })
            .collect())
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DecisionDto {
    #[serde(rename_all = "camelCase")]
    Move {
        dest_dir: Option<String>,
        #[serde(default)]
        new_name: Option<String>,
    },
    Skip,
    Ignore,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionDto {
    /// moved | duplicate | already_in_place | skipped | ignored | gone
    pub kind: &'static str,
    pub path: String,
    pub dst: Option<String>,
    pub batch_id: Option<String>,
}

#[tauri::command]
pub async fn resolve_pending(
    app: AppHandle,
    id: i64,
    decision: DecisionDto,
) -> CmdResult<ResolutionDto> {
    let res = blocking({
        let app = app.clone();
        move || {
            let batch = new_batch_id();
            resolve_one(&app, id, &decision.into(), &batch)
        }
    })
    .await;
    watch::pending_changed(&app, false);
    res
}

/// Bulk action from the queue screen. All moves share one batch id so a
/// single "마지막 이동 되돌리기" restores the whole selection (CLAUDE.md
/// rule 3). Items are processed in order; a failure does not stop the rest.
#[tauri::command]
pub async fn resolve_pending_batch(
    app: AppHandle,
    ids: Vec<i64>,
    decision: DecisionDto,
) -> CmdResult<Vec<Result<ResolutionDto, String>>> {
    let res = blocking({
        let app = app.clone();
        move || {
            let decision: Decision = decision.into();
            let batch = new_batch_id();
            Ok(ids
                .iter()
                .map(|&id| resolve_one(&app, id, &decision, &batch))
                .collect())
        }
    })
    .await;
    watch::pending_changed(&app, false);
    res
}

impl From<DecisionDto> for Decision {
    fn from(d: DecisionDto) -> Self {
        match d {
            DecisionDto::Move { dest_dir, new_name } => Decision::Move {
                dest_dir: dest_dir
                    .filter(|d| !d.trim().is_empty())
                    .map(|d| tidy_core::config::expand_home(Path::new(d.trim()))),
                new_name: new_name.filter(|n| !n.trim().is_empty()),
            },
            DecisionDto::Skip => Decision::Skip,
            DecisionDto::Ignore => Decision::Ignore,
        }
    }
}

fn resolve_one(
    app: &AppHandle,
    id: i64,
    decision: &Decision,
    batch: &str,
) -> CmdResult<ResolutionDto> {
    let state = app.state::<AppState>();
    let inner = state.lock_inner();
    let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
    let idx = IndexOptions::from_config(&inner.cfg);
    let r = resolve(mover, id, decision, batch, Some(&idx)).map_err(|e| e.to_string())?;
    Ok(match r {
        Resolution::Moved {
            src, dst, batch_id, ..
        } => {
            watch::emit_activity(app, "moved", &src, format!("→ {} [확인]", s(&dst)));
            ResolutionDto {
                kind: "moved",
                path: s(&src),
                dst: Some(s(&dst)),
                batch_id: Some(batch_id),
            }
        }
        Resolution::Duplicate { src, existing } => ResolutionDto {
            kind: "duplicate",
            path: s(&src),
            dst: Some(s(&existing)),
            batch_id: None,
        },
        Resolution::AlreadyInPlace { path } => simple("already_in_place", &path),
        Resolution::Skipped { path } => simple("skipped", &path),
        Resolution::Ignored { path } => simple("ignored", &path),
        Resolution::Gone { path } => simple("gone", &path),
    })
}

fn simple(kind: &'static str, p: &Path) -> ResolutionDto {
    ResolutionDto {
        kind,
        path: s(p),
        dst: None,
        batch_id: None,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoDto {
    pub restored: Vec<(String, String)>,
    pub conflicts: Vec<String>,
}

/// Undo the most recent batch (same as `tidy undo --last`).
#[tauri::command]
pub async fn undo_last(app: AppHandle) -> CmdResult<Option<UndoDto>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let Some(report) = mover.undo_last().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        reindex_undo(
            mover.store(),
            &report,
            &IndexOptions::from_config(&inner.cfg),
        );
        for (from, to) in &report.restored {
            watch::emit_activity(&app, "info", to, format!("되돌림 ← {}", s(from)));
        }
        Ok(Some(UndoDto {
            restored: report.restored.iter().map(|(a, b)| (s(a), s(b))).collect(),
            conflicts: report
                .conflicts
                .iter()
                .map(|c| format!("{}: {}", s(&c.dst_path), c.reason))
                .collect(),
        }))
    })
    .await
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle, start: Option<String>) -> CmdResult<Option<String>> {
    blocking(move || {
        let mut dialog = app.dialog().file();
        if let Some(dir) = start.map(PathBuf::from).filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        Ok(dialog
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| s(&p)))
    })
    .await
}

/// Whether a folder looks like a network share (UNC, remote drive, SMB/NFS
/// mount) — the default "종류" when the user adds a folder.
#[tauri::command]
pub async fn is_network_path(path: String) -> CmdResult<bool> {
    blocking(move || Ok(tidy_core::paths::looks_like_network(Path::new(&path)))).await
}

// ── settings ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WatchDto {
    pub path: String,
    /// "local" | "network"
    pub kind: String,
    pub recursive: bool,
    pub index_content: bool,
    pub poll_interval_s: Option<u64>,
    /// Network folders: polling only, no OS events.
    #[serde(default)]
    pub poll_only: bool,
    /// "HH:MM-HH:MM": background body-text indexing only in this window.
    #[serde(default)]
    pub content_index_window: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub config_path: String,
    pub watches: Vec<WatchDto>,
    pub rules_path: String,
    pub rules_text: String,
    pub rules_exists: bool,
    pub db_path: String,
    pub db_path_custom: Option<String>,
    pub db_path_default: String,
    pub quiet_mode: bool,
    pub hotkey: String,
    /// `general.start_on_login` and the OS login item state.
    pub autostart: autostart::AutostartDto,
}

/// Shown in the rules editor when there is no rules.toml yet (not saved
/// until the user presses Save).
const RULES_TEMPLATE: &str = include_str!("../../../../rules.example.toml");

#[tauri::command]
pub async fn get_settings(app: AppHandle) -> CmdResult<SettingsDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config();
        let rules_exists = cfg.classify.rules.is_file();
        let rules_text = if rules_exists {
            std::fs::read_to_string(&cfg.classify.rules).map_err(|e| e.to_string())?
        } else {
            RULES_TEMPLATE.to_string()
        };
        Ok(SettingsDto {
            config_path: s(&state.config_path),
            watches: cfg
                .watches
                .iter()
                .map(|w| WatchDto {
                    path: s(&w.path),
                    kind: match w.kind {
                        RootKind::Local => "local".into(),
                        RootKind::Network => "network".into(),
                    },
                    recursive: w.recursive,
                    index_content: w.index_content,
                    poll_interval_s: w.poll_interval_s,
                    poll_only: w.poll_only,
                    content_index_window: w.content_index_window.clone(),
                })
                .collect(),
            rules_path: s(&cfg.classify.rules),
            rules_text,
            rules_exists,
            db_path: s(&cfg.store.db_path()),
            db_path_custom: cfg.store.path.as_deref().map(s),
            db_path_default: s(&tidy_core::config::Store::default_db_path()),
            quiet_mode: cfg.general.quiet_mode,
            hotkey: cfg.general.hotkey.clone(),
            autostart: autostart::state(&app, cfg.general.start_on_login),
        })
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesCheckDto {
    pub ok: bool,
    pub rules: Vec<String>,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn check_rules(text: String) -> CmdResult<RulesCheckDto> {
    Ok(match RuleSet::from_toml(&text) {
        Ok(rs) => RulesCheckDto {
            ok: true,
            rules: rs.rules().iter().map(|r| r.name.clone()).collect(),
            error: None,
        },
        Err(e) => RulesCheckDto {
            ok: false,
            rules: Vec::new(),
            error: Some(e.to_string()),
        },
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsArgs {
    pub watches: Vec<WatchDto>,
    pub db_path_custom: Option<String>,
    pub quiet_mode: bool,
    pub hotkey: String,
    pub rules_text: String,
    #[serde(default = "default_true")]
    pub start_on_login: bool,
}

fn default_true() -> bool {
    true
}

/// Validate, write config.toml (comments kept) and rules.toml, then reload:
/// reopen the DB, re-register the hotkey, restart watching, re-index.
#[tauri::command]
pub async fn save_settings(app: AppHandle, args: SaveSettingsArgs) -> CmdResult<StatusDto> {
    blocking(move || {
        RuleSet::from_toml(&args.rules_text).map_err(|e| format!("규칙 오류: {e}"))?;
        let mut watches = Vec::new();
        for w in &args.watches {
            let path = w.path.trim();
            if path.is_empty() {
                continue;
            }
            let network = w.kind == "network";
            let window = w
                .content_index_window
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if let Some(win) = &window {
                TimeWindow::parse(win).map_err(|e| format!("{path}: 본문 인덱싱 시간대 — {e}"))?;
            }
            watches.push(WatchRoot {
                path: PathBuf::from(path),
                kind: if network {
                    RootKind::Network
                } else {
                    RootKind::Local
                },
                recursive: w.recursive,
                index_content: w.index_content,
                poll_interval_s: w.poll_interval_s.filter(|s| *s > 0),
                content_index_window: window.filter(|_| network),
                poll_only: network && w.poll_only,
            });
        }
        let hotkey = if args.hotkey.trim().is_empty() {
            tidy_core::config::General::default().hotkey
        } else {
            args.hotkey.trim().to_string()
        };
        let state = app.state::<AppState>();
        let patch = SettingsPatch {
            watches,
            store_path: args
                .db_path_custom
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            quiet_mode: args.quiet_mode,
            hotkey: hotkey.clone(),
            start_on_login: args.start_on_login,
        };
        write_settings(&state.config_path, &patch).map_err(|e| e.to_string())?;
        // Rules path comes from the (re)loaded config.
        state.reload();
        let rules_path = state.config().classify.rules;
        RuleSet::save_text(&rules_path, &args.rules_text).map_err(|e| e.to_string())?;
        state.clear_error();
        if state.lock_inner().hotkey.as_deref() != Some(hotkey.as_str()) {
            windows::register_hotkey(&app, &hotkey);
        }
        let auto = autostart::sync(&app, args.start_on_login);
        if let Some(e) = auto.error {
            state.set_error(e);
        }
        watch::restart(&app);
        watch::spawn_index(&app);
        watch::pending_changed(&app, false);
        semantic::restart_embed(&app);
        Ok(status(&app))
    })
    .await
}

// ── AI settings, key, test ──────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsDto {
    pub enabled: bool,
    pub active_profile: String,
    /// All profile names in the config (for the profile picker).
    #[serde(default)]
    pub profiles: Vec<String>,
    /// "anthropic" | "openai" | "ollama" | "" (none)
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub key_ref: String,
    pub effort: String,
    pub auto_threshold: f64,
    pub send_content: bool,
    /// Whether a key is stored in the OS keychain under `key_ref`
    /// (the key itself never leaves the backend).
    #[serde(default)]
    pub key_stored: bool,
    /// Embedding model (semantic search): "openai" | "ollama" | "" (none).
    #[serde(default)]
    pub embed_provider: String,
    #[serde(default)]
    pub embed_model: String,
    #[serde(default)]
    pub embed_base_url: String,
    #[serde(default)]
    pub embed_key_ref: String,
    #[serde(default)]
    pub embed_key_stored: bool,
    /// Where keys go on this machine (e.g. "Secret Service(GNOME 키링/KWallet)").
    #[serde(default)]
    pub key_store: String,
}

fn ai_settings(cfg: &tidy_core::Config) -> AiSettingsDto {
    let pc = cfg.ai.active().and_then(|p| p.classify.clone());
    let ec = cfg.ai.active().and_then(|p| p.embed.clone());
    let embed_key_ref = ec
        .as_ref()
        .and_then(|p| p.key_ref.clone())
        .unwrap_or_default();
    let key_ref = pc
        .as_ref()
        .and_then(|p| p.key_ref.clone())
        .unwrap_or_default();
    AiSettingsDto {
        enabled: cfg.ai.enabled,
        active_profile: cfg.ai.active_profile.clone(),
        profiles: cfg.ai.profiles.keys().cloned().collect(),
        provider: pc
            .as_ref()
            .map(|p| p.provider.as_str().to_string())
            .unwrap_or_default(),
        model: pc.as_ref().map(|p| p.model.clone()).unwrap_or_default(),
        base_url: pc
            .as_ref()
            .and_then(|p| p.base_url.clone())
            .unwrap_or_default(),
        key_stored: !key_ref.is_empty() && tidy_core::ai::keys::has_key(&key_ref),
        key_ref,
        effort: pc.and_then(|p| p.effort).unwrap_or_default(),
        auto_threshold: cfg.ai.auto_threshold,
        send_content: cfg.ai.send_content,
        embed_provider: ec
            .as_ref()
            .map(|p| p.provider.as_str().to_string())
            .unwrap_or_default(),
        embed_model: ec.as_ref().map(|p| p.model.clone()).unwrap_or_default(),
        embed_base_url: ec
            .as_ref()
            .and_then(|p| p.base_url.clone())
            .unwrap_or_default(),
        embed_key_stored: !embed_key_ref.is_empty() && tidy_core::ai::keys::has_key(&embed_key_ref),
        embed_key_ref,
        key_store: tidy_core::ai::keys::store_name().to_string(),
    }
}

#[tauri::command]
pub async fn get_ai_settings(app: AppHandle) -> CmdResult<AiSettingsDto> {
    blocking(move || Ok(ai_settings(&app.state::<AppState>().config()))).await
}

/// Save `[ai]` (comments kept). An `apiKey` goes to the OS keychain only.
#[tauri::command]
pub async fn save_ai_settings(
    app: AppHandle,
    settings: AiSettingsDto,
    api_key: Option<String>,
    embed_api_key: Option<String>,
) -> CmdResult<AiSettingsDto> {
    blocking(move || {
        let provider = match settings.provider.as_str() {
            "anthropic" => Some(ProviderKind::Anthropic),
            "openai" => Some(ProviderKind::Openai),
            "ollama" => Some(ProviderKind::Ollama),
            "" => None,
            other => return Err(format!("알 수 없는 제공자: {other}")),
        };
        let classify = match provider {
            Some(kind) => {
                if settings.model.trim().is_empty() {
                    return Err("모델 이름을 입력하세요".into());
                }
                let key_ref = settings.key_ref.trim().to_string();
                if kind == ProviderKind::Anthropic && key_ref.is_empty() {
                    return Err(
                        "Anthropic은 키 이름(key_ref)이 필요합니다 (예: tidy/anthropic)".into(),
                    );
                }
                Some(ProviderConfig {
                    provider: kind,
                    model: settings.model.trim().to_string(),
                    key_ref: (!key_ref.is_empty()).then_some(key_ref),
                    base_url: Some(settings.base_url.trim().to_string()).filter(|u| !u.is_empty()),
                    effort: Some(settings.effort.trim().to_string()).filter(|e| !e.is_empty()),
                })
            }
            None => None,
        };
        let embed = match settings.embed_provider.as_str() {
            "" => None,
            "anthropic" => {
                return Err(
                    "Anthropic에는 임베딩 API가 없습니다 — OpenAI 호환 또는 Ollama를 고르세요"
                        .into(),
                )
            }
            p @ ("openai" | "ollama") => {
                if settings.embed_model.trim().is_empty() {
                    return Err("임베딩 모델 이름을 입력하세요".into());
                }
                let key_ref = settings.embed_key_ref.trim().to_string();
                Some(ProviderConfig {
                    provider: if p == "openai" {
                        ProviderKind::Openai
                    } else {
                        ProviderKind::Ollama
                    },
                    model: settings.embed_model.trim().to_string(),
                    key_ref: (!key_ref.is_empty()).then_some(key_ref),
                    base_url: Some(settings.embed_base_url.trim().to_string())
                        .filter(|u| !u.is_empty()),
                    effort: None,
                })
            }
            other => return Err(format!("알 수 없는 임베딩 제공자: {other}")),
        };
        for (key, r) in [
            (
                &api_key,
                classify.as_ref().and_then(|c| c.key_ref.as_deref()),
            ),
            (
                &embed_api_key,
                embed.as_ref().and_then(|c| c.key_ref.as_deref()),
            ),
        ] {
            if let (Some(key), Some(r)) = (key.as_deref().filter(|k| !k.trim().is_empty()), r) {
                tidy_core::ai::keys::set_key(r, key).map_err(|e| e.to_string())?;
            }
        }
        let profile = if settings.active_profile.trim().is_empty() {
            "default".to_string()
        } else {
            settings.active_profile.trim().to_string()
        };
        let state = app.state::<AppState>();
        write_ai_settings(
            &state.config_path,
            &AiPatch {
                enabled: settings.enabled,
                active_profile: profile,
                auto_threshold: settings.auto_threshold.clamp(0.0, 1.0),
                send_content: settings.send_content,
                classify,
                embed,
            },
        )
        .map_err(|e| e.to_string())?;
        state.reload();
        watch::restart(&app);
        semantic::restart_embed(&app);
        Ok(ai_settings(&state.config()))
    })
    .await
}

#[tauri::command]
pub async fn delete_api_key(app: AppHandle, key_ref: String) -> CmdResult<AiSettingsDto> {
    blocking(move || {
        tidy_core::ai::keys::delete_key(&key_ref).map_err(|e| e.to_string())?;
        Ok(ai_settings(&app.state::<AppState>().config()))
    })
    .await
}

/// One tiny request with the saved settings ("연결 테스트").
#[tauri::command]
pub async fn test_ai(app: AppHandle) -> CmdResult<String> {
    blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config();
        let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
        let ai = AiClassifier::from_config(&cfg, &rules).map_err(|e| e.to_string())?;
        // Own connection: the UI's shared one must not be held during a
        // network call of up to `ai.timeout_secs`.
        let store =
            tidy_core::store::Store::open(&cfg.store.db_path()).map_err(|e| e.to_string())?;
        ai.test_connection(&store)
            .map(|m| format!("연결됨: {} / {m}", ai.provider_name()))
            .map_err(|e| e.to_string())
    })
    .await
}

// ── learner ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnDto {
    pub key: String,
    pub rule_name: String,
    pub count: u32,
    pub examples: Vec<String>,
    pub dest: String,
    pub rule_toml: String,
}

#[tauri::command]
pub async fn list_learn(app: AppHandle) -> CmdResult<Vec<LearnDto>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config();
        let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        Ok(
            learner::suggestions(mover.store(), &rules, cfg.classify.learn_threshold)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|s| LearnDto {
                    dest: s.dest.display().to_string(),
                    key: s.key,
                    rule_name: s.rule_name,
                    count: s.count,
                    examples: s.examples,
                    rule_toml: s.rule_toml,
                })
                .collect(),
        )
    })
    .await
}

/// Add the learned rule to rules.toml and restart watching with it.
#[tauri::command]
pub async fn accept_learn(app: AppHandle, key: String) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config();
        {
            let inner = state.lock_inner();
            let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
            learner::accept(
                mover.store(),
                &cfg.classify.rules,
                cfg.classify.learn_threshold,
                &key,
            )
            .map_err(|e| e.to_string())?;
        }
        watch::emit_activity(
            &app,
            "info",
            &cfg.classify.rules,
            "학습한 규칙을 추가했습니다".into(),
        );
        watch::restart(&app);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn dismiss_learn(app: AppHandle, key: String) -> CmdResult<()> {
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        learner::dismiss(mover.store(), &key).map_err(|e| e.to_string())
    })
    .await
}
