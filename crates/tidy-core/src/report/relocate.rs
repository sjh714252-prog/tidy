//! (0.11.2) The first example rules (M1) had absolute destinations —
//! `~/Documents/Tidy/문서/{yyyy}-{mm}`, `~/Pictures/Screenshots/{yyyy}/{mm}`,
//! `~/Downloads/Installers`, `~/Downloads/압축 파일`. Configs that kept them
//! sent those files there even after an organize location (`D:\정리`) was
//! chosen ("규칙 N개가 정리 위치를 무시합니다").
//!
//! [`migrate_rules_text`] rewrites exactly those legacy destinations to the
//! relative form (`문서/{yyyy}-{mm}`, `캡처/{yyyy}/{mm}`, …) so they follow
//! the organize location. [`plan_relocations`] then proposes moving what
//! Tidy already put in the old places to where the rules would put it now:
//! a saved plan per old folder, applied only on approval as one undoable
//! batch ([`super::restructure::apply`]). Nothing here moves a file.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::classify::rules_doc::is_absolute_dest;
use crate::classify::{apply_cards, parse_cards, Anchors, RuleError};
use crate::config::{expand_home, Config};
use crate::store::Store;
use crate::watcher::IgnoreFilter;

use super::restructure::{save, Plan, PlanError, PlanSource, PlannedMove};

/// A rule destination that was rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyFix {
    pub rule: String,
    pub old: String,
    pub new: String,
}

/// Legacy base folders (relative to home) → relative base under the
/// organize location. `Documents/Tidy/<X>` keeps `<X>`.
const LEGACY_BASES: &[(&str, &str)] = &[
    ("Pictures/Screenshots", "캡처"),
    ("Downloads/Installers", "설치파일"),
    ("Downloads/압축 파일", "압축 파일"),
];
const TIDY_HOME: &str = "Documents/Tidy";

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Static part of a destination (before the first `{var}`), "/"-separated,
/// without a trailing "/"; and the rest (starting at the separator before
/// the first variable, or empty).
fn split_static(dest: &str) -> (String, String) {
    let d = dest.trim().replace('\\', "/");
    let cut = match d.find('{') {
        Some(i) => d[..i].rfind('/').map(|j| j + 1).unwrap_or(0),
        None => d.len(),
    };
    let (base, rest) = d.split_at(cut);
    (
        base.trim_end_matches('/').to_string(),
        rest.trim_start_matches('/').to_string(),
    )
}

