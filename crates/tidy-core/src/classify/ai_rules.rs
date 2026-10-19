//! "AI로 규칙 만들기" (SPEC §14.1): send the watch folders' tree summaries
//! (names, sizes, counts — never file contents) and get rule drafts back
//! as cards. Nothing is written: the editor shows the drafts and the user
//! saves (or drops) them.

use std::path::{Component, Path};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::ai::{AiCall, AiClassifier};
use super::rules_doc::{apply_cards, RuleCard};
use crate::ai::{prompts, AiError};
use crate::config::{OrganizeMode, RootKind, WatchRoot};
use crate::report::{restructure::tree_summary, scan, ScanOptions};
use crate::store::Store;

const RULES_MAX_TOKENS: u32 = 8_000;
const MAX_DRAFTS: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct RuleDraft {
    pub card: RuleCard,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RejectedDraft {
    pub name: String,
    pub dest: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleDrafts {
    pub summary: String,
    pub drafts: Vec<RuleDraft>,
    pub rejected: Vec<RejectedDraft>,
}

#[derive(Debug, Deserialize)]
struct RawDrafts {
    summary: String,
    rules: Vec<RawRule>,
}

#[derive(Debug, Deserialize)]
struct RawRule {
    name: String,
    #[serde(default)]
    ext: Vec<String>,
    #[serde(default)]
    name_contains: Option<String>,
    dest: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    reason: String,
}

/// Ask the AI for rule drafts for `watches` (local folders that exist).
pub fn draft_rules(
    ai: &AiClassifier,
    store: &Store,
    watches: &[WatchRoot],
    timeout: Duration,
) -> Result<RuleDrafts, AiError> {
    let mut folders = Vec::new();
    for w in watches
        .iter()
        .filter(|w| w.kind == RootKind::Local && w.path.is_dir())
    {
        let opts = ScanOptions {
            max_depth: Some(if w.recursive { 2 } else { 0 }),
            ..Default::default()
        };
        let Ok(res) = scan(&w.path, &opts) else {
            continue;
        };
        let mode = match w.mode {
            OrganizeMode::InPlace => "이 폴더 안에서 정리".to_string(),
            OrganizeMode::Collect => format!("{} 아래로 모으기", w.anchor().display()),
        };
        folders.push((w.path.display().to_string(), mode, tree_summary(&res.tree)));
    }
    if folders.is_empty() {
        return Err(AiError::BadResponse(
            "규칙을 만들 감시 폴더가 없습니다 (로컬 폴더를 먼저 추가하세요)".into(),
        ));
    }
    let user = prompts::rules_user(&folders);
    let schema = prompts::rules_schema();
    let raw: RawDrafts = ai.call(
        store,
        &AiCall {
            purpose: "rules",
            system: prompts::RULES_SYSTEM,
            user: &user,
            schema: &schema,
            max_tokens: RULES_MAX_TOKENS,
            timeout,
        },
        |v| {
            let raw: RawDrafts = serde_json::from_value(v)
                .map_err(|e| AiError::BadResponse(format!("rules do not match the schema: {e}")))?;
            let n = raw.rules.len();
            Ok((raw, Some(format!("{n} rules proposed"))))
        },
    )?;
    Ok(validate(raw))
}

/// A destination the model may propose: relative, no `..`, no `~`, a
/// valid template.
fn check_dest(dest: &str) -> Result<(), &'static str> {
    let d = dest.trim();
    if d.is_empty() {
        return Err("목적지가 비어 있음");
    }
    if d.starts_with('~') || d.contains(':') {
        return Err("절대 경로");
    }
    let p = Path::new(d);
    if p.components().any(|c| {
        matches!(
            c,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) || d.starts_with('/')
        || d.starts_with('\\')
    {
        return Err("절대 경로나 상위 폴더(..)");
    }
    Ok(())
}

fn validate(raw: RawDrafts) -> RuleDrafts {
    let mut drafts = Vec::new();
    let mut rejected = Vec::new();
    for r in raw.rules {
        let reject = |why: &str| RejectedDraft {
            name: r.name.clone(),
            dest: r.dest.clone(),
            reason: why.to_string(),
        };
        if drafts.len() >= MAX_DRAFTS {
            rejected.push(reject("규칙 개수 상한 초과"));
            continue;
        }
        if r.name.trim().is_empty() {
            rejected.push(reject("이름 없음"));
            continue;
        }
        if let Err(why) = check_dest(&r.dest) {
            rejected.push(reject(why));
            continue;
        }
        let card = RuleCard {
            name: r.name.trim().chars().take(60).collect(),
            ext: r
                .ext
                .iter()
                .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_alphanumeric()))
                .collect(),
            name_contains: r.name_contains.unwrap_or_default().trim().to_string(),
            dest: r.dest.trim().replace('\\', "/"),
            confirm: r.confirm,
            ..Default::default()
        };
        if card.ext.is_empty() && card.name_contains.is_empty() {
            rejected.push(reject("조건이 없어 모든 파일에 걸림"));
            continue;
        }
        // Must compile like any rule (unknown {variables}, …).
        if let Err(e) = apply_cards("", std::slice::from_ref(&card)) {
            rejected.push(reject(&e.to_string()));
            continue;
        }
        drafts.push(RuleDraft {
            card,
            reason: r.reason.trim().chars().take(300).collect(),
        });
    }
    RuleDrafts {
        summary: raw.summary.trim().to_string(),
        drafts,
        rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::MockProvider;
    use crate::classify::RuleSet;
    use crate::config::Config;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn drafts_are_validated_and_nothing_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let desk = tmp.path().join("바탕 화면");
        std::fs::create_dir_all(&desk).unwrap();
        std::fs::write(desk.join("견적서_A.pdf"), "x").unwrap();
        std::fs::write(desk.join("setup.exe"), "x").unwrap();
        let answer = json!({
            "summary": "문서와 설치 파일이 많습니다",
            "rules": [
                {"name": "견적서", "ext": ["PDF"], "name_contains": "견적서", "dest": "문서/견적서/{yyyy}", "confirm": false, "reason": "견적서가 반복됨"},
                {"name": "설치", "ext": [".exe"], "name_contains": null, "dest": "설치파일", "confirm": true, "reason": "설치 파일"},
                {"name": "밖으로", "ext": ["zip"], "name_contains": null, "dest": "/etc/x", "confirm": false, "reason": "x"},
                {"name": "상위", "ext": ["zip"], "name_contains": null, "dest": "../x", "confirm": false, "reason": "x"},
                {"name": "틀린 변수", "ext": ["zip"], "name_contains": null, "dest": "x/{year}", "confirm": false, "reason": "x"},
                {"name": "전부", "ext": [], "name_contains": null, "dest": "기타", "confirm": false, "reason": "x"}
            ]
        });
        let mock = Arc::new(MockProvider::new(answer));
        let mut cfg = Config::default();
        cfg.ai.enabled = true;
        let ai = AiClassifier::with_provider(mock.clone(), &cfg, &RuleSet::default()).unwrap();
        let store = Store::open_in_memory().unwrap();
        let w = WatchRoot {
            path: desk.clone(),
            ..Default::default()
        };
        let d = draft_rules(&ai, &store, &[w], Duration::from_secs(5)).unwrap();
        assert_eq!(d.drafts.len(), 2, "{:?}", d.rejected);
        assert_eq!(d.drafts[0].card.ext, ["pdf"]);
        assert_eq!(d.drafts[0].card.name_contains, "견적서");
        assert!(d.drafts[1].card.confirm);
        assert_eq!(d.rejected.len(), 4);
        // Only names/sizes went out, never file bodies.
        let (_, user) = &mock.prompts()[0];
        assert!(user.contains("견적서_A.pdf") && user.contains("바탕 화면"));
        assert_eq!(store.recent_ai_log(1).unwrap()[0].purpose, "rules");
        // Nothing on disk changed.
        assert_eq!(std::fs::read_dir(&desk).unwrap().count(), 2);
    }
}
