//! 0.13.0 (SPEC §14.19): the tree Tidy builds stays easy to read.
//! - a new kind (`로그/`) or sub-topic folder only when enough items go
//!   there (`min_folder_items`), existing folders always used;
//! - a new file whose name names a customer goes to that customer's folder
//!   before the type rules (`문서/…`), and to the rules when that fails.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, Anchors, RuleSet};
use tidy_core::config::{CurateCfg, DuplicatePolicy, WatchRoot};
use tidy_core::curate::{ContextDict, Curator};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::report::restructure::Plan;
use tidy_core::store::Store;
use tidy_core::watcher::IgnoreFilter;
use tidy_core::Config;

/// (entity, issue, date, doc_kind, topic, confidence) per file name.
type Answer = (
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
    &'static str,
    Option<&'static str>,
    f64,
);

fn answers() -> HashMap<&'static str, Answer> {
    let log = (
        Some("가람정보"),
        "과금",
        Some("2026-10-20"),
        "로그",
        None,
        0.95,
    );
    let dev = |t| (None, "개발", None, "자료", Some(t), 0.9);
    HashMap::from([
        // Three logs of one issue → their own 로그/ folder.
        ("과금로그1.log", log),
        ("과금로그2.log", log),
        ("과금로그3.log", log),
        // One notice of the same issue → straight into the issue folder.
        (
            "과금 안내.txt",
            (
                Some("가람정보"),
                "과금",
                Some("2026-10-21"),
                "안내문서",
                None,
                0.9,
            ),
        ),
        // Sub-topics: three API docs keep theirs; the one-off ones don't.
        ("api1.md", dev("개발/API 문서")),
        ("api2.md", dev("개발/API 문서")),
        ("api3.md", dev("개발/API 문서")),
        ("예제.go", dev("개발/소스 예제")),
        ("샘플.go", dev("개발/샘플 코드")),
        // Watch time.
        (
            "가람정보_견적서.pdf",
            (
                Some("가람정보"),
                "과금",
                Some("2026-10-22"),
                "계약",
                None,
                0.95,
            ),
        ),
    ])
}

fn mock() -> Arc<MockProvider> {
    let table = answers();
    Arc::new(MockProvider::responder(move |_system, user| {
        let mut files = Vec::new();
        for line in user.lines() {
            let Some(rest) = line.strip_prefix('[') else {
                continue;
            };
            let (id, rest) = rest.split_once(']').unwrap();
            let name = rest
                .trim()
                .strip_prefix("파일: ")
                .and_then(|r| r.split(" | ").next())
                .unwrap_or_default();
            if let Some((e, i, d, k, t, c)) = table.get(name) {
                files.push(json!({
                    "id": id.parse::<u64>().unwrap(), "kind": "file", "entity": e, "issue": i,
                    "date": d, "doc_kind": k, "topic": t, "confidence": c, "reason": "테스트",
                }));
            }
        }
        json!({ "files": files })
    }))
}

