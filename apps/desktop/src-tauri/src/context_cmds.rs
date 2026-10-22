//! (M10) Work-context commands (SPEC §14.3): the context dictionary and
//! `[curate]` settings, "바탕화면에서 뽑기", cost estimate, the work-context
//! plan with per-item edits (→ dictionary suggestions). Plans reuse the
//! restructure preview/apply commands; nothing here moves a file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tidy_core::classify::{AiClassifier, Anchors, RuleSet};
use tidy_core::config::{write_curate_settings, CuratePatch};
use tidy_core::curate::{
    ContextCandidates, ContextDict, ContextSuggestion, CurateInput, Curator, Estimate, MoveEdit,
};
use tidy_core::report::restructure;
use tidy_core::store::Store;
use tidy_core::Config;

use crate::commands::{blocking, CmdResult};
use crate::report_cmds::PlanDto;
use crate::state::AppState;
use crate::watch;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn expand(p: &str) -> PathBuf {
    tidy_core::config::expand_home(Path::new(p.trim()))
}

fn ai_for(cfg: &Config) -> CmdResult<AiClassifier> {
    let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
    AiClassifier::from_config(cfg, &rules).map_err(|e| {
        format!("업무 맥락 정리는 AI가 필요합니다 — 설정 → AI 연결에서 켜 주세요 ({e})")
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurateDto {
    pub settings: CuratePatch,
    pub dict: ContextDict,
    // read-only
    #[serde(default)]
    pub context_path: String,
    #[serde(default)]
    pub context_exists: bool,
    /// Files analysed so far (cache rows).
    #[serde(default)]
    pub analysed: u64,
}

fn curate_dto(cfg: &Config, store: Option<&Store>) -> CmdResult<CurateDto> {
    let c = &cfg.curate;
    Ok(CurateDto {
        settings: CuratePatch {
            enabled: c.enabled,
            prefer: c.prefer,
            template: c.template.clone(),
            other_template: c.other_template.clone(),
            min_confidence: c.min_confidence,
        },
        dict: ContextDict::load(&c.context).map_err(err)?,
        context_path: c.context.display().to_string(),
        context_exists: c.context.is_file(),
        analysed: store.and_then(|s| s.curation_count().ok()).unwrap_or(0),
    })
}

#[tauri::command]
pub async fn get_curate_settings(app: AppHandle) -> CmdResult<CurateDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).ok();
        curate_dto(&cfg, store.as_ref())
    })
    .await
}

/// Save `[curate]` and the dictionary (the settings tab's one save button).
#[tauri::command]
pub async fn save_curate_settings(app: AppHandle, settings: CurateDto) -> CmdResult<CurateDto> {
    blocking(move || {
        for t in [
            &settings.settings.template,
            &settings.settings.other_template,
        ] {
            tidy_core::curate::FolderTemplate::parse(t).map_err(|e| format!("폴더 틀: {e}"))?;
        }
        let state = app.state::<AppState>();
        let cfg = state.config();
        let mut dict = settings.dict.clone();
        dict.normalize();
        dict.save(&cfg.curate.context).map_err(err)?;
        write_curate_settings(&state.config_path, &settings.settings).map_err(err)?;
        state.reload();
        watch::restart(&app);
        let cfg = state.config();
        let store = Store::open(&cfg.store.db_path()).ok();
        curate_dto(&cfg, store.as_ref())
    })
    .await
}

fn inputs_for(
    cfg: &Config,
    cur: &Curator,
    store: &Store,
    root: &Path,
    recursive: bool,
) -> Vec<CurateInput> {
    let protected: Vec<PathBuf> = cfg.report.protected.clone();
    Curator::files_in(root, recursive, &cfg.classify.ignore_globs, &protected)
        .iter()
        .filter_map(|p| cur.input(p, Some(store)).ok())
        .collect()
}

/// "파일 N개, 예상 토큰/비용" shown before analysing.
#[tauri::command]
pub async fn curate_estimate(app: AppHandle, path: String, recursive: bool) -> CmdResult<Estimate> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let cur = Curator::from_config(&cfg).map_err(err)?;
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let root = dunce::canonicalize(expand(&path)).map_err(err)?;
        let inputs = inputs_for(&cfg, &cur, &store, &root, recursive);
        let (provider, model) = cfg
            .ai
            .active()
            .and_then(|p| p.classify.as_ref())
            .map(|p| (p.provider.as_str().to_string(), p.model.clone()))
            .unwrap_or_default();
        Ok(cur.estimate(&store, &inputs, &provider, &model))
    })
    .await
}