fn same(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// For a legacy absolute destination: (old absolute base folder, new
/// relative base). `None` for anything else.
pub fn legacy_base(dest: &str) -> Option<(PathBuf, String)> {
    if !is_absolute_dest(dest) {
        return None;
    }
    let home = home()?;
    let (base, _) = split_static(dest);
    let abs = expand_home(Path::new(&base));
    for (old, new) in LEGACY_BASES {
        if same(&abs, &home.join(old)) {
            return Some((abs, (*new).to_string()));
        }
    }
    let tidy = home.join(TIDY_HOME);
    if let Some(parent) = abs.parent() {
        if same(parent, &tidy) {
            let name = abs.file_name()?.to_string_lossy().into_owned();
            return Some((abs, name));
        }
    }
    None
}

/// The relative destination a legacy one becomes (`None` if not legacy).
pub fn legacy_relative(dest: &str) -> Option<String> {
    let (_, new_base) = legacy_base(dest)?;
    let (_, rest) = split_static(dest);
    Some(if rest.is_empty() {
        new_base
    } else {
        format!("{new_base}/{rest}")
    })
}

/// Rewrite legacy absolute destinations in `rules.toml` text. `None` when
/// nothing changes. Everything else in the file is kept as it was.
pub fn migrate_rules_text(text: &str) -> Result<Option<(String, Vec<LegacyFix>)>, RuleError> {
    let mut cards = parse_cards(text)?;
    let mut fixes = Vec::new();
    for c in &mut cards {
        if let Some(new) = legacy_relative(&c.dest) {
            fixes.push(LegacyFix {
                rule: c.name.clone(),
                old: c.dest.clone(),
                new: new.clone(),
            });
            c.dest = new;
        }
    }
    if fixes.is_empty() {
        return Ok(None);
    }
    Ok(Some((apply_cards(text, &cards)?, fixes)))
}

/// Every legacy folder (absolute) with its new relative base, existing or not.
fn legacy_folders() -> Vec<(PathBuf, String)> {
    let Some(home) = home() else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, String)> = LEGACY_BASES
        .iter()
        .map(|(o, n)| (home.join(o), (*n).to_string()))
        .collect();
    if let Ok(rd) = std::fs::read_dir(home.join(TIDY_HOME)) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = e.file_name().to_string_lossy().into_owned();
                out.push((e.path(), name));
            }
        }
    }
    out
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Files Tidy put in legacy folders → where they go now, one saved plan
/// per folder that has any. Skips a folder a current rule still sends files
/// to (`rules_text`), and one that already has an unapplied relocation
/// plan. Inside `~/Documents/Tidy` (Tidy's own old folder) every file goes;
/// elsewhere only files `move_log` says Tidy moved there. Files go under
/// the anchor of the watch folder they came from, else the first organize
/// location. Returns the saved plans.
pub fn plan_relocations(
    cfg: &Config,
    store: &Store,
    rules_text: &str,
) -> Result<Vec<Plan>, PlanError> {
    let anchors = Anchors::from_config(cfg);
    let fallback = super::backlog::organize_roots(&cfg.watches)
        .into_iter()
        .next();
    let still_used: Vec<PathBuf> = parse_cards(rules_text)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| legacy_base(&c.dest).map(|(abs, _)| abs))
        .collect();
    let open: Vec<PathBuf> = store
        .recent_plans(50)?
        .into_iter()
        .filter(|p| p.source == PlanSource::Relocate.as_str() && p.applied_batch.is_none())
        .map(|p| p.root)
        .collect();
    let filter = cfg
        .ignore_filter()
        .unwrap_or_else(|_| IgnoreFilter::new(&[]).expect("empty filter"));
    let tidy_home = home().map(|h| h.join(TIDY_HOME));
    let mut plans = Vec::new();
    for (old, new_base) in legacy_folders() {
        if !old.is_dir()
            || still_used.iter().any(|u| same(u, &old))
            || open.iter().any(|r| same(r, &old))
        {
            continue;
        }
        let all = tidy_home.as_deref().is_some_and(|t| old.starts_with(t));
        let mut moves = Vec::new();
        for e in walkdir::WalkDir::new(&old)
            .min_depth(1)
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = e.path();
            if !e.file_type().is_file() || filter.is_ignored(path) {
                continue;
            }
            let logged = store.active_move_to(path)?;
            if logged.is_none() && !all {
                continue;
            }
            let anchor = logged
                .as_ref()
                .and_then(|m| anchors.for_path(&m.src_path).map(Path::to_path_buf))
                .or_else(|| fallback.clone());
            let Some(anchor) = anchor else { continue };
            let Ok(rel) = path.strip_prefix(&old) else {
                continue;
            };
            let to = anchor.join(&new_base).join(rel);
            if same(&to, path) {
                continue;
            }
            let mut warnings = Vec::new();
            if to.exists() {
                warnings.push("목적지에 같은 이름이 있어 '(1)'처럼 번호가 붙습니다".into());
            }
            moves.push(PlannedMove {
                id: moves.len() as u32 + 1,
                from: path.to_path_buf(),
                to,
                is_dir: false,
                files: 1,
                bytes: e.metadata().map(|m| m.len()).unwrap_or(0),
                reason: "예전 규칙 위치 → 정리 위치".into(),
                warnings,
                context: None,
                encrypt: false,
            });
        }
        if moves.is_empty() {
            continue;
        }
        let n = moves.len();
        let plan = Plan {
            id: 0,
            root: old.clone(),
            source: PlanSource::Relocate,
            created_at: now(),
            summary: format!(
                "예전 규칙이 정리 위치 대신 {}에 둔 파일 {n}개를 지금 규칙대로 정리 위치의 '{new_base}' 아래로 옮깁니다.",
                old.display()
            ),
            folders: Vec::new(),
            moves,
            rejected: Vec::new(),
            applied_batch: None,
            anchor: None,
        };
        plans.push(save(store, plan)?);
    }
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_destinations_become_relative() {
        assert_eq!(
            legacy_relative("~/Documents/Tidy/문서/{yyyy}-{mm}").as_deref(),
            Some("문서/{yyyy}-{mm}")
        );
        assert_eq!(
            legacy_relative("~/Documents/Tidy/테스트 결과/{yyyy}/{mm}").as_deref(),
            Some("테스트 결과/{yyyy}/{mm}")
        );
        assert_eq!(
            legacy_relative("~/Pictures/Screenshots/{yyyy}/{mm}").as_deref(),
            Some("캡처/{yyyy}/{mm}")
        );
        assert_eq!(
            legacy_relative("~/Downloads/Installers").as_deref(),
            Some("설치파일")
        );
        assert_eq!(
            legacy_relative("~/Downloads/압축 파일").as_deref(),
            Some("압축 파일")
        );
        let home = dirs::home_dir().unwrap();
        let abs = home.join("Documents").join("Tidy").join("문서");
        assert_eq!(
            legacy_relative(&format!("{}/{{yyyy}}", abs.display())).as_deref(),
            Some("문서/{yyyy}")
        );
        // The user's own absolute folders and relative ones stay.
        for d in [
            "D:/정리/문서",
            "~/Documents/업무/{yyyy}",
            "~/Downloads",
            "문서/{yyyy}-{mm}",
            "~/Documents/Tidy",
        ] {
            assert_eq!(legacy_relative(d), None, "{d}");
        }
    }

    #[test]
    fn migrate_keeps_other_rules_and_keys() {
        let text = r#"# 내 규칙
[[rule]]
name = "스크린샷"
match = { ext = ["png"], name_regex = "^스크린샷" }
dest = "~/Pictures/Screenshots/{yyyy}/{mm}"

[[rule]]
name = "내 폴더"
match = { ext = ["log"] }
dest = "D:/로그"

[[rule]]
name = "문서"
match = { ext = ["pdf", "docx"] }
dest = "~/Documents/Tidy/문서/{yyyy}-{mm}"
"#;
        let (out, fixes) = migrate_rules_text(text).unwrap().unwrap();
        assert_eq!(fixes.len(), 2);
        assert!(out.contains("# 내 규칙"));
        assert!(out.contains(r#"dest = "캡처/{yyyy}/{mm}""#), "{out}");
        assert!(out.contains(r#"dest = "문서/{yyyy}-{mm}""#), "{out}");
        assert!(out.contains(r#"dest = "D:/로그""#));
        assert!(out.contains("^스크린샷"));
        assert!(migrate_rules_text(&out).unwrap().is_none());
    }
}