fn dict() -> ContextDict {
    let mut d = ContextDict::starter();
    d.add_entity("가람정보", &["GARA".into()]).unwrap();
    d.add_entity("KT", &[]).unwrap();
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

fn desk_with(names: &[&str]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let org = base.join("정리");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(&org).unwrap();
    for n in names {
        fs::write(desk.join(n), format!("{n} 본문")).unwrap();
    }
    (tmp, desk, org)
}

const SWEEP: &[&str] = &[
    "과금로그1.log",
    "과금로그2.log",
    "과금로그3.log",
    "과금 안내.txt",
    "api1.md",
    "api2.md",
    "api3.md",
    "예제.go",
    "샘플.go",
];

#[test]
fn small_new_folders_fold_into_the_folder_above() {
    let (_tmp, desk, org) = desk_with(SWEEP);
    let cfg = Config::default();
    assert_eq!(cfg.curate.min_folder_items, 3);
    let cur = Curator::new(dict(), cfg.curate.clone(), true).unwrap();
    let ai = AiClassifier::with_provider(mock(), &cfg, &RuleSet::default()).unwrap();
    let store = Store::open_in_memory().unwrap();
    let nf = IgnoreFilter::new(&[]).unwrap();
    let plan = cur.plan(&ai, &store, &desk, &org, &nf, &[], &[]).unwrap();
    let r = |n: &str| rel(&to_of(&plan, n), &org);
    assert_eq!(
        r("과금로그1.log"),
        "가람정보/20261020_과금/로그/과금로그1.log"
    );
    assert_eq!(
        r("과금로그3.log"),
        "가람정보/20261020_과금/로그/과금로그3.log"
    );
    assert_eq!(r("과금 안내.txt"), "가람정보/20261020_과금/과금 안내.txt");
    assert_eq!(r("api2.md"), "기타/개발/API 문서/api2.md");
    // Two one-off sub-topics → no "소스 예제/", "샘플 코드/" folders.
    assert_eq!(r("예제.go"), "기타/개발/예제.go");
    assert_eq!(r("샘플.go"), "기타/개발/샘플.go");

    // An existing kind folder is used even for one file.
    fs::create_dir_all(org.join("가람정보/20261020_과금/안내문서")).unwrap();
    let mut plan = plan;
    cur.relayout(&mut plan, Some(&store));
    assert_eq!(
        rel(&to_of(&plan, "과금 안내.txt"), &org),
        "가람정보/20261020_과금/안내문서/과금 안내.txt"
    );

    // min_folder_items = 1: the old, always-detailed layout.
    let every = Curator::new(
        dict(),
        CurateCfg {
            min_folder_items: 1,
            ..cfg.curate.clone()
        },
        true,
    )
    .unwrap();
    let plan = every.plan(&ai, &store, &desk, &org, &nf, &[], &[]).unwrap();
    assert_eq!(
        rel(&to_of(&plan, "예제.go"), &org),
        "기타/개발/소스 예제/예제.go"
    );
}

#[test]
fn customer_hint_needs_a_real_name() {
    let cur = Curator::new(dict(), CurateCfg::default(), true).unwrap();
    assert_eq!(
        cur.customer_hint("가람정보_견적서.pdf", None).as_deref(),
        Some("가람정보")
    );
    assert_eq!(
        cur.customer_hint("gara 요청 2026.xlsx", None).as_deref(),
        Some("가람정보"),
        "alias, any case"
    );
    // "KT" is too short to look for inside other words.
    assert_eq!(cur.customer_hint("market_report.pdf", None), None);
    assert_eq!(cur.customer_hint("견적서.pdf", None), None);
}

fn engine(desk: &Path, org: &Path, provider: Arc<MockProvider>) -> Engine {
    let cfg = Config::default();
    let rules = RuleSet::from_toml(
        "[[rule]]\nname = \"문서\"\nmatch = { ext = [\"pdf\"] }\ndest = \"문서/{yyyy}\"\n",
    )
    .unwrap();
    let watches = vec![WatchRoot {
        path: desk.to_path_buf(),
        mode: tidy_core::config::OrganizeMode::Collect,
        organize_root: Some(org.to_path_buf()),
        ..Default::default()
    }];
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let cur = Curator::new(dict(), cfg.curate.clone(), true).unwrap();
    Engine::new(rules.clone(), mover, IgnoreFilter::new(&[]).unwrap())
        .with_anchors(Anchors::from_watches(&watches))
        .with_ai(AiClassifier::with_provider(provider, &cfg, &rules).unwrap())
        .with_curator(Arc::new(cur))
}

#[test]
fn a_customer_file_goes_to_the_customer_before_the_type_rule() {
    let (_tmp, desk, org) = desk_with(&["가람정보_견적서.pdf", "견적서.pdf"]);
    fs::create_dir_all(org.join("가람정보/20261020_과금")).unwrap();
    let e = engine(&desk, &org, mock());

    let a = e
        .handle(&desk.join("가람정보_견적서.pdf"), &new_batch_id())
        .unwrap();
    let Action::Moved { dst, rule, .. } = &a else {
        panic!("{a:?}")
    };
    assert_eq!(rule, "업무 맥락");
    assert_eq!(
        rel(dst, &org),
        "가람정보/20261020_과금/가람정보_견적서.pdf",
        "not 문서/…, and no 계약/ folder for one file"
    );
    // No customer in the name → the type rule, no AI call needed.
    let a = e.handle(&desk.join("견적서.pdf"), &new_batch_id()).unwrap();
    let Action::Moved { dst, rule, .. } = &a else {
        panic!("{a:?}")
    };
    assert_eq!(rule, "문서");
    assert!(rel(dst, &org).starts_with("문서/"), "{dst:?}");
}

#[test]
fn customer_first_falls_back_to_the_rules_when_the_ai_fails() {
    let (_tmp, desk, org) = desk_with(&["가람정보_견적서.pdf"]);
    let failing = Arc::new(MockProvider::sequence(vec![Err("offline".into())]));
    let e = engine(&desk, &org, failing);
    let a = e
        .handle(&desk.join("가람정보_견적서.pdf"), &new_batch_id())
        .unwrap();
    let Action::Moved { dst, rule, .. } = &a else {
        panic!("{a:?}")
    };
    assert_eq!(rule, "문서");
    assert!(rel(dst, &org).starts_with("문서/"), "{dst:?}");
}
