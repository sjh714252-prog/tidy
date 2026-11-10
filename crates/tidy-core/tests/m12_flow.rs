//! M12 (SPEC §14.7): folder-unit work-context curation with a MockProvider.
//! A Go tool folder on the desktop (`가온누리원 로그/tools/go/*.go`) is ONE
//! item that keeps its structure under `가온누리원/<date>_로그분석/개발/…`;
//! files directly on the desktop are items of their own; the AI sees the
//! path relative to the desktop; watching never scatters a sub-folder's
//! files; folders still being copied wait; split into files on request.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, Anchors, RuleSet};
use tidy_core::config::{DuplicatePolicy, WatchRoot};
use tidy_core::curate::{ContextDict, Curator, MoveEdit};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::report::restructure;
use tidy_core::store::Store;
use tidy_core::watcher::IgnoreFilter;
use tidy_core::Config;

fn nofilter() -> IgnoreFilter {
    IgnoreFilter::new(&[]).unwrap()
}

/// Answers by item name; records every user prompt.
fn mock(prompts: Arc<Mutex<Vec<String>>>) -> Arc<MockProvider> {
    Arc::new(MockProvider::responder(move |_sys, user| {
        prompts.lock().unwrap().push(user.to_string());
        let mut files = Vec::new();
        for line in user.lines() {
            let Some(rest) = line.strip_prefix('[') else {
                continue;
            };
            let (id, rest) = rest.split_once(']').unwrap();
            let id: u64 = id.parse().unwrap();
            let rest = rest.trim();
            let (kind, body) = match (rest.strip_prefix("폴더: "), rest.strip_prefix("파일: "))
            {
                (Some(b), _) => ("folder", b),
                (_, Some(b)) => ("file", b),
                _ => continue,
            };
            let name = body.split(" | ").next().unwrap();
            // Path context: the first folder below the desktop names the customer.
            let path = body
                .split(" | ")
                .find_map(|p| {
                    p.strip_prefix("경로: ")
                        .or_else(|| p.strip_prefix("폴더: "))
                })
                .unwrap_or(".");
            let customer = path.starts_with("가온누리원") || name.contains("가온누리원");
            files.push(if customer {
                json!({"id": id, "kind": kind, "entity": "가온누리원", "issue": "로그분석",
                       "date": "2026-10-23", "doc_kind": if kind == "folder" { "개발" } else { "자료" },
                       "topic": null, "confidence": 0.95, "reason": format!("경로 {path}")})
            } else {
                json!({"id": id, "kind": kind, "entity": null, "issue": "기타", "date": null,
                       "doc_kind": "자료", "topic": "메모/개인", "confidence": 0.9, "reason": "고객사 없음"})
            });
        }
        json!({ "files": files })
    }))
}

struct Desk {
    _tmp: tempfile::TempDir,
    desk: PathBuf,
    tool: PathBuf,
}

fn desk() -> Desk {
    let tmp = tempfile::tempdir().unwrap();
    let desk = dunce::canonicalize(tmp.path()).unwrap().join("바탕 화면");
    let tool = desk.join("가온누리원 로그");
    let go = tool.join("tools").join("go");
    fs::create_dir_all(go.join("cmd")).unwrap();
    fs::write(tool.join("README.md"), "가온누리원 선거 로그 분석 도구").unwrap();
    for i in 0..30 {
        fs::write(
            go.join(format!("f{i:02}.go")),
            format!("package main // {i}"),
        )
        .unwrap();
    }
    fs::write(go.join("cmd").join("run.go"), "package cmd").unwrap();
    fs::write(desk.join("가온누리원 회의록.txt"), "회의").unwrap();
    fs::write(desk.join("장보기.txt"), "우유").unwrap();
    Desk {
        _tmp: tmp,
        desk,
        tool,
    }
}

fn cfg_settle(secs: u64) -> Config {
    let mut c = Config::default();
    c.curate.folder_settle_secs = secs;
    c
}

fn curator(cfg: &Config) -> Curator {
    let mut d = ContextDict::starter();
    d.add_entity("가온누리원", &["NRW".into()]).unwrap();
    Curator::new(d, cfg.curate.clone(), true).unwrap()
}

