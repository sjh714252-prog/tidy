//! 0.16.0 (SPEC §14.22) "정리 위치 새로 짜기": an organize root that grew
//! under several older layouts is re-planned as a whole, offline — every
//! customer's file into that customer's folder by kind, split folders
//! merged, the rest by the rules; one plan, one undoable batch.

use std::fs;
use std::path::{Path, PathBuf};

use tidy_core::classify::RuleSet;
use tidy_core::config::DuplicatePolicy;
use tidy_core::curate::{ContextDict, Curator};
use tidy_core::index::{IndexOptions, Indexer};
use tidy_core::mover::Mover;
use tidy_core::report::rebuild::rebuild;
use tidy_core::report::restructure::{self, Plan, PlanSource};
use tidy_core::report::review::ReviewOptions;
use tidy_core::store::Store;
use tidy_core::watcher::IgnoreFilter;
use tidy_core::Config;

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

fn write(org: &Path, rel: &str, body: &str) -> PathBuf {
    let p = org.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, body).unwrap();
    p
}

#[test]
fn rebuild_gathers_every_customer_file_by_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let org = dunce::canonicalize(tmp.path()).unwrap().join("정리");
    fs::create_dir_all(&org).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let store = mover.store();

    // Customers in two spellings, issue folders, a loose file at the top.
    write(&org, "KT스마트메시지/20261014_과금/a.log", "a");
    write(&org, "KT 스마트메시지/20261016_과금/c.txt", "c");
    write(&org, "KT스마트메시지/견적.pdf", "q");
    write(&org, "가람정보/20261020_과금/l.log", "l");
    // Customer data in the type rules' folders, under 기타 and at the top.
    write(&org, "문서/2026/가람정보_견적서.pdf", "pdf");
    let body = write(&org, "문서/2026/회의록.txt", "가람정보 담당자와 과금 회의");
    write(&org, "문서/2026/보고서.pdf", "그냥 보고서");
    write(&org, "이미지/KT스마트메시지 화면.png", "png");
    write(&org, "기타/개발/예제.go", "go");
    write(&org, "기타/가람정보/메모.txt", "memo");
    write(&org, "기타/개발/가람정보 연동.go", "go");
    write(&org, "가람정보 설치본/setup.exe", "exe");
    write(&org, "설치.exe", "exe");
    write(&org, "메모.txt", "memo");
    // A user folder (not Tidy's layout) under a kind folder: whole.
    write(&org, "문서/2026/내 자료/x.pdf", "x");
    // A code project: never touched.
    write(&org, "tidy/Cargo.toml", "[package]");
    write(&org, "tidy/src/main.rs", "fn main(){}");

    // The body text comes from the index.
    let opts = IndexOptions::default();
    Indexer::new(store).index_file(&body, &opts).unwrap();

    let cfg = Config::default();
    let mut d = ContextDict::starter();
    d.add_entity("가람정보", &[]).unwrap();
    let cur = Curator::new(d, cfg.curate.clone(), true).unwrap();
    let rules =
        RuleSet::from_toml("[[rule]]\nname='설치 파일'\nmatch={ext=['exe']}\ndest='설치파일'\n")
            .unwrap();
    let nf = IgnoreFilter::new(&[]).unwrap();
    let mut o = ReviewOptions::base(&cfg, &org, Vec::new());
    o.rules = Some(&rules);
    o.filter = Some(&nf);
    o.curator = Some(&cur);

    let r = rebuild(&org, store, &o).unwrap();
    let plan = r.plan.clone().expect("a plan");
    assert_eq!(plan.source, PlanSource::Rebuild);
    assert_eq!(
        lines(&plan, &org),
        vec![
            "KT 스마트메시지/20261016_과금 -> KT스마트메시지/20261014_과금",
            "KT스마트메시지/견적.pdf -> KT스마트메시지/문서/견적.pdf",
            "가람정보 설치본 -> 가람정보/가람정보 설치본",
            "기타/가람정보/메모.txt -> 가람정보/텍스트/메모.txt",
            "기타/개발/가람정보 연동.go -> 가람정보/코드/가람정보 연동.go",
            "문서/2026/가람정보_견적서.pdf -> 가람정보/문서/가람정보_견적서.pdf",
            "문서/2026/회의록.txt -> 가람정보/텍스트/회의록.txt",
            "설치.exe -> 설치파일/설치.exe",
            "이미지/KT스마트메시지 화면.png -> KT스마트메시지/이미지/KT스마트메시지 화면.png",
        ],
        "{}",
        plan.summary
    );
    assert!(r.new_customers.is_empty(), "{:?}", r.new_customers);
    assert!(!r.capped);

    // Apply: one batch; undo puts it all back.
    let plan = restructure::save(store, plan).unwrap();
    let a = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    let k = org.join("KT스마트메시지");
    assert!(k.join("20261014_과금/c.txt").is_file());
    assert!(k.join("문서/견적.pdf").is_file());
    assert!(k.join("이미지/KT스마트메시지 화면.png").is_file());
    assert!(!org.join("KT 스마트메시지").exists());
    assert!(org.join("가람정보/텍스트/회의록.txt").is_file());
    assert!(org.join("가람정보/가람정보 설치본/setup.exe").is_file());
    assert!(
        org.join("기타/개발/예제.go").is_file(),
        "기타's topic stays"
    );
    assert!(
        org.join("문서/2026/내 자료/x.pdf").is_file(),
        "user folder stays"
    );
    assert!(
        org.join("메모.txt").is_file(),
        "no rule, no customer: stays"
    );
    assert!(org.join("tidy/src/main.rs").is_file());

    // Looked at again: nothing left to do.
    let again = rebuild(&org, store, &o).unwrap();
    assert!(
        again.plan.is_none(),
        "{:?}",
        again.plan.map(|p| lines(&p, &org))
    );

    mover.undo_batch(&a.batch_id).unwrap();
    assert!(org.join("KT 스마트메시지/20261016_과금/c.txt").is_file());
    assert!(org.join("문서/2026/회의록.txt").is_file());
    assert!(org.join("가람정보 설치본/setup.exe").is_file());
    assert!(org.join("KT스마트메시지/견적.pdf").is_file());
}

#[test]
fn rebuild_names_a_new_customer_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let org = dunce::canonicalize(tmp.path()).unwrap().join("정리");
    write(&org, "문서/한빛소프트 계약서.pdf", "pdf");
    let store = Store::open_in_memory().unwrap();
    let cfg = Config::default();
    let mut d = ContextDict::starter();
    d.add_entity("한빛소프트", &[]).unwrap();
    let cur = Curator::new(d, cfg.curate.clone(), true).unwrap();
    let mut o = ReviewOptions::base(&cfg, &org, Vec::new());
    o.curator = Some(&cur);
    let r = rebuild(&org, &store, &o).unwrap();
    assert_eq!(
        lines(&r.plan.unwrap(), &org),
        vec!["문서/한빛소프트 계약서.pdf -> 한빛소프트/문서/한빛소프트 계약서.pdf"]
    );
    assert_eq!(r.new_customers, vec!["한빛소프트".to_string()]);
    assert!(
        r.summary.contains("새 고객사 폴더: 한빛소프트"),
        "{}",
        r.summary
    );
}
