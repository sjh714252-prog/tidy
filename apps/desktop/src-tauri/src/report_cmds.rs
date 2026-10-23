//! M5 commands: folder report, restructure plans (rules / AI) with
//! before/after preview and approved apply, move history by batch.
//! Report and plan objects are sent as the core's own JSON (snake_case);
//! `src/ipc/types.ts` mirrors them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tidy_core::classify::{AiClassifier, RuleSet};
use tidy_core::engine::reindex_undo;
use tidy_core::index::IndexOptions;
use tidy_core::report::{self, restructure, DupGroup, Stats, TreeOptions};
use tidy_core::store::{BatchSummary, MoveLogEntry, Store, StoredPlan};

use crate::commands::UndoDto;
use crate::state::AppState;
use crate::watch;

type CmdResult<T> = Result<T, String>;

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn expand(p: &str) -> PathBuf {
    tidy_core::config::expand_home(Path::new(p.trim()))
}

/// Folders offered in the picker: the watch folders only (M11, SPEC
/// §14.6); any other folder goes through "다른 폴더…".
#[tauri::command]
pub async fn report_roots(app: AppHandle) -> CmdResult<Vec<String>> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let mut out: Vec<String> = cfg
            .watches
            .iter()
            .map(|w| w.path.display().to_string())
            .collect();
        out.dedup();
        Ok(out)
    })
    .await
}

/// (M11) The "지정 위치로 모으기" root `path` is (or lies in): organizing
/// there would move already organized files again.
#[tauri::command]
pub async fn organize_target(app: AppHandle, path: String) -> CmdResult<Option<String>> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let p = tidy_core::config::expand_home(std::path::Path::new(path.trim()));
        Ok(
            tidy_core::report::backlog::organize_root_of(&cfg.watches, &p)
                .map(|w| w.anchor().display().to_string()),
        )
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDto {
    pub root: String,
    pub generated_at: i64,
    pub stale_days: u64,
    /// Folder tree as text (depth 3).
    pub tree_text: String,
    /// Core `Stats` JSON (snake_case).
    pub stats: Stats,
    pub duplicates: Vec<DupGroup>,
    /// (M9) "개인정보 의심 파일 N개(미암호화 M개)" from the index.
    pub pii: Option<String>,
    pub pii_suspected: u64,
    pub pii_unencrypted: u64,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn build_report(app: AppHandle, path: String, duplicates: bool) -> CmdResult<ReportDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        // Own connection (hash cache only): never hold the UI one for a scan.
        let store = Store::open(&cfg.store.db_path()).ok();
        let opts = report::ReportOptions {
            top_n: cfg.report.top_n,
            stale_days: cfg.report.stale_days,
            duplicates: duplicates.then(report::DupOptions::default),
            scan: report::ScanOptions {
                max_depth: cfg.report.scan_depth,
                ..Default::default()
            },
        };
        let r = report::build(&expand(&path), &opts, store.as_ref()).map_err(err)?;
        Ok(ReportDto {
            root: r.root.display().to_string(),
            generated_at: r.generated_at,
            stale_days: r.stale_days,
            tree_text: report::render_tree(
                &r.tree,
                &TreeOptions {
                    max_depth: Some(3),
                    ..Default::default()
                },
            ),
            stats: r.stats,
            duplicates: r.duplicates,
            pii: r.pii.map(|p| p.label()),
            pii_suspected: r.pii.map(|p| p.suspected).unwrap_or(0),
            pii_unencrypted: r.pii.map(|p| p.unencrypted).unwrap_or(0),
            errors: r.errors,
        })
    })
    .await
}

fn protected(cfg: &tidy_core::Config, extra: &[String]) -> Vec<PathBuf> {
    cfg.report
        .protected
        .iter()
        .cloned()
        .chain(
            extra
                .iter()
                .filter(|p| !p.trim().is_empty())
                .map(|p| expand(p)),
        )
        .map(|p| dunce::canonicalize(&p).unwrap_or(p))
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDto {
    /// Core `Plan` JSON (snake_case).
    pub plan: restructure::Plan,
    pub preview: restructure::Preview,
}

/// What rules.toml would do with the files already in `path`.
#[tauri::command]
pub async fn plan_rules(
    app: AppHandle,
    path: String,
    recursive: bool,
    protect: Vec<String>,
) -> CmdResult<PlanDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let rules = RuleSet::load(&cfg.classify.rules).map_err(err)?;
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let root = expand(&path);
        let canon = dunce::canonicalize(&root).unwrap_or_else(|_| root.clone());
        let anchors = tidy_core::classify::Anchors::from_config(&cfg);
        let plan = restructure::plan_from_rules_in(
            &root,
            &rules,
            recursive,
            &cfg.ignore_filter().map_err(|e| e.to_string())?,
            &protected(&cfg, &protect),
            Some(anchors.for_path(&canon).unwrap_or(&canon)),
        )
        .map_err(err)?;
        let plan = restructure::save(&store, plan).map_err(err)?;
        let preview = restructure::preview(&plan, None).map_err(err)?;
        Ok(PlanDto { plan, preview })
    })
    .await
}

