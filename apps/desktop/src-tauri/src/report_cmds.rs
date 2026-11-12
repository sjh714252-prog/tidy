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
            duplicates: duplicates
                .then(|| report::DupOptions::for_root(&expand(&path), &cfg.watches)),
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

/// (M19) The organize roots ("모으기" targets) the 정리하기 tab offers to
/// review ("정리 위치 점검").
#[tauri::command]
pub async fn review_roots(app: AppHandle) -> CmdResult<Vec<String>> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        Ok(report::review::review_roots(&cfg)
            .into_iter()
            .map(|p| p.display().to_string())
            .collect())
    })
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDto {
    /// `None`: nothing to re-sort.
    pub plan: Option<restructure::Plan>,
    pub preview: Option<restructure::Preview>,
    pub summary: String,
    pub merged: usize,
    pub folded: usize,
    pub by_rules: usize,
    pub by_ai: usize,
    /// Files the AI could place but was not asked to ("AI로 이것도 보기").
    pub ai_waiting: usize,
    pub ai_error: Option<String>,
    /// (0.16.0) Customer files that go to the customer's folder by kind.
    pub to_customer: usize,
    /// (0.16.0, rebuild) Customers that get a new folder.
    pub new_customers: Vec<String>,
    /// (0.16.0, rebuild) More moves than one plan holds.
    pub capped: bool,
}

fn save_preview(
    store: &Store,
    plan: Option<restructure::Plan>,
) -> CmdResult<(Option<restructure::Plan>, Option<restructure::Preview>)> {
    Ok(match plan {
        Some(p) => {
            let p = restructure::save(store, p).map_err(err)?;
            let pv = restructure::preview(&p, None).map_err(err)?;
            (Some(p), Some(pv))
        }
        None => (None, None),
    })
}

/// (0.16.1, SPEC §14.22) "정리 위치 목록 내보내기": one JSON file describing
/// the organize root (to plan a full cleanup elsewhere and open it back as
/// a plan file). Saved to Downloads; nothing moves.
#[tauri::command]
pub async fn export_listing(
    app: AppHandle,
    path: String,
    protect: Vec<String>,
) -> CmdResult<report::listing::Listing> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        report::listing::run(
            &cfg,
            &expand(&path),
            &store,
            &protected(&cfg, &protect),
            None,
        )
        .map_err(err)
    })
    .await
}

/// (0.16.0, SPEC §14.22) "정리 위치 새로 짜기": re-plan the whole organize
/// root under today's layout — every customer's file into that customer's
/// folder by kind, split folders merged, the rest by the rules. Offline;
/// nothing moves.
#[tauri::command]
pub async fn plan_rebuild(
    app: AppHandle,
    path: String,
    protect: Vec<String>,
) -> CmdResult<ReviewDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let mut r = report::rebuild::run(&cfg, &expand(&path), &store, protected(&cfg, &protect))
            .map_err(err)?;
        let (plan, preview) = save_preview(&store, r.plan.take())?;
        Ok(ReviewDto {
            plan,
            preview,
            summary: r.summary,
            merged: r.merged,
            folded: 0,
            by_rules: r.by_rules,
            by_ai: 0,
            ai_waiting: 0,
            ai_error: None,
            to_customer: r.to_customer,
            new_customers: r.new_customers,
            capped: r.capped,
        })
    })
    .await
}

/// (M19, SPEC §14.21) "정리 위치 점검": look inside an organize root and
/// propose re-sorting it. `use_ai`: also let the AI place customer files
/// outside the customer folders (only when the user asked; an AI that is
/// off or fails leaves the rest of the plan as it is). Nothing moves.
#[tauri::command]
pub async fn plan_review(
    app: AppHandle,
    path: String,
    use_ai: bool,
    protect: Vec<String>,
) -> CmdResult<ReviewDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let ai = if use_ai {
            let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
            AiClassifier::from_config(&cfg, &rules).ok()
        } else {
            None
        };
        let mut r = report::review::run(
            &cfg,
            &expand(&path),
            &store,
            protected(&cfg, &protect),
            ai.as_ref(),
        )
        .map_err(err)?;
        if use_ai && ai.is_none() && r.ai_error.is_none() && r.ai_waiting > 0 {
            r.ai_error = Some("AI를 쓸 수 없습니다 — 설정 → AI 연결을 확인하세요".into());
        }
        let (plan, preview) = save_preview(&store, r.plan.take())?;
        Ok(ReviewDto {
            plan,
            preview,
            summary: r.summary,
            merged: r.merged,
            folded: r.folded,
            by_rules: r.by_rules,
            by_ai: r.by_ai,
            ai_waiting: r.ai_waiting,
            ai_error: r.ai_error,
            to_customer: r.to_customer,
            new_customers: Vec::new(),
            capped: false,
        })
    })
    .await
}