/// Analyse the folder (AI) and save a work-context plan. Nothing moves.
#[tauri::command]
pub async fn plan_context(
    app: AppHandle,
    path: String,
    recursive: bool,
    protect: Vec<String>,
) -> CmdResult<PlanDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let ai = ai_for(&cfg)?;
        let cur = Curator::from_config(&cfg).map_err(err)?;
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let root = dunce::canonicalize(expand(&path)).map_err(err)?;
        let anchors = Anchors::from_config(&cfg);
        let anchor = anchors.for_path(&root).unwrap_or(&root).to_path_buf();
        let mut protected: Vec<PathBuf> = cfg.report.protected.clone();
        protected.extend(
            protect
                .iter()
                .filter(|p| !p.trim().is_empty())
                .map(|p| expand(p)),
        );
        let plan = cur
            .plan(
                &ai,
                &store,
                &root,
                &anchor,
                recursive,
                &cfg.classify.ignore_globs,
                &protected,
            )
            .map_err(err)?;
        let plan = restructure::save(&store, plan).map_err(err)?;
        let ids: Vec<u32> = classified(&plan);
        let preview = restructure::preview(&plan, Some(&ids)).map_err(err)?;
        Ok(PlanDto { plan, preview })
    })
    .await
}

/// Items that would move (not 미분류, not already in place).
fn classified(plan: &restructure::Plan) -> Vec<u32> {
    plan.moves
        .iter()
        .filter(|m| m.from != m.to)
        .map(|m| m.id)
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionDto {
    pub suggestion: ContextSuggestion,
    pub label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditDto {
    pub plan: restructure::Plan,
    pub preview: restructure::Preview,
    /// What the dictionary could learn from this change (confirm to save).
    pub suggestions: Vec<SuggestionDto>,
}

/// The user changed customer / issue / kind of one item in the preview.
#[tauri::command]
pub async fn edit_context_move(
    app: AppHandle,
    id: i64,
    move_id: u32,
    edit: MoveEdit,
    selected: Vec<u32>,
) -> CmdResult<EditDto> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let cur = Curator::from_config(&cfg).map_err(err)?;
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let mut plan = restructure::load(&store, id).map_err(err)?;
        if plan.applied_batch.is_some() {
            return Err("이미 적용한 제안은 고칠 수 없습니다".into());
        }
        let suggestions = cur
            .edit_move(&mut plan, move_id, &edit, &store)
            .map_err(err)?;
        let json = serde_json::to_string(&plan).map_err(err)?;
        store.update_plan_json(plan.id, &json).map_err(err)?;
        let mut ids = selected;
        if !ids.contains(&move_id) {
            ids.push(move_id);
        }
        ids.retain(|i| plan.moves.iter().any(|m| m.id == *i && m.from != m.to));
        let preview = restructure::preview(&plan, Some(&ids)).map_err(err)?;
        Ok(EditDto {
            plan,
            preview,
            suggestions: suggestions
                .into_iter()
                .map(|s| SuggestionDto {
                    label: s.label(),
                    suggestion: s,
                })
                .collect(),
        })
    })
    .await
}

/// Write confirmed dictionary suggestions to context.toml.
#[tauri::command]
pub async fn apply_context_suggestions(
    app: AppHandle,
    suggestions: Vec<ContextSuggestion>,
) -> CmdResult<CurateDto> {
    blocking(move || {
        let state = app.state::<AppState>();
        let cfg = state.config();
        let mut dict = ContextDict::load(&cfg.curate.context).map_err(err)?;
        for s in &suggestions {
            dict.apply(s).map_err(err)?;
        }
        dict.save(&cfg.curate.context).map_err(err)?;
        let store = Store::open(&cfg.store.db_path()).ok();
        curate_dto(&cfg, store.as_ref())
    })
    .await
}

/// "바탕화면에서 뽑기": candidates from the watch folders (nothing saved).
#[tauri::command]
pub async fn extract_context(app: AppHandle) -> CmdResult<ContextCandidates> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let ai = ai_for(&cfg)?;
        let cur = Curator::from_config(&cfg).map_err(err)?;
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        let mut inputs = Vec::new();
        for w in &cfg.watches {
            if let Ok(root) = dunce::canonicalize(&w.path) {
                inputs.extend(inputs_for(&cfg, &cur, &store, &root, w.recursive));
            }
        }
        if inputs.is_empty() {
            return Err("감시 폴더에 파일이 없습니다".into());
        }
        cur.extract_context(&ai, &store, &inputs).map_err(err)
    })
    .await
}

/// Customer / issue pairs seen so far (drop-down choices in the preview).
#[tauri::command]
pub async fn context_groups(app: AppHandle) -> CmdResult<Vec<(String, String)>> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let store = Store::open(&cfg.store.db_path()).map_err(err)?;
        store.curation_groups().map_err(err)
    })
    .await
}
