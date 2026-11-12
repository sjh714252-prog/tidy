//! M19 (0.14.0, SPEC §14.20) "정리 위치 점검": the organize root Tidy has
//! been filling is looked at as a whole and re-sorted — split folders of
//! one customer / issue merged, a customer folder parked under 기타 moved
//! back, small folders folded, loose top files placed by the rules and
//! (only when asked) customer files outside the customer folders placed by
//! the AI. One plan, one undoable batch; without AI nothing blocks.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, RuleSet};
use tidy_core::config::DuplicatePolicy;
use tidy_core::curate::{ContextDict, Curator};
use tidy_core::engine::new_batch_id;
use tidy_core::mover::{MoveRequest, Mover};
use tidy_core::report::restructure::{self, Plan, PlanSource};
use tidy_core::report::review::{review, ReviewOptions};
use tidy_core::store::{DecidedBy, Store};
use tidy_core::watcher::IgnoreFilter;
use tidy_core::Config;

fn put(mover: &Mover, src: &Path, dest_dir: &Path) {
    mover
        .move_file(&MoveRequest {
            src: src.to_path_buf(),
            dest_dir: dest_dir.to_path_buf(),
            batch_id: new_batch_id(),
            decided_by: DecidedBy::Restructure,
            rule_name: Some("업무 맥락".into()),
            ai_confidence: Some(0.9),
            new_name: None,
            encrypt: false,
            on_duplicate: None,
        })
        .unwrap();
}

fn rel(p: &Path, base: &Path) -> String {
    p.strip_prefix(base)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn lines(plan: &Plan, org: &Path) -> Vec<String> {
    let mut v: Vec<String> = plan
        .moves
        .iter()
        .map(|m| format!("{} -> {}", rel(&m.from, org), rel(&m.to, org)))
        .collect();
    v.sort();
    v
}

fn dict() -> ContextDict {
    let mut d = ContextDict::starter();
    d.add_entity("가람정보", &[]).unwrap();
    d
}

/// The organize root after a while of use (every file put there by Tidy
/// unless noted).
fn setup() -> (tempfile::TempDir, PathBuf, Mover) {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let org = base.join("정리");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(&org).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let mut n = 0;
    let mut file = |dir: &str, name: &str| {
        n += 1;
        let src = desk.join(format!("{n}")).join(name);
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, format!("{name} {n}")).unwrap();
        put(&mover, &src, &org.join(dir));
    };
    // One customer in two spellings; the same issue three times.
    file("KT스마트메시지/20261014_과금", "a1.log");
    file("KT스마트메시지/20261014_과금", "a2.log");
    file("KT스마트메시지/20261020_과금", "b.log");
    file("KT 스마트메시지/20261016_과금", "c.txt");
    file("KT 스마트메시지/20261101_장애", "d.txt");
    // A customer folder parked under 기타, next to a real topic.
    file("가람정보/20261020_과금/로그", "l1.log");
    file("가람정보/20261020_과금/로그", "l2.log");
    file("가람정보/20261020_과금/로그", "l3.log");
    file("기타/가람정보", "메모.txt");
    file("기타/개발", "예제.go");
    // A small kind folder (folds).
    file("가람정보/20261020_과금/안내문서", "안내.txt");
    // A customer's PDF that went to the type rules' folder.
    file("문서/2026", "가람정보_견적서.pdf");
    file("문서/2026", "보고서.pdf");
    // Loose at the top: one a rule takes, one nobody does.
    fs::write(org.join("설치.exe"), "exe").unwrap();
    fs::write(org.join("메모2.txt"), "메모").unwrap();
    // A code project inside the root: never touched.
    fs::create_dir_all(org.join("tidy/src")).unwrap();
    fs::write(org.join("tidy/Cargo.toml"), "[package]").unwrap();
    fs::write(org.join("tidy/src/main.rs"), "fn main(){}").unwrap();
    (tmp, org, mover)
}

fn rules() -> RuleSet {
    RuleSet::from_toml("[[rule]]\nname='설치 파일'\nmatch={ext=['exe']}\ndest='설치파일'\n")
        .unwrap()
}

