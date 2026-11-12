//! (0.14.0, SPEC §5.3) `[mover] ask_before_move`: while watching, nothing
//! moves on its own. A rule match and a confident AI answer into an
//! `ai_auto` folder — both of which move by themselves otherwise — wait in
//! the confirm queue; approving moves the file (logged, undoable).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, RuleSet};
use tidy_core::config::{Config, DuplicatePolicy};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::store::{PendingSource, Store};
use tidy_core::watcher::IgnoreFilter;

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

fn engine(rules: &RuleSet, mock: Arc<MockProvider>, db: &Path, ask: bool) -> Engine {
    let mut cfg = Config::default();
    cfg.ai.enabled = true;
    let ai = AiClassifier::with_provider(mock, &cfg, rules).unwrap();
    let mover = Mover::new(Store::open(db).unwrap(), DuplicatePolicy::Skip);
    Engine::new(
        rules.clone(),
        mover,
        IgnoreFilter::new(&cfg.classify.ignore_globs).unwrap(),
    )
    .with_ai(ai)
    .ask_before_move(ask)
}

#[test]
fn every_move_waits_for_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let pics = base.join("사진");
    let kt = base.join("정리 문서").join("KT 과금");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(&kt).unwrap();
    let rules_path = base.join("rules.toml");
    fs::write(
        &rules_path,
        format!(
            "[[rule]]\nname = \"스크린샷\"\nmatch = {{ ext = [\"png\"] }}\ndest = \"{}\"\n\n\
             [[rule]]\nname = \"문서 폴더\"\nmatch = {{ ext = [\"never\"] }}\ndest = \"{}\"\nai_auto = true\n",
            toml_path(&pics),
            toml_path(&kt)
        ),
    )
    .unwrap();
    let rules = RuleSet::load(&rules_path).unwrap();
    let mock = Arc::new(MockProvider::sequence(vec![Ok(json!({
        "dest": kt.display().to_string(),
        "confidence": 0.97,
        "reason": "파일명에 KT 과금이 있음",
        "new_folder": false,
        "suggested_rename": null
    }))]));
    let eng = engine(&rules, mock.clone(), &base.join("tidy.db"), true);
    let store = eng.mover().store();

    // Rule match: queued, not moved.
    let shot = desk.join("화면.png");
    fs::write(&shot, "png").unwrap();
    match eng.handle(&shot, &new_batch_id()).unwrap() {
        Action::Pending {
            source, suggested, ..
        } => {
            assert_eq!(source, PendingSource::Rule);
            assert_eq!(suggested, pics);
        }
        other => panic!("{other:?}"),
    }
    assert!(shot.is_file());

    // Confident AI answer into an ai_auto folder: queued too.
    let bill = desk.join("KT과금내역서_2026-07.txt");
    fs::write(&bill, "7월 과금").unwrap();
    match eng.handle(&bill, &new_batch_id()).unwrap() {
        Action::Pending {
            source, confidence, ..
        } => {
            assert_eq!(source, PendingSource::Ai);
            assert_eq!(confidence, Some(0.97));
        }
        other => panic!("{other:?}"),
    }
    assert!(bill.is_file());
    // Saved again while waiting: the AI is not asked a second time.
    assert!(matches!(
        eng.handle(&bill, &new_batch_id()).unwrap(),
        Action::Skipped { .. }
    ));
    assert_eq!(mock.prompts().len(), 1);
    assert!(store.recent_moves(10).unwrap().is_empty());

    // Approving moves to the suggested folder.
    let pending = store.list_pending().unwrap();
    assert_eq!(pending.len(), 2);
    for p in pending {
        let r = resolve(
            eng.mover(),
            p.id,
            &Decision::Move {
                dest_dir: None,
                new_name: None,
                on_duplicate: None,
            },
            &new_batch_id(),
            None,
        )
        .unwrap();
        assert!(matches!(r, Resolution::Moved { .. }), "{r:?}");
    }
    assert!(pics.join("화면.png").is_file());
    assert!(kt.join("KT과금내역서_2026-07.txt").is_file());
    assert!(!shot.exists() && !bill.exists());
}

#[test]
fn off_by_default_moves_as_before() {
    assert!(!Config::default().mover.ask_before_move);
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("desk");
    fs::create_dir_all(&desk).unwrap();
    let rules = RuleSet::from_toml(&format!(
        "[[rule]]\nname = \"스크린샷\"\nmatch = {{ ext = [\"png\"] }}\ndest = \"{}\"\n",
        toml_path(&base.join("사진"))
    ))
    .unwrap();
    let eng = engine(
        &rules,
        Arc::new(MockProvider::sequence(vec![])),
        &base.join("tidy.db"),
        false,
    );
    let shot = desk.join("a.png");
    fs::write(&shot, "png").unwrap();
    assert!(matches!(
        eng.handle(&shot, &new_batch_id()).unwrap(),
        Action::Moved { .. }
    ));
}
