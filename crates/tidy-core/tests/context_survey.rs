//! 0.12.0 (SPEC §14.18): "모든 감시 폴더 훑기" — one work-context plan over
//! several watch folders, laid out under the organize root with the
//! folders already there reused (spelling, issue a few days apart), the
//! proposed structure (existing vs new), one-batch apply and undo; and a
//! file arriving later goes into the existing folder.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, RuleSet};
use tidy_core::config::DuplicatePolicy;
use tidy_core::curate::{ContextDict, Curator, Source};
use tidy_core::mover::Mover;
use tidy_core::report::restructure::{self, Plan};
use tidy_core::store::Store;
use tidy_core::watcher::IgnoreFilter;
use tidy_core::Config;

type Answer = (
    Option<&'static str>,
    &'static str,
    &'static str,
    &'static str,
    f64,
);

/// (entity, issue, date, kind, confidence) per file / folder name.
fn answers() -> HashMap<&'static str, Answer> {
    HashMap::from([
        // Spelled with a space; the folder "KT스마트메시지/20261014_과금"
        // already exists → reused (6 days apart).
        (
            "KT 과금 로그 1020.log",
            (Some("KT 스마트메시지"), "과금", "2026-10-20", "로그", 0.95),
        ),
        (
            "kt 과금 안내.txt",
            (
                Some("kt스마트메시지"),
                "과금",
                "2026-10-21",
                "안내문서",
                0.9,
            ),
        ),
        // A new customer → new folders.
        (
            "가람정보_장애.txt",
            (Some("가람정보"), "장애", "2026-10-30", "회의록", 0.9),
        ),
        // A folder item from the second watch folder: an existing customer,
        // a new issue.
        (
            "KT 전환 자료",
            (Some("KT스마트메시지"), "전환", "2026-10-28", "자료", 0.9),
        ),
        ("메모.txt", (None, "메모", "2026-10-01", "기타 자료", 0.2)),
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
            let Some((id, rest)) = rest.split_once(']') else {
                continue;
            };
            let rest = rest.trim();
            let Some(name) = rest
                .strip_prefix("파일: ")
                .or_else(|| rest.strip_prefix("폴더: "))
                .and_then(|r| r.split(" | ").next())
            else {
                continue;
            };
            if let Some((e, i, d, k, c)) = table.get(name) {
                files.push(json!({
                    "id": id.parse::<u64>().unwrap(), "entity": e, "issue": i, "date": d,
                    "doc_kind": k, "topic": if e.is_none() { Some("메모/기타") } else { None },
                    "confidence": c, "reason": format!("{name}에서 판단"),
                }));
            }
        }
        json!({ "files": files })
    }))
}

fn rel(p: &Path, base: &Path) -> String {
    p.strip_prefix(base)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn item<'a>(plan: &'a Plan, name: &str) -> &'a restructure::PlannedMove {
    plan.moves
        .iter()
        .find(|m| m.from.file_name().unwrap() == name)
        .unwrap()
}

