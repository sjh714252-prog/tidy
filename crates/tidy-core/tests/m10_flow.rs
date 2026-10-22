//! M10 (SPEC §14.3): work-context curation with a deterministic
//! MockProvider — two customers × two issues + three policy documents,
//! date merging within 14 days, the folder layout (in place and collected),
//! the analysis cache, corrections → dictionary suggestions, one-batch
//! apply and undo.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, RuleSet};
use tidy_core::config::DuplicatePolicy;
use tidy_core::curate::{ContextDict, ContextSuggestion, Curator, MoveEdit};
use tidy_core::mover::Mover;
use tidy_core::report::restructure::{self, Plan};
use tidy_core::store::Store;
use tidy_core::Config;

/// What the fake model says per file name:
/// (entity, issue, date, doc_kind, topic, confidence).
type Answer = (
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
    f64,
);

fn answers() -> HashMap<&'static str, Answer> {
    HashMap::from([
        // 바다정보 × 과금이슈: dates 10-20 and 10-22 → one folder (20261020).
        (
            "바다정보_과금오류_20261020.log",
            (
                Some("바다정보"),
                "과금이슈",
                Some("2026-10-20"),
                "로그",
                None,
                0.95,
            ),
        ),
        (
            "과금 안내문(바다).txt",
            (
                Some("바다"),
                "과금이슈",
                Some("2026-10-22"),
                "안내문서",
                None,
                0.9,
            ),
        ),
        // 바다정보 × 장애 (alias BADA).
        (
            "BADA_장애로그_20261105.log",
            (Some("BADA"), "장애", Some("2026-11-05"), "로그", None, 0.92),
        ),
        // 하늘통신 × 번호전환, × 정기점검.
        (
            "하늘통신_번호전환_회의록.txt",
            (
                Some("하늘통신"),
                "번호전환",
                Some("2026-09-01"),
                "회의록",
                None,
                0.88,
            ),
        ),
        (
            "SKY_전환대상.csv",
            (
                Some("SKY"),
                "번호전환",
                Some("2026-09-03"),
                "데이터",
                None,
                0.86,
            ),
        ),
        (
            "하늘통신_정기점검_결과.txt",
            (Some("하늘통신"), "정기점검", None, "안내문서", None, 0.9),
        ),
        // No customer: policy documents by topic.
        (
            "문자 발송 정책.txt",
            (None, "정책", None, "안내문서", Some("정책/문자 정책"), 0.8),
        ),
        (
            "개인정보 처리 방침.txt",
            (
                None,
                "정책",
                None,
                "안내문서",
                Some("정책/개인정보 정책"),
                0.8,
            ),
        ),
        (
            "보안 정책 v2.txt",
            (None, "정책", None, "안내문서", Some("정책/보안 정책"), 0.75),
        ),
        // Too unsure → 미분류.
        (
            "메모.txt",
            (None, "메모", None, "기타 자료", Some("메모/기타"), 0.3),
        ),
    ])
}

/// Files that arrive later while watching.
fn later() -> HashMap<&'static str, Answer> {
    HashMap::from([
        (
            "바다정보_과금_추가로그.log",
            (
                Some("바다정보"),
                "과금이슈",
                Some("2026-10-25"),
                "로그",
                None,
                0.95,
            ),
        ),
        (
            "바다정보_대체발송_중복건.txt",
            (
                Some("바다정보"),
                "대체발송 중복",
                Some("2026-10-20"),
                "안내문서",
                None,
                0.93,
            ),
        ),
        (
            "애매한 파일.txt",
            (Some("바다정보"), "과금이슈", None, "로그", None, 0.4),
        ),
    ])
}

fn mock() -> Arc<MockProvider> {
    let mut table = answers();
    table.extend(later());
    Arc::new(MockProvider::responder(move |_system, user| {
        let mut files = Vec::new();
        for line in user.lines() {
            let Some(rest) = line.strip_prefix('[') else {
                continue;
            };
            let (id, rest) = rest.split_once(']').unwrap();
            let name = rest
                .trim()
                .strip_prefix("이름: ")
                .and_then(|r| r.split(" | ").next())
                .unwrap();
            if let Some((e, i, d, k, t, c)) = table.get(name) {
                files.push(json!({
                    "id": id.parse::<u64>().unwrap(), "entity": e, "issue": i, "date": d,
                    "doc_kind": k, "topic": t, "confidence": c, "reason": format!("{name}에서 판단"),
                }));
            }
        }
        json!({ "files": files })
    }))
}

