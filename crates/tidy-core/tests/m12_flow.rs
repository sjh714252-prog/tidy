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
