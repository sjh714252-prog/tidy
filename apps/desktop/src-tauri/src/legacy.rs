//! (0.11.2) Startup fix for the first example rules' absolute destinations
//! (`~/Documents/Tidy/…`): rewrite them to follow the organize location,
//! then propose moving what they already put there (saved plans shown on
//! the home tab as "적용하지 않은 미리보기"; nothing moves until applied).

use tauri::{AppHandle, Manager};
use tidy_core::report::relocate::{is_bad_relocation, migrate_rules_text, plan_relocations};
use tidy_core::report::restructure::PlanSource;
use tidy_core::store::Store;

use crate::state::AppState;
use crate::watch;

/// Rewrite the rules (before the watch loop starts), then plan relocations
/// in the background.
pub fn run(app: &AppHandle) {
    let cfg = app.state::<AppState>().config();
    if !cfg.onboarded() || cfg.watches.is_empty() {
        return;
    }
    let path = cfg.classify.rules.clone();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let text = match migrate_rules_text(&text) {
        Ok(Some((out, fixes))) => {
            let backup = path.with_extension("toml.bak-0.11.2");
            if let Err(e) = std::fs::write(&backup, &text) {
                tracing::warn!("rules backup failed, rules left as they were: {e}");
                return;
            }
            if let Err(e) = std::fs::write(&path, &out) {
                tracing::warn!("rules update failed: {e}");
                return;
            }
            for f in &fixes {
                tracing::info!(rule = %f.rule, old = %f.old, new = %f.new, "legacy rule destination now follows the organize location");
            }
            watch::emit_activity(
                app,
                "info",
                &path,
                format!(
                    "예전 예시 규칙 {}개({})가 정리 위치를 무시하고 있어 정리 위치 기준으로 바꿨습니다 (이전 파일: {})",
                    fixes.len(),
                    fixes.iter().map(|f| f.rule.as_str()).collect::<Vec<_>>().join(", "),
                    backup.display()
                ),
            );
            out
        }
        Ok(None) => text,
        Err(e) => {
            tracing::warn!("rules not checked for legacy destinations: {e}");
            return;
        }
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(store) = Store::open(&cfg.store.db_path()) else {
            return;
        };
        // (0.12.4) Relocation previews made for a development project
        // (`Documents\tidy` = a git clone, same as `Documents\Tidy` on
        // Windows: crates/, apps/, .git/ …) are dropped, never applied.
        if let Ok(old) = store.recent_plans(200) {
            for p in old.iter().filter(|p| {
                p.source == PlanSource::Relocate.as_str()
                    && p.applied_batch.is_none()
                    && is_bad_relocation(&p.root)
            }) {
                if store.discard_plan(p.id).unwrap_or(false) {
                    tracing::info!(id = p.id, root = %p.root.display(), "dropped relocation preview of a development project");
                }
            }
        }
        match plan_relocations(&cfg, &store, &text) {
            Ok(plans) => {
                for p in &plans {
                    tracing::info!(id = p.id, root = %p.root.display(), moves = p.moves.len(), "relocation plan saved");
                    watch::emit_activity(&app, "info", &p.root, p.summary.clone());
                }
            }
            Err(e) => tracing::warn!("relocation plans failed: {e}"),
        }
    });
}