fn rel(p: &Path, base: &Path) -> String {
    p.strip_prefix(base)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn a_folder_moves_as_one_item_with_path_context_and_undoes() {
    let d = desk();
    let cfg = cfg_settle(0);
    let cur = curator(&cfg);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let ai = AiClassifier::with_provider(mock(prompts.clone()), &cfg, &RuleSet::default()).unwrap();
    let org = d.desk.parent().unwrap().join("정리");
    fs::create_dir_all(&org).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);

    let plan = cur
        .plan(&ai, mover.store(), &d.desk, &org, &nofilter(), &[], &[])
        .unwrap();
    // One folder item + two files (not 31 files of the tool).
    assert_eq!(plan.moves.len(), 3, "{:#?}", plan.moves);
    let folder = plan.moves.iter().find(|m| m.is_dir).unwrap();
    assert_eq!(folder.from, d.tool);
    assert_eq!(folder.files, 32);
    assert_eq!(
        rel(&folder.to, &org),
        "가온누리원/20261023_로그분석/개발/가온누리원 로그"
    );
    let note = folder.context.as_ref().unwrap();
    assert!(
        note.tree.as_deref().unwrap().contains("tools"),
        "{:?}",
        note.tree
    );
    // The AI saw the folder summary and each file's path from the desktop.
    let sent = prompts.lock().unwrap().join("\n");
    assert!(
        sent.contains("] 폴더: 가온누리원 로그 | 경로: 가온누리원 로그 | 파일 32개"),
        "{sent}"
    );
    assert!(sent.contains("확장자: go 31"), "{sent}");
    assert!(
        sent.contains("문서: 가온누리원 선거 로그 분석 도구"),
        "{sent}"
    );
    assert!(sent.contains("파일: 장보기.txt | 폴더: ."), "{sent}");
    assert!(plan.summary.contains("폴더 1개"), "{}", plan.summary);

    // Preview: the folder is not "left empty"; the tree after has the structure.
    let plan = restructure::save(mover.store(), plan).unwrap();
    let p = restructure::preview(&plan, None).unwrap();
    assert!(p.emptied.is_empty(), "{:?}", p.emptied);
    assert_eq!(p.files_moved, 34);

    // Apply → structure kept, one batch; undo → everything back.
    let r = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.moved.len(), 34);
    let moved = org
        .join("가온누리원")
        .join("20261023_로그분석")
        .join("개발")
        .join("가온누리원 로그");
    assert!(moved
        .join("tools")
        .join("go")
        .join("cmd")
        .join("run.go")
        .is_file());
    assert!(moved.join("README.md").is_file());
    assert!(!d.tool.exists(), "no empty shell left on the desktop");
    assert!(org
        .join("기타")
        .join("메모")
        .join("개인")
        .join("장보기.txt")
        .is_file());
    let u = mover.undo_batch(&r.batch_id).unwrap();
    assert_eq!(u.restored.len(), 34, "{:?}", u.conflicts);
    assert!(d.tool.join("tools").join("go").join("f29.go").is_file());
    assert!(d.desk.join("장보기.txt").is_file());
}