/// Ask the AI for a restructure plan (can take a minute; nothing moves).
#[tauri::command]
pub async fn plan_ai(app: AppHandle, path: String, protect: Vec<String>) -> CmdResult<PlanDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
        let ai = AiClassifier::from_config(&cfg, &rules).map_err(|e| {
            format!("AI를 쓸 수 없습니다: {e}. '규칙으로 정리 미리보기'는 AI 없이 됩니다.")
        })?;
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let plan = restructure::plan_from_ai(
            &expand(&path),
            &ai,
            &store,
            &protected(&cfg, &protect),
            Duration::from_secs(cfg.ai.restructure_timeout_secs),
            cfg.report.max_moves,
        )
        .map_err(err)?;
        let plan = restructure::save(&store, plan).map_err(err)?;
        let preview = restructure::preview(&plan, None).map_err(err)?;
        Ok(PlanDto { plan, preview })
    })
    .await
}

/// Before/after trees for the moves the user ticked.
#[tauri::command]
pub async fn preview_plan(
    app: AppHandle,
    id: i64,
    ids: Vec<u32>,
) -> CmdResult<restructure::Preview> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let plan = restructure::load(&store, id).map_err(err)?;
        restructure::preview(&plan, Some(&ids)).map_err(err)
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyDto {
    pub batch_id: String,
    pub moved: usize,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

/// Apply the ticked moves of plan `id` as one undoable batch. Only ever
/// called from the user's explicit "적용" click (CLAUDE.md rule 6).
#[tauri::command]
pub async fn apply_plan(
    app: AppHandle,
    id: i64,
    ids: Vec<u32>,
    protect: Vec<String>,
) -> CmdResult<ApplyDto> {
    crate::overview::invalidate();
    blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config();
        let inner = state.lock_inner();
        // The UI mover shares "expected moves" with the watcher, so these
        // moves are not picked up again as new files.
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let plan = restructure::load(mover.store(), id).map_err(err)?;
        let r = restructure::apply(
            mover,
            &plan,
            Some(&ids),
            &protected(&cfg, &protect),
            Some(&IndexOptions::from_config(&cfg)),
        )
        .map_err(err)?;
        watch::emit_activity(
            &app,
            "info",
            &plan.root,
            format!(
                "{} #{id}: {}개 이동 (batch {})",
                if plan.source == restructure::PlanSource::Context {
                    "업무 맥락"
                } else {
                    "재구성"
                },
                r.moved.len(),
                &r.batch_id[..8]
            ),
        );
        Ok(ApplyDto {
            batch_id: r.batch_id,
            moved: r.moved.len(),
            skipped: r
                .skipped
                .iter()
                .map(|(p, why)| format!("{}: {why}", p.display()))
                .collect(),
            errors: r
                .errors
                .iter()
                .map(|(p, e)| format!("{}: {e}", p.display()))
                .collect(),
        })
    })
    .await
}

#[tauri::command]
pub async fn list_plans(app: AppHandle) -> CmdResult<Vec<StoredPlan>> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let mut plans = store.recent_plans(20).map_err(err)?;
        for p in &mut plans {
            p.plan_json.clear(); // list view only
        }
        Ok(plans)
    })
    .await
}

#[tauri::command]
pub async fn open_plan(app: AppHandle, id: i64) -> CmdResult<PlanDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let plan = restructure::load(&store, id).map_err(err)?;
        let preview = restructure::preview(&plan, None).map_err(err)?;
        Ok(PlanDto { plan, preview })
    })
    .await
}

// ── history (SPEC §9.6) ─────────────────────────────────────────────────

#[tauri::command]
pub async fn list_batches(app: AppHandle, limit: Option<usize>) -> CmdResult<Vec<BatchSummary>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        mover.store().batches(limit.unwrap_or(100)).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn batch_moves(app: AppHandle, batch_id: String) -> CmdResult<Vec<MoveLogEntry>> {
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        mover.store().moves_in_batch(&batch_id).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn undo_batch(app: AppHandle, batch_id: String) -> CmdResult<UndoDto> {
    crate::overview::invalidate();
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let report = mover.undo_batch(&batch_id).map_err(err)?;
        reindex_undo(
            mover.store(),
            &report,
            &IndexOptions::from_config(&inner.cfg),
        );
        watch::emit_activity(
            &app,
            "info",
            Path::new(""),
            format!(
                "batch {} 되돌림: {}개",
                &batch_id[..batch_id.len().min(8)],
                report.restored.len()
            ),
        );
        Ok(UndoDto {
            restored: report
                .restored
                .iter()
                .map(|(a, b)| (a.display().to_string(), b.display().to_string()))
                .collect(),
            conflicts: report
                .conflicts
                .iter()
                .map(|c| format!("{}: {}", c.dst_path.display(), c.reason))
                .collect(),
        })
    })
    .await
}