/// (0.11.2) Open a plan file (`.json`, see `tidy_core::report::import`):
/// saved as a plan and previewed; nothing moves until applied.
#[tauri::command]
pub async fn import_plan(app: AppHandle, path: String) -> CmdResult<PlanDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let text = std::fs::read_to_string(expand(&path)).map_err(err)?;
        let plan = tidy_core::report::import::import_plan(&text).map_err(err)?;
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
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

/// (M17) A natural-language command → what it understood, the files and
/// (for a move) a saved plan. Nothing moves; `apply_plan` does, on click.
#[tauri::command]
pub async fn run_command(
    app: AppHandle,
    text: String,
    rules_only: bool,
) -> CmdResult<tidy_core::command::CommandResult> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let ai = if rules_only {
            None
        } else {
            let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
            AiClassifier::from_config(&cfg, &rules).ok()
        };
        let mut opts = tidy_core::command::CommandOptions::new(protected(&cfg, &[]));
        opts.rules_only = rules_only;
        tidy_core::command::run(&text, &cfg, &store, ai.as_ref(), &opts).map_err(err)
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
    /// (M14) Skipped because an identical file is already there.
    pub duplicates: Vec<DupSkipDto>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupSkipDto {
    pub path: String,
    pub existing: String,
    /// "똑같은 파일이 이미 …에 있어 그대로 둠"
    pub message: String,
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
                match plan.source {
                    restructure::PlanSource::Context => "업무 맥락",
                    restructure::PlanSource::Command => "명령",
                    restructure::PlanSource::Relocate => "정리 위치로 옮기기",
                    restructure::PlanSource::Imported => "제안 파일",
                    restructure::PlanSource::Compact => "정리 위치 다듬기",
                    restructure::PlanSource::Review => "정리 위치 점검",
                    restructure::PlanSource::Rebuild => "정리 위치 새로 짜기",
                    _ => "정리 제안",
                },
                r.moved.len(),
                &r.batch_id[..8]
            ),
        );
        Ok(ApplyDto {
            batch_id: r.batch_id,
            moved: r.moved.len(),
            duplicates: r
                .duplicates
                .iter()
                .map(|(p, e)| DupSkipDto {
                    path: p.display().to_string(),
                    existing: e.display().to_string(),
                    message: tidy_core::mover::duplicate_message(e),
                })
                .collect(),
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
pub async fn list_plans(app: AppHandle) -> CmdResult<Vec<PlanListItem>> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let plans = store.recent_plans(20).map_err(err)?;
        Ok(plans
            .into_iter()
            .map(|mut p| {
                let moves = serde_json::from_str::<restructure::Plan>(&p.plan_json)
                    .map(|x| x.moves.iter().filter(|m| m.from != m.to).count())
                    .unwrap_or(0);
                p.plan_json.clear(); // list view only
                PlanListItem { plan: p, moves }
            })
            .collect())
    })
    .await
}

/// A saved plan for the list, with (0.11.1) how many items it would move
/// ("적용하지 않은 미리보기" on the home tab).
#[derive(Debug, Serialize)]
pub struct PlanListItem {
    #[serde(flatten)]
    pub plan: StoredPlan,
    pub moves: usize,
}

/// (0.12.4) "버리기": forget an unapplied preview (nothing on disk moves).
#[tauri::command]
pub async fn discard_plan(app: AppHandle, id: i64) -> CmdResult<bool> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        store.discard_plan(id).map_err(err)
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

/// How many finished moves since `since_secs_ago` could be undone
/// (preview for the "최근 이동 전부 되돌리기" button).
#[tauri::command]
pub async fn count_moves_since(
    app: AppHandle,
    since_secs_ago: i64,
    auto_only: bool,
) -> CmdResult<usize> {
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - since_secs_ago.max(0);
        Ok(mover
            .store()
            .active_moves_since(since, auto_only)
            .map_err(err)?
            .len())
    })
    .await
}