#[test]
fn review_merges_folds_and_places_without_ai() {
    let (_tmp, org, mover) = setup();
    let cfg = Config::default();
    let store = mover.store();
    let cur = Curator::new(dict(), cfg.curate.clone(), true).unwrap();
    let rules = rules();
    let nf = IgnoreFilter::new(&[]).unwrap();
    let mut o = ReviewOptions::base(&cfg, &org, Vec::new());
    o.rules = Some(&rules);
    o.filter = Some(&nf);
    o.curator = Some(&cur);
    assert_eq!(o.other_dir, "기타");

    let r = review(&org, store, &o).unwrap();
    let plan = r.plan.clone().expect("a plan");
    assert_eq!(plan.source, PlanSource::Review);
    assert_eq!(
        lines(&plan, &org),
        vec![
            "KT 스마트메시지/20261016_과금 -> KT스마트메시지/20261014_과금",
            "KT 스마트메시지/20261101_장애 -> KT스마트메시지/20261101_장애",
            "KT스마트메시지/20261020_과금 -> KT스마트메시지/20261014_과금",
            "가람정보/20261020_과금/안내문서/안내.txt -> 가람정보/20261020_과금/안내.txt",
            "기타/가람정보/메모.txt -> 가람정보/메모.txt",
            "설치.exe -> 설치파일/설치.exe",
        ],
        "{}",
        plan.summary
    );
    assert_eq!(r.by_rules, 1);
    assert_eq!(r.folded, 1);
    // The customer PDF in 문서/ and the loose 메모2.txt wait for the AI.
    assert_eq!(r.ai_waiting, 2, "{}", r.summary);
    assert!(r.summary.contains("AI로 이것도 보기"), "{}", r.summary);
    assert!(plan
        .moves
        .iter()
        .all(|m| !m.from.starts_with(org.join("tidy"))));

    // Apply all: one batch, emptied folders gone, undo puts it all back.
    let plan = restructure::save(store, plan).unwrap();
    let a = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    let k = org.join("KT스마트메시지");
    for f in [
        "20261014_과금/a1.log",
        "20261014_과금/b.log",
        "20261014_과금/c.txt",
        "20261101_장애/d.txt",
    ] {
        assert!(k.join(f).is_file(), "{f}");
    }
    assert!(
        !org.join("KT 스마트메시지").exists(),
        "merged folder removed"
    );
    assert!(!k.join("20261020_과금").exists());
    assert!(!org.join("기타/가람정보").exists());
    assert!(org.join("기타/개발/예제.go").is_file(), "real topic stays");
    assert!(org.join("가람정보/메모.txt").is_file());
    assert!(org.join("설치파일/설치.exe").is_file());
    assert!(org.join("tidy/Cargo.toml").is_file());

    // Looked at again: only the AI files are left.
    let again = review(&org, store, &o).unwrap();
    assert!(
        again.plan.is_none(),
        "{:?}",
        again.plan.map(|p| lines(&p, &org))
    );
    assert_eq!(again.ai_waiting, 2);

    mover.undo_batch(&a.batch_id).unwrap();
    assert!(org.join("KT 스마트메시지/20261016_과금/c.txt").is_file());
    assert!(org.join("KT스마트메시지/20261020_과금/b.log").is_file());
    assert!(org.join("기타/가람정보/메모.txt").is_file());
    assert!(org.join("설치.exe").is_file());
}

#[test]
fn review_with_ai_places_customer_files_and_survives_failure() {
    let (_tmp, org, mover) = setup();
    let cfg = Config::default();
    let store = mover.store();
    let cur = Curator::new(dict(), cfg.curate.clone(), true).unwrap();
    let mock = Arc::new(MockProvider::responder(move |_s, user| {
        let mut files = Vec::new();
        for line in user.lines() {
            let Some(rest) = line.strip_prefix('[') else {
                continue;
            };
            let (id, rest) = rest.split_once(']').unwrap();
            if rest.contains("가람정보_견적서.pdf") {
                files.push(json!({
                    "id": id.parse::<u64>().unwrap(), "kind": "file", "entity": "가람정보",
                    "issue": "과금", "date": "2026-10-22", "doc_kind": "계약", "topic": null,
                    "confidence": 0.95, "reason": "견적서",
                }));
            }
        }
        json!({ "files": files })
    }));
    let ai = AiClassifier::with_provider(mock, &cfg, &RuleSet::default()).unwrap();
    let mut o = ReviewOptions::base(&cfg, &org, Vec::new());
    o.curator = Some(&cur);
    o.ai = Some(&ai);
    let r = review(&org, store, &o).unwrap();
    let plan = r.plan.unwrap();
    // Into the existing issue folder (2 days apart), no new 계약/ folder.
    assert!(
        lines(&plan, &org).contains(
            &"문서/2026/가람정보_견적서.pdf -> 가람정보/20261020_과금/가람정보_견적서.pdf"
                .to_string()
        ),
        "{:?}",
        lines(&plan, &org)
    );
    assert_eq!(r.by_ai, 1);
    assert_eq!(r.ai_waiting, 0);
    assert!(!lines(&plan, &org)
        .iter()
        .any(|l| l.starts_with("문서/2026/보고서")));
    assert!(
        !lines(&plan, &org)
            .iter()
            .any(|l| l.starts_with("메모2.txt")),
        "AI unsure → stays"
    );

    // AI down: the rest of the plan stands, nothing blocks.
    let failing = Arc::new(MockProvider::sequence(vec![Err("offline".into())]));
    let ai = AiClassifier::with_provider(failing, &cfg, &RuleSet::default()).unwrap();
    o.ai = Some(&ai);
    let r = review(&org, store, &o).unwrap();
    assert!(r.ai_error.is_some());
    assert_eq!(r.ai_waiting, 3, "no rules here: 설치.exe waits too");
    assert!(r.plan.is_some_and(|p| !p.moves.is_empty()));
}

#[test]
fn review_leaves_protected_and_watch_folders_alone() {
    let (_tmp, org, mover) = setup();
    let mut cfg = Config::default();
    cfg.watches.push(tidy_core::config::WatchRoot {
        path: org.join("KT 스마트메시지"),
        ..Default::default()
    });
    let store = mover.store();
    let o = ReviewOptions::base(&cfg, &org, vec![org.join("가람정보")]);
    let r = review(&org, store, &o).unwrap();
    let got = lines(&r.plan.unwrap(), &org);
    assert!(
        !got.iter().any(|l| l.contains("KT 스마트메시지")),
        "{got:?}"
    );
    assert!(!got.iter().any(|l| l.contains("가람정보")), "{got:?}");
    // The kept spelling's own same-issue folders still merge.
    assert!(
        got.contains(&"KT스마트메시지/20261020_과금 -> KT스마트메시지/20261014_과금".to_string())
    );
}