fn setup() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let desk = dunce::canonicalize(tmp.path()).unwrap().join("바탕 화면");
    fs::create_dir_all(&desk).unwrap();
    for name in answers().keys() {
        fs::write(desk.join(name), format!("{name} 본문")).unwrap();
    }
    (tmp, desk)
}

fn dict() -> ContextDict {
    let mut d = ContextDict::starter();
    d.add_entity("바다정보", &["BADA".into(), "바다".into()])
        .unwrap();
    d.add_entity("하늘통신", &["SKY".into()]).unwrap();
    d
}

fn rel(p: &Path, base: &Path) -> String {
    p.strip_prefix(base)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn to_of(plan: &Plan, name: &str) -> PathBuf {
    plan.moves
        .iter()
        .find(|m| m.from.file_name().unwrap() == name)
        .unwrap()
        .to
        .clone()
}

#[test]
fn curate_plan_layout_merge_cache_edit_apply_undo() {
    let (_tmp, desk) = setup();
    let cfg = Config::default();
    let provider = mock();
    let ai = AiClassifier::with_provider(provider.clone(), &cfg, &RuleSet::default()).unwrap();
    let cur = Curator::new(dict(), cfg.curate.clone(), true).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let store = mover.store();

    // Cost first: nothing cached yet.
    let inputs: Vec<_> = Curator::files_in(&desk, false, &[], &[])
        .iter()
        .map(|p| cur.input(p, Some(store)).unwrap())
        .collect();
    let est = cur.estimate(store, &inputs, "mock", "mock");
    assert_eq!((est.files, est.to_analyze, est.requests), (10, 10, 1));
    assert!(est.input_tokens > 0 && est.cost_usd == Some(0.0));

    let plan = cur.plan(&ai, store, &desk, &desk, false, &[], &[]).unwrap();
    assert_eq!(provider.prompts().len(), 1, "one batch for 10 files");
    let r = |n: &str| rel(&to_of(&plan, n), &desk);
    assert_eq!(
        r("바다정보_과금오류_20261020.log"),
        "바다정보/20261020_과금이슈/로그/바다정보_과금오류_20261020.log"
    );
    // Alias → dictionary name, date 10-22 merged into the 10-20 folder.
    assert_eq!(
        r("과금 안내문(바다).txt"),
        "바다정보/20261020_과금이슈/안내문서/과금 안내문(바다).txt"
    );
    assert_eq!(
        r("BADA_장애로그_20261105.log"),
        "바다정보/20261105_장애/로그/BADA_장애로그_20261105.log"
    );
    assert_eq!(
        r("SKY_전환대상.csv"),
        "하늘통신/20260901_번호전환/데이터/SKY_전환대상.csv"
    );
    assert_eq!(
        r("하늘통신_번호전환_회의록.txt"),
        "하늘통신/20260901_번호전환/회의록/하늘통신_번호전환_회의록.txt"
    );
    assert!(
        r("하늘통신_정기점검_결과.txt").starts_with("하늘통신/"),
        "no date in text → modified date"
    );
    assert_eq!(
        r("문자 발송 정책.txt"),
        "기타/정책/문자 정책/문자 발송 정책.txt"
    );
    assert_eq!(
        r("보안 정책 v2.txt"),
        "기타/정책/보안 정책/보안 정책 v2.txt"
    );
    // Unsure → stays (미분류), not part of the preview's moves.
    let memo = plan
        .moves
        .iter()
        .find(|m| m.from.ends_with("메모.txt"))
        .unwrap();
    assert!(memo.context.as_ref().unwrap().unclassified && memo.to == memo.from);
    assert!(
        plan.summary.contains("고객사 2곳") && plan.summary.contains("미분류 1개"),
        "{}",
        plan.summary
    );
    let note = to_of(&plan, "SKY_전환대상.csv");
    assert!(note.ends_with("SKY_전환대상.csv"));

    // Cache: the same files cost nothing the second time.
    let mut plan = cur.plan(&ai, store, &desk, &desk, false, &[], &[]).unwrap();
    assert_eq!(
        provider.prompts().len(),
        1,
        "answers come from the curation cache"
    );
    assert_eq!(cur.estimate(store, &inputs, "mock", "mock").to_analyze, 0);

    // The user assigns the memo to 하늘통신 / 정기점검 → it joins that folder.
    let memo_id = plan
        .moves
        .iter()
        .find(|m| m.from.ends_with("메모.txt"))
        .unwrap()
        .id;
    let sug = cur
        .edit_move(
            &mut plan,
            memo_id,
            &MoveEdit {
                entity: Some("SKY".into()),
                issue: Some("정기점검".into()),
                ..Default::default()
            },
            store,
        )
        .unwrap();
    assert_eq!(
        sug,
        vec![ContextSuggestion::AddIssueType {
            name: "정기점검".into()
        }],
        "정기점검 is a new issue type; SKY is a known alias"
    );
    let memo_to = rel(&to_of(&plan, "메모.txt"), &desk);
    let check_to = rel(&to_of(&plan, "하늘통신_정기점검_결과.txt"), &desk);
    assert_eq!(
        Path::new(&memo_to).parent().unwrap().parent(),
        Path::new(&check_to).parent().unwrap().parent(),
        "same customer/date_issue folder: {memo_to} vs {check_to}"
    );
    // A new customer the AI did not know → "add customer" suggestion.
    let pol = plan
        .moves
        .iter()
        .find(|m| m.from.ends_with("보안 정책 v2.txt"))
        .unwrap()
        .id;
    let sug = cur
        .edit_move(
            &mut plan,
            pol,
            &MoveEdit {
                entity: Some("구름소프트".into()),
                issue: Some("보안점검".into()),
                ..Default::default()
            },
            store,
        )
        .unwrap();
    assert!(sug.contains(&ContextSuggestion::AddEntity {
        name: "구름소프트".into()
    }));
    assert!(rel(&to_of(&plan, "보안 정책 v2.txt"), &desk).starts_with("구름소프트/"));
    // The correction is remembered for the next analysis.
    let again = cur.plan(&ai, store, &desk, &desk, false, &[], &[]).unwrap();
    assert!(rel(&to_of(&again, "메모.txt"), &desk).starts_with("하늘통신/"));

    // Apply (one batch) → files in place; undo → all back.
    let plan = restructure::save(store, plan).unwrap();
    let pv = restructure::preview(&plan, None).unwrap();
    assert_eq!(pv.files_moved, 10);
    assert!(
        pv.after.contains("바다정보") && pv.after.contains("20261020_과금이슈"),
        "{}",
        pv.after
    );
    let rep = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert_eq!(rep.moved.len(), 10, "{:?} {:?}", rep.skipped, rep.errors);
    assert!(desk
        .join("바다정보/20261020_과금이슈/안내문서/과금 안내문(바다).txt")
        .is_file());
    assert!(desk
        .join("기타/정책/문자 정책/문자 발송 정책.txt")
        .is_file());
    let batch = store.moves_in_batch(&rep.batch_id).unwrap();
    assert_eq!(batch.len(), 10);
    assert_eq!(
        batch[0].rule_name.as_deref(),
        Some(format!("업무 맥락 #{}", plan.id).as_str())
    );
    // A later file of the same issue joins the 20261020 folder.
    assert!(store
        .curation_dates("바다정보", "과금이슈")
        .unwrap()
        .contains(&chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()));
    let undo = mover.undo_batch(&rep.batch_id).unwrap();
    assert_eq!(undo.restored.len(), 10, "{:?}", undo.conflicts);
    for name in answers().keys() {
        assert!(desk.join(name).is_file(), "{name} back");
    }
}

#[test]
fn watch_time_placement_auto_into_existing_folder_or_ask() {
    use tidy_core::classify::Anchors;
    use tidy_core::config::WatchRoot;
    use tidy_core::engine::{new_batch_id, Action, Engine};
    use tidy_core::store::PendingSource;
    use tidy_core::watcher::IgnoreFilter;

    let (_tmp, desk) = setup();
    let cfg = Config::default();
    let cur = Arc::new(Curator::new(dict(), cfg.curate.clone(), true).unwrap());
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    // Organise what is there first (plan → apply), as the user would.
    let ai = AiClassifier::with_provider(mock(), &cfg, &RuleSet::default()).unwrap();
    let plan = cur
        .plan(&ai, mover.store(), &desk, &desk, false, &[], &[])
        .unwrap();
    let plan = restructure::save(mover.store(), plan).unwrap();
    restructure::apply(&mover, &plan, None, &[], None).unwrap();

    let watches = vec![WatchRoot {
        path: desk.clone(),
        ..Default::default()
    }];
    let engine = Engine::new(RuleSet::default(), mover, IgnoreFilter::new(&[]).unwrap())
        .with_anchors(Anchors::from_watches(&watches))
        .with_ai(AiClassifier::with_provider(mock(), &cfg, &RuleSet::default()).unwrap())
        .with_curator(cur);
    for name in later().keys() {
        fs::write(desk.join(name), "새 파일").unwrap();
    }
    // Same issue, 5 days later, folder exists, 95% ≥ 90% → moved on its own
    // into the 20261020 folder.
    let a = engine
        .handle(&desk.join("바다정보_과금_추가로그.log"), &new_batch_id())
        .unwrap();
    let Action::Moved { dst, .. } = &a else {
        panic!("{a:?}")
    };
    assert_eq!(
        rel(dst, &desk),
        "바다정보/20261020_과금이슈/로그/바다정보_과금_추가로그.log"
    );
    // A new issue of a known customer → asked, nothing moved.
    let f = desk.join("바다정보_대체발송_중복건.txt");
    let a = engine.handle(&f, &new_batch_id()).unwrap();
    let Action::Pending {
        reason,
        suggested,
        source,
        ..
    } = &a
    else {
        panic!("{a:?}")
    };
    assert_eq!(*source, PendingSource::Ai);
    assert!(
        reason
            .as_deref()
            .unwrap()
            .starts_with("바다정보의 새 이슈 '대체발송 중복'(20261020)으로 폴더를 만들까요?"),
        "{reason:?}"
    );
    assert_eq!(
        rel(suggested, &desk),
        "바다정보/20261020_대체발송 중복/안내문서"
    );
    assert!(f.exists());
    // Unsure → left alone (no rule either).
    let a = engine
        .handle(&desk.join("애매한 파일.txt"), &new_batch_id())
        .unwrap();
    assert!(matches!(a, Action::NoRule(_)), "{a:?}");
}

#[test]
fn collect_anchor_and_custom_template() {
    let (tmp, desk) = setup();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("정리");
    let mut cfg = Config::default();
    cfg.curate.template = "{root}/고객사/{entity}/{yyyy}/{issue}".into();
    let ai = AiClassifier::with_provider(mock(), &cfg, &RuleSet::default()).unwrap();
    let cur = Curator::new(dict(), cfg.curate.clone(), false).unwrap();
    let store = Store::open_in_memory().unwrap();
    let plan = cur
        .plan(&ai, &store, &desk, &root, false, &[], &[])
        .unwrap();
    assert_eq!(
        rel(&to_of(&plan, "BADA_장애로그_20261105.log"), &root),
        "고객사/바다정보/2026/장애/BADA_장애로그_20261105.log"
    );
    // Without send_content the prompt has names, never text.
    let inputs = cur.input(&desk.join("메모.txt"), Some(&store)).unwrap();
    assert!(inputs.excerpt.is_none());
    let bad = tidy_core::config::CurateCfg {
        template: "../밖".into(),
        ..Default::default()
    };
    assert!(Curator::new(dict(), bad, false).is_err());
}

#[test]
fn ai_failure_is_an_error_not_a_move() {
    let (_tmp, desk) = setup();
    let cfg = Config::default();
    let failing = Arc::new(MockProvider::sequence(vec![Err("offline".into())]));
    let ai = AiClassifier::with_provider(failing, &cfg, &RuleSet::default()).unwrap();
    let cur = Curator::new(dict(), cfg.curate.clone(), true).unwrap();
    let store = Store::open_in_memory().unwrap();
    assert!(cur
        .plan(&ai, &store, &desk, &desk, false, &[], &[])
        .is_err());
    assert_eq!(fs::read_dir(&desk).unwrap().count(), 10, "nothing moved");
    let _unused: Option<Value> = None;
}