/// Undo every finished move of the last `since_secs_ago` seconds in one go
/// (auto_only = rule/AI moves only). Used after a watch-time run went
/// wrong (e.g. a project folder split file by file).
#[tauri::command]
pub async fn undo_since(
    app: AppHandle,
    since_secs_ago: i64,
    auto_only: bool,
) -> CmdResult<UndoDto> {
    crate::overview::invalidate();
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - since_secs_ago.max(0);
        let report = mover.undo_since(since, auto_only).map_err(err)?;
        reindex_undo(
            mover.store(),
            &report,
            &IndexOptions::from_config(&inner.cfg),
        );
        watch::emit_activity(
            &app,
            "info",
            Path::new(""),
            format!("최근 이동 되돌림: {}개", report.restored.len()),
        );
        Ok(UndoDto::from(&report))
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
        Ok(UndoDto::from(&report))
    })
    .await
}

/// (M13) 홈 "오늘 한 일": moves since `since` (local midnight, unix
/// seconds, from the UI), a folder moved whole as one item. Read-only.
#[tauri::command]
pub async fn today_moves(
    app: AppHandle,
    since: i64,
    limit: Option<usize>,
) -> CmdResult<tidy_core::report::today::TodaySummary> {
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let entries = mover.store().moves_since(since).map_err(err)?;
        Ok(tidy_core::report::today::summarize(
            &entries,
            since,
            limit.unwrap_or(8),
        ))
    })
    .await
}

// ── duplicates (M14, SPEC §14.9 2) ──────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashDto {
    pub batch_id: String,
    pub trashed: usize,
    pub bytes: u64,
    pub failed: Vec<String>,
}

/// Send the copies the user did not keep to the trash, one batch for the
/// whole "적용" (only ever called from that click — CLAUDE.md rule 6).
/// Every file is re-hashed first; see `Mover::trash_duplicates`.
#[tauri::command]
pub async fn trash_duplicates(
    app: AppHandle,
    groups: Vec<tidy_core::mover::DupTrash>,
) -> CmdResult<TrashDto> {
    crate::overview::invalidate();
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let batch = tidy_core::engine::new_batch_id();
        let r = mover.trash_duplicates(&groups, &batch);
        // Trashed files leave the search index.
        for (p, _) in &r.trashed {
            if let Err(e) = mover.store().remove_file(p) {
                tracing::debug!("index row not removed: {e}");
            }
        }
        watch::emit_activity(
            &app,
            "info",
            Path::new(""),
            format!(
                "중복 정리: {}개를 휴지통으로 ({:.1} MB)",
                r.trashed.len(),
                r.bytes() as f64 / 1_048_576.0
            ),
        );
        Ok(TrashDto {
            batch_id: r.batch_id.clone(),
            trashed: r.trashed.len(),
            bytes: r.bytes(),
            failed: r
                .failed
                .iter()
                .map(|(p, why)| format!("{}: {why}", p.display()))
                .collect(),
        })
    })
    .await
}

// ── empty folders (0.14.0) ──────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyRemoveDto {
    pub batch_id: String,
    pub removed: usize,
    pub failed: Vec<String>,
}

/// "빈 폴더 정리": remove the folders the user ticked, one batch (undo
/// makes them again). Only ever called from that click; each folder is
/// checked again first (`Mover::remove_empty_dirs`).
#[tauri::command]
pub async fn remove_empty_folders(app: AppHandle, paths: Vec<String>) -> CmdResult<EmptyRemoveDto> {
    crate::overview::invalidate();
    blocking(move || {
        let state = app.state::<AppState>();
        let inner = state.lock_inner();
        let mover = inner.mover.as_ref().ok_or("DB가 열려 있지 않습니다")?;
        let batch = tidy_core::engine::new_batch_id();
        let paths: Vec<std::path::PathBuf> = paths.into_iter().map(Into::into).collect();
        let r = mover.remove_empty_dirs(&paths, &batch);
        watch::emit_activity(
            &app,
            "info",
            Path::new(""),
            format!("빈 폴더 정리: {}개", r.removed.len()),
        );
        Ok(EmptyRemoveDto {
            batch_id: r.batch_id.clone(),
            removed: r.removed.len(),
            failed: r
                .failed
                .iter()
                .map(|(p, why)| format!("{}: {why}", p.display()))
                .collect(),
        })
    })
    .await
}

/// "휴지통 열기" (undo of a `trash` row points here). With
/// `TIDY_E2E_NO_OPEN` the request is only recorded (headless e2e).
#[tauri::command]
pub async fn open_trash() -> CmdResult<Option<String>> {
    if crate::about::e2e_record("trash") {
        return Ok(Some("trash".into()));
    }
    Ok(tidy_core::mover::open_trash())
}