#[test]
fn organized_and_unsettled_folders_are_not_items_and_split_works() {
    let d = desk();
    // In place: results land on the desktop itself.
    let cfg = cfg_settle(0);
    let cur = curator(&cfg);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let ai = AiClassifier::with_provider(mock(prompts.clone()), &cfg, &RuleSet::default()).unwrap();
    let store = Store::open_in_memory().unwrap();
    // A customer folder (result) and a rule destination are not items.
    fs::create_dir_all(d.desk.join("가온누리원").join("이전")).unwrap();
    fs::write(d.desk.join("가온누리원").join("이전").join("a.txt"), "a").unwrap();
    fs::create_dir_all(d.desk.join("문서")).unwrap();
    fs::write(d.desk.join("문서").join("b.pdf"), "b").unwrap();
    let rules =
        RuleSet::from_toml("[[rule]]\nname='문서'\nmatch={ext=['pdf']}\ndest='문서'\n").unwrap();
    let skip = rules.dest_roots_for(std::slice::from_ref(&d.desk));
    let mut plan = cur
        .plan(&ai, &store, &d.desk, &d.desk, &nofilter(), &[], &skip)
        .unwrap();
    let dirs: Vec<_> = plan
        .moves
        .iter()
        .filter(|m| m.is_dir)
        .map(|m| m.from.clone())
        .collect();
    assert_eq!(dirs, vec![d.tool.clone()]);

    // "파일별로 나누기" → the tool's 32 files, each with its path as context.
    let id = plan.moves.iter().find(|m| m.is_dir).unwrap().id;
    let n = cur
        .split_folder(&ai, &store, &mut plan, id, &nofilter())
        .unwrap();
    assert_eq!(n, 32);
    assert!(plan.moves.iter().all(|m| !m.is_dir));
    let sent = prompts.lock().unwrap().join("\n");
    assert!(
        sent.contains("파일: f00.go | 폴더: 가온누리원 로그/tools/go"),
        "{sent}"
    );
    // A file's date can be changed in the preview.
    let any = plan
        .moves
        .iter()
        .find(|m| m.from.ends_with("run.go"))
        .unwrap()
        .id;
    cur.edit_move(
        &mut plan,
        any,
        &MoveEdit {
            entity: Some("가온누리원".into()),
            issue: Some("로그분석".into()),
            date: Some("2026-09-01".into()),
            ..Default::default()
        },
        &store,
    )
    .unwrap();
    let m = plan.moves.iter().find(|m| m.id == any).unwrap();
    assert_eq!(m.context.as_ref().unwrap().own_date, "2026-09-01");

    // A folder still being copied waits (settle 60 s, just written).
    let cfg = cfg_settle(60);
    let cur = curator(&cfg);
    let (units, unsettled) = cur.folder_units(
        &d.desk,
        &d.desk,
        &nofilter(),
        &[],
        &skip,
        Some(&store),
        SystemTime::now(),
    );
    assert!(units.is_empty());
    assert_eq!(unsettled, vec![d.tool.clone()]);
    let (units, _) = cur.folder_units(
        &d.desk,
        &d.desk,
        &nofilter(),
        &[],
        &skip,
        Some(&store),
        SystemTime::now() + Duration::from_secs(61),
    );
    assert_eq!(units.len(), 1);
}

#[test]
fn watching_never_scatters_a_sub_folder() {
    let d = desk();
    let cfg = cfg_settle(0);
    let cur = Arc::new(curator(&cfg));
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let watches = vec![WatchRoot {
        path: d.desk.clone(),
        recursive: true,
        ..Default::default()
    }];
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    // The customer folder exists already, so a top-level file of it is
    // placed automatically (95% ≥ 90%, known folder).
    fs::create_dir_all(
        d.desk
            .join("가온누리원")
            .join("20261023_로그분석")
            .join("자료"),
    )
    .unwrap();
    let engine = Engine::new(RuleSet::default(), mover, nofilter())
        .with_anchors(Anchors::from_watches(&watches))
        .with_ai(
            AiClassifier::with_provider(mock(prompts.clone()), &cfg, &RuleSet::default()).unwrap(),
        )
        .with_curator(cur);
    // A new file inside the tool folder: never moved by work context.
    let inner = d.tool.join("tools").join("go").join("new.go");
    fs::write(&inner, "package main").unwrap();
    let a = engine.handle(&inner, &new_batch_id()).unwrap();
    assert!(matches!(a, Action::NoRule(_)), "{a:?}");
    assert!(inner.is_file());
    assert!(
        prompts.lock().unwrap().is_empty(),
        "no AI call for files in sub-folders"
    );
    // A new file directly on the desktop: placed, with "." as its folder.
    let top = d.desk.join("가온누리원 회의록.txt");
    let a = engine.handle(&top, &new_batch_id()).unwrap();
    assert!(matches!(a, Action::Moved { .. }), "{a:?}");
    assert!(prompts.lock().unwrap()[0].contains("파일: 가온누리원 회의록.txt | 폴더: ."));
}