#[test]
fn survey_all_watch_folders_reuses_existing_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let down = base.join("다운로드");
    let target = base.join("정리");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(down.join("KT 전환 자료/캡처")).unwrap();
    fs::create_dir_all(target.join("KT스마트메시지/20261014_과금/로그")).unwrap();
    fs::write(
        target.join("KT스마트메시지/20261014_과금/로그/예전.log"),
        "x",
    )
    .unwrap();
    for n in [
        "KT 과금 로그 1020.log",
        "kt 과금 안내.txt",
        "가람정보_장애.txt",
        "메모.txt",
    ] {
        fs::write(desk.join(n), format!("{n} 본문")).unwrap();
    }
    fs::write(down.join("KT 전환 자료/대상.csv"), "a,b").unwrap();
    fs::write(down.join("KT 전환 자료/캡처/화면.png"), "png").unwrap();
    // The folder item must be "settled" (not changing).
    let mut cfg = Config::default();
    cfg.curate.folder_settle_secs = 0;

    let provider = mock();
    let ai = AiClassifier::with_provider(provider.clone(), &cfg, &RuleSet::default()).unwrap();
    let mut dict = ContextDict::starter();
    dict.add_entity("KT스마트메시지", &[]).unwrap();
    let cur = Curator::new(dict, cfg.curate.clone(), true).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let store = mover.store();
    let filter = IgnoreFilter::new(&[]).unwrap();

    let sources = vec![
        Source {
            root: desk.clone(),
            anchor: target.clone(),
            skip: vec![target.clone()],
        },
        Source {
            root: down.clone(),
            anchor: target.clone(),
            skip: vec![target.clone()],
        },
    ];
    let plan = cur.plan_many(&ai, store, &sources, &filter, &[]).unwrap();
    // One request for both folders; the existing folders were in the prompt.
    let prompts = provider.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].1.contains("정리 위치에 이미 있는 폴더")
            && prompts[0].1.contains("KT스마트메시지: 20261014_과금"),
        "{}",
        prompts[0].1
    );
    assert_eq!(plan.root, target);
    assert_eq!(plan.sources, vec![desk.clone(), down.clone()]);

    let r = |n: &str| rel(&item(&plan, n).to, &target);
    // Spelling and date differences → the existing customer / issue folder.
    assert_eq!(
        r("KT 과금 로그 1020.log"),
        "KT스마트메시지/20261014_과금/로그/KT 과금 로그 1020.log"
    );
    assert_eq!(
        r("kt 과금 안내.txt"),
        "KT스마트메시지/20261014_과금/kt 과금 안내.txt"
    );
    let n = |name: &str| item(&plan, name).context.clone().unwrap();
    assert_eq!(n("KT 과금 로그 1020.log").folder_state, "existing");
    assert_eq!(n("KT 과금 로그 1020.log").date, "2026-10-14");
    // (0.13.0) One 안내문서 file: no new kind folder of its own — it goes
    // into the existing issue folder.
    assert_eq!(n("kt 과금 안내.txt").folder_state, "existing");
    assert_eq!(n("kt 과금 안내.txt").new_dir, None);
    // A new customer.
    assert_eq!(
        r("가람정보_장애.txt"),
        "가람정보/20261030_장애/가람정보_장애.txt"
    );
    assert_eq!(n("가람정보_장애.txt").new_dir.as_deref(), Some("가람정보"));
    // The folder item from 다운로드: whole, under the existing customer.
    let f = item(&plan, "KT 전환 자료");
    assert!(f.is_dir);
    assert_eq!(
        rel(&f.to, &target),
        "KT스마트메시지/20261028_전환/KT 전환 자료"
    );
    assert_eq!(
        n("KT 전환 자료").new_dir.as_deref(),
        Some("KT스마트메시지/20261028_전환")
    );
    assert!(n("메모.txt").unclassified);
    assert!(
        plan.summary.contains("이미 있는 폴더로 2개"),
        "{}",
        plan.summary
    );

    // The proposed structure: customer → issue → kind, existing vs new.
    let s = restructure::structure(&plan, None);
    let get = |p: &str| {
        s.iter()
            .find(|d| d.path == p)
            .unwrap_or_else(|| panic!("{p}: {s:?}"))
    };
    assert!(get("KT스마트메시지").exists);
    assert_eq!(get("KT스마트메시지").items, 3);
    assert!(get("KT스마트메시지/20261014_과금").exists);
    assert!(!get("KT스마트메시지/20261028_전환").exists);
    assert!(!get("가람정보").exists);
    assert_eq!(get("가람정보/20261030_장애").depth, 2);

    // The preview of a gathering plan shows the incoming files.
    let plan = restructure::save(store, plan).unwrap();
    let pv = restructure::preview(&plan, None).unwrap();
    assert!(pv.after.contains("가람정보"), "{}", pv.after);
    assert_eq!(pv.files_moved, 5, "3 files + 2 in the folder");
    assert!(pv.outside.is_empty());

    // Apply as one batch, then undo.
    let rep = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert_eq!(rep.moved.len(), 5, "{:?}", rep.errors);
    assert!(target
        .join("KT스마트메시지/20261014_과금/로그/KT 과금 로그 1020.log")
        .is_file());
    assert!(target
        .join("KT스마트메시지/20261028_전환/KT 전환 자료/캡처/화면.png")
        .is_file());
    assert!(!down.join("KT 전환 자료").exists());
    assert!(desk.join("메모.txt").is_file(), "미분류 stays");
    mover.undo_batch(&rep.batch_id).unwrap();
    assert!(desk.join("KT 과금 로그 1020.log").is_file());
    assert!(down.join("KT 전환 자료/캡처/화면.png").is_file());
    assert!(target
        .join("KT스마트메시지/20261014_과금/로그/예전.log")
        .is_file());
}

#[test]
fn new_file_goes_into_existing_folder_when_watching() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let target = base.join("정리");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(target.join("KT스마트메시지/20261014_과금/로그")).unwrap();
    fs::write(desk.join("KT 과금 로그 1020.log"), "x").unwrap();
    fs::write(desk.join("가람정보_장애.txt"), "x").unwrap();
    let cfg = Config::default();
    let ai = AiClassifier::with_provider(mock(), &cfg, &RuleSet::default()).unwrap();
    // Empty dictionary: the customer is known from its folder only.
    let cur = Curator::new(ContextDict::starter(), cfg.curate.clone(), true).unwrap();
    let store = Store::open_in_memory().unwrap();

    let input = cur
        .input_in(&desk.join("KT 과금 로그 1020.log"), &desk, Some(&store))
        .unwrap();
    let p = cur.place(&ai, &store, &input, &target).unwrap().unwrap();
    assert_eq!(p.dest_dir, target.join("KT스마트메시지/20261014_과금/로그"));
    assert!(p.known_folder && p.known_entity && p.new_dir.is_none());

    let input = cur
        .input_in(&desk.join("가람정보_장애.txt"), &desk, Some(&store))
        .unwrap();
    let p = cur.place(&ai, &store, &input, &target).unwrap().unwrap();
    assert!(!p.known_folder && !p.known_entity);
    assert_eq!(p.new_dir.as_deref(), Some("가람정보"));
}
