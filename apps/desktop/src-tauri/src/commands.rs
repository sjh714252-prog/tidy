//! Tauri commands (IPC). Every command is async and does its work on the
//! blocking pool (CLAUDE.md: heavy IO via spawn_blocking), so the UI thread
//! never waits on SQLite or the filesystem. Types mirror `src/ipc/types.ts`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tidy_core::classify::RuleSet;
use tidy_core::config::{write_settings, RootKind, SettingsPatch, WatchRoot};
use tidy_core::engine::{new_batch_id, reindex_undo};
use tidy_core::index::IndexOptions;
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::search::{search as fts_search, MatchedIn, Query, SearchOptions};

use crate::state::AppState;
use crate::{tray, watch, windows};

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
}

fn status(app: &AppHandle) -> StatusDto {
    let state = app.state::<AppState>();
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
    /// name | content | both | filter
    pub matched_in: &'static str,
    pub name_highlight: String,
    pub snippet: String,
    pub text_status: &'static str,
    pub exists: bool,
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
        let opts = SearchOptions {
            limit: limit.unwrap_or(50) as usize,
            mark_start: MARK_START.into(),
            mark_end: MARK_END.into(),
            snippet_tokens: 24,
        };
        let hits = fts_search(mover.store(), &q, &opts).map_err(|e| e.to_string())?;
        Ok(hits
            .into_iter()
            .map(|h| {
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
                    },
                    name_highlight: h.name_highlight,
                    snippet: h.snippet,
                    text_status: h.file.text_status.as_str(),
                }
            })
            .collect())
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
    pub rule: Option<String>,
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
                    rule: p.reason,
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
            let decision = match decision {
                DecisionDto::Move { dest_dir } => Decision::Move {
                    dest_dir: dest_dir
                        .filter(|d| !d.trim().is_empty())
                        .map(|d| tidy_core::config::expand_home(Path::new(d.trim()))),
                },
                DecisionDto::Skip => Decision::Skip,
                DecisionDto::Ignore => Decision::Ignore,
            };
            let state = app.state::<AppState>();
            let inner = state.lock_inner();
            let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
            let idx = IndexOptions::from_config(&inner.cfg);
            let batch = new_batch_id();
            let r = resolve(mover, id, &decision, &batch, Some(&idx)).map_err(|e| e.to_string())?;
            Ok(match r {
                Resolution::Moved {
                    src, dst, batch_id, ..
                } => {
                    watch::emit_activity(&app, "moved", &src, format!("→ {} [확인]", s(&dst)));
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
    })
    .await;
    watch::pending_changed(&app, false);
    res
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
            watches.push(WatchRoot {
                path: PathBuf::from(path),
                kind: if w.kind == "network" {
                    RootKind::Network
                } else {
                    RootKind::Local
                },
                recursive: w.recursive,
                index_content: w.index_content,
                poll_interval_s: w.poll_interval_s,
                content_index_window: None,
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
        watch::restart(&app);
        watch::spawn_index(&app);
        watch::pending_changed(&app, false);
        Ok(status(&app))
    })
    .await
}