/// 0.12.3: the desktop once organized in place (Tidy moved files into
/// `가온누리원/…` on the desktop), now set to collect into `정리`. Those
/// leftover folders are no longer "results": their files go to the
/// organize location one by one (path = customer hint), the emptied
/// folders go, and the preview says what stays.
#[test]
fn leftover_in_place_results_move_to_the_organize_location() {
    let d = desk();
    let cfg = cfg_settle(0);
    let cur = curator(&cfg);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let ai = AiClassifier::with_provider(mock(prompts.clone()), &cfg, &RuleSet::default()).unwrap();
    let org = d.desk.parent().unwrap().join("정리");
    fs::create_dir_all(&org).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    // Earlier in-place organizing put a file into 바탕 화면/가온누리원/20260527_장애.
    let old = d.desk.join("old.txt");
    fs::write(&old, "가온누리원 장애 보고").unwrap();
    let left = d.desk.join("가온누리원").join("20260527_장애");
    mover
        .move_file(&tidy_core::mover::MoveRequest {
            src: old,
            dest_dir: left.clone(),
            batch_id: new_batch_id(),
            decided_by: tidy_core::store::DecidedBy::Restructure,
            rule_name: Some("업무 맥락".into()),
            ai_confidence: Some(0.9),
            new_name: None,
            encrypt: false,
            on_duplicate: None,
        })
        .unwrap();
    fs::write(left.join("메모.txt"), "직접 넣은 파일").unwrap();
    fs::write(d.desk.join("Edge.lnk"), "lnk").unwrap();
    let filter = IgnoreFilter::new(&["*.lnk".to_string()]).unwrap();

    let plan = cur
        .plan(&ai, mover.store(), &d.desk, &org, &filter, &[], &[])
        .unwrap();
    let froms: Vec<String> = plan.moves.iter().map(|m| rel(&m.from, &d.desk)).collect();
    assert!(
        froms.contains(&"가온누리원/20260527_장애/old.txt".to_string()),
        "{froms:?}"
    );
    assert!(
        froms.contains(&"가온누리원/20260527_장애/메모.txt".to_string()),
        "{froms:?}"
    );
    assert!(
        plan.summary
            .contains("예전에 이 폴더 안에서 정리했던 폴더 1개(가온누리원)"),
        "{}",
        plan.summary
    );
    assert!(
        plan.summary.contains("이 미리보기에 없는 1개(Edge.lnk)"),
        "{}",
        plan.summary
    );
    let sent = prompts.lock().unwrap().join("\n");
    assert!(
        sent.contains("파일: 메모.txt | 폴더: 가온누리원/20260527_장애"),
        "{sent}"
    );

    // In place (results on the desktop itself) it still stays put.
    let inplace = cur
        .plan(&ai, mover.store(), &d.desk, &d.desk, &filter, &[], &[])
        .unwrap();
    assert!(inplace
        .moves
        .iter()
        .all(|m| !m.from.starts_with(d.desk.join("가온누리원"))));

    let plan = restructure::save(mover.store(), plan).unwrap();
    let r = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(walk_has(&org, "메모.txt"), "moved into 정리");
    assert!(
        !d.desk.join("가온누리원").exists(),
        "emptied leftover folder removed"
    );
    assert!(d.desk.join("Edge.lnk").is_file());
    let u = mover.undo_batch(&r.batch_id).unwrap();
    assert!(u.conflicts.is_empty(), "{:?}", u.conflicts);
    assert!(left.join("메모.txt").is_file());
}

fn walk_has(dir: &Path, name: &str) -> bool {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy() == name)
}

/// 0.12.5: a customer folder Tidy once put a file into (so a 0.12.3
/// leftover) still keeps its own source tree together: the file Tidy put
/// there goes on its own, `tools/` (never touched by Tidy) moves as ONE
/// folder item under the customer — never `reader.go` alone into
/// 기타/개발/소스코드.
#[test]
fn leftover_customer_folder_keeps_its_source_tree_whole() {
    let d = desk();
    let cfg = cfg_settle(0);
    let cur = curator(&cfg);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let ai = AiClassifier::with_provider(mock(prompts.clone()), &cfg, &RuleSet::default()).unwrap();
    let org = d.desk.parent().unwrap().join("정리");
    fs::create_dir_all(&org).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let old = d.desk.join("장애 보고.txt");
    fs::write(&old, "보고").unwrap();
    mover
        .move_file(&tidy_core::mover::MoveRequest {
            src: old,
            dest_dir: d.tool.clone(),
            batch_id: new_batch_id(),
            decided_by: tidy_core::store::DecidedBy::Ai,
            rule_name: Some("업무 맥락".into()),
            ai_confidence: Some(0.9),
            new_name: None,
            encrypt: false,
            on_duplicate: None,
        })
        .unwrap();

    let plan = cur
        .plan(&ai, mover.store(), &d.desk, &org, &nofilter(), &[], &[])
        .unwrap();
    let froms: Vec<(String, bool)> = plan
        .moves
        .iter()
        .map(|m| (rel(&m.from, &d.desk), m.is_dir))
        .collect();
    assert!(
        froms.contains(&("가온누리원 로그/tools".to_string(), true)),
        "{froms:?}"
    );
    assert!(
        froms.contains(&("가온누리원 로그/장애 보고.txt".to_string(), false)),
        "{froms:?}"
    );
    assert!(
        !froms
            .iter()
            .any(|(f, _)| f.starts_with("가온누리원 로그/tools/")),
        "nothing pulled out of the source tree: {froms:?}"
    );
    let tools = plan.moves.iter().find(|m| m.is_dir).unwrap();
    assert!(
        rel(&tools.to, &org).starts_with("가온누리원/"),
        "under the customer: {}",
        tools.to.display()
    );

    let plan = restructure::save(mover.store(), plan).unwrap();
    let r = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let moved = walkdir::WalkDir::new(&org)
        .into_iter()
        .filter_map(Result::ok)
        .find(|e| e.file_name() == "run.go")
        .unwrap();
    assert!(
        rel(moved.path(), &org).ends_with("tools/go/cmd/run.go"),
        "structure kept: {}",
        moved.path().display()
    );
}

/// 0.12.5: the plain AI step (no work context) leaves files in sub-folders
/// alone too, and old queue entries that would pull such a file out are
/// dropped (the file stays).
#[test]
fn ai_never_pulls_a_file_out_of_a_sub_folder_and_old_queue_entries_go() {
    let d = desk();
    let cfg = cfg_settle(0);
    let watches = vec![WatchRoot {
        path: d.desk.clone(),
        recursive: true,
        ..Default::default()
    }];
    let calls = Arc::new(Mutex::new(0usize));
    let c2 = calls.clone();
    let provider = Arc::new(MockProvider::responder(move |_s, _u| {
        *c2.lock().unwrap() += 1;
        json!({"dest_folder": "기타/개발/소스코드", "confidence": 0.7, "reason": "소스"})
    }));
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let engine = Engine::new(RuleSet::default(), mover, nofilter())
        .with_anchors(Anchors::from_watches(&watches))
        .with_ai(AiClassifier::with_provider(provider, &cfg, &RuleSet::default()).unwrap());
    let inner = d.tool.join("tools").join("go").join("f01.go");
    let a = engine.handle(&inner, &new_batch_id()).unwrap();
    assert!(matches!(a, Action::NoRule(_)), "{a:?}");
    assert_eq!(*calls.lock().unwrap(), 0, "no AI call");

    let store = engine.mover().store();
    let top = d.desk.join("장보기.txt");
    for p in [&inner, &top] {
        store
            .upsert_pending_entry(&tidy_core::store::NewPending {
                path: p,
                suggested_dest: Some(&d.desk.join("기타")),
                reason: Some("예전 제안"),
                source: tidy_core::store::PendingSource::Ai,
                confidence: Some(0.7),
                suggested_name: None,
                encrypt: false,
                pii: None,
            })
            .unwrap();
    }
    let dropped = engine.drop_scattering_pending().unwrap();
    assert_eq!(dropped, vec![inner.clone()]);
    let left: Vec<PathBuf> = store
        .list_pending()
        .unwrap()
        .into_iter()
        .map(|p| p.path)
        .collect();
    assert_eq!(left, vec![top]);
    assert!(inner.is_file());
}
