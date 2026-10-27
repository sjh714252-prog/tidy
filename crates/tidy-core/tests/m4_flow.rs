//! M4 end-to-end with `MockProvider` only (no real API calls):
//! rules first → AI auto-move into an `ai_auto` folder → low-confidence AI
//! suggestion waits for the user → AI failure leaves the file → three user
//! decisions → learner proposes a rule → accepted rule handles the next file
//! without asking the AI.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{learner, AiClassifier, RuleSet};
use tidy_core::config::{Config, DuplicatePolicy};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::store::{DecidedBy, PendingSource, Store};
use tidy_core::watcher::IgnoreFilter;

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

fn answer(dest: &Path, confidence: f64) -> serde_json::Value {
    json!({
        "dest": dest.display().to_string(),
        "confidence": confidence,
        "reason": "파일명에 KT 과금이 있음",
        "new_folder": false,
        "suggested_rename": null
    })
}

fn engine(rules: &RuleSet, mock: Arc<MockProvider>, db: &Path) -> Engine {
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
}

#[test]
fn rules_ai_ask_learn() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let docs = base.join("정리 문서");
    let kt = docs.join("업무").join("KT 과금");
    let misc = base.join("기타 보관");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(&kt).unwrap();
    fs::create_dir_all(&misc).unwrap();
    let db = base.join("tidy.db");
    let rules_path = base.join("rules.toml");
    fs::write(
        &rules_path,
        format!(
            "[[rule]]\nname = \"스크린샷\"\nmatch = {{ ext = [\"png\"] }}\ndest = \"{}/사진\"\n\n\
             [[rule]]\nname = \"문서 폴더\"\nmatch = {{ ext = [\"never\"] }}\ndest = \"{}/{{yyyy}}\"\nai_auto = true\n",
            toml_path(&base),
            toml_path(&docs)
        ),
    )
    .unwrap();
    let rules = RuleSet::load(&rules_path).unwrap();

    // Answers in call order.
    let mock = Arc::new(MockProvider::sequence(vec![
        Ok(answer(&kt, 0.95)),            // 1: auto (ai_auto folder)
        Ok(answer(&misc, 0.95)),          // 2: high but not ai_auto -> ask
        Err("connection refused".into()), // 3: failure -> file stays
        Ok(answer(&kt, 0.6)),             // 4..: low confidence -> ask
    ]));
    let eng = engine(&rules, mock.clone(), &db);

    // Rule match: the AI is not consulted.
    let shot = desk.join("화면.png");
    fs::write(&shot, "png").unwrap();
    assert!(matches!(
        eng.handle(&shot, &new_batch_id()).unwrap(),
        Action::Moved {
            by: DecidedBy::Rule,
            ..
        }
    ));
    assert_eq!(mock.prompts().len(), 0);

    // 1: confident + inside an ai_auto folder -> moved by the AI, logged.
    let f1 = desk.join("KT과금내역서_2026-07.txt");
    fs::write(&f1, "7월 과금").unwrap();
    match eng.handle(&f1, &new_batch_id()).unwrap() {
        Action::Moved {
            dst,
            by,
            confidence,
            log_id,
            ..
        } => {
            assert_eq!(by, DecidedBy::Ai);
            assert_eq!(confidence, Some(0.95));
            assert!(dst.starts_with(&kt));
            let e = eng.mover().store().get_move(log_id).unwrap().unwrap();
            assert_eq!(e.decided_by, DecidedBy::Ai);
            assert_eq!(e.ai_confidence, Some(0.95));
        }
        other => panic!("{other:?}"),
    }

    // 2: confident but the folder is not ai_auto -> asks the user.
    let f2 = desk.join("KT과금내역서_2026-08.txt");
    fs::write(&f2, "8월 과금").unwrap();
    assert!(matches!(
        eng.handle(&f2, &new_batch_id()).unwrap(),
        Action::Pending {
            source: PendingSource::Ai,
            ..
        }
    ));
    // Saving it again does not ask the AI again.
    let calls = mock.prompts().len();
    assert!(matches!(
        eng.handle(&f2, &new_batch_id()).unwrap(),
        Action::Skipped { .. }
    ));
    assert_eq!(mock.prompts().len(), calls);

    // 3: provider down -> file stays, error reported, logged.
    let f3 = desk.join("KT과금내역서_2026-09.txt");
    fs::write(&f3, "9월 과금").unwrap();
    assert!(matches!(
        eng.handle(&f3, &new_batch_id()).unwrap(),
        Action::AiFailed { .. }
    ));
    assert!(f3.exists());
    let log = eng.mover().store().recent_ai_log(10).unwrap();
    assert_eq!(log.iter().filter(|l| !l.ok).count(), 1);

    // 4: low confidence -> pending; the user picks KT for f2, f3(retry), f4.
    let f4 = desk.join("KT과금내역서_2026-10.txt");
    fs::write(&f4, "10월 과금").unwrap();
    assert!(matches!(
        eng.handle(&f4, &new_batch_id()).unwrap(),
        Action::Pending { .. }
    ));
    assert!(matches!(
        eng.handle(&f3, &new_batch_id()).unwrap(),
        Action::Pending { .. }
    ));
    let store = eng.mover().store();
    let pending = store.list_pending().unwrap();
    assert_eq!(pending.len(), 3);
    assert!(pending
        .iter()
        .all(|p| p.source == PendingSource::Ai && p.confidence.is_some()));
    for p in &pending {
        let r = resolve(
            eng.mover(),
            p.id,
            &Decision::Move {
                dest_dir: Some(kt.clone()),
                new_name: None,
                on_duplicate: None,
            },
            &new_batch_id(),
            None,
        )
        .unwrap();
        assert!(matches!(r, Resolution::Moved { .. }));
    }

    // Learner: three identical decisions -> rule suggestion -> accept.
    let sugg = learner::suggestions(store, &rules, 3).unwrap();
    assert_eq!(sugg.len(), 1, "{sugg:#?}");
    assert_eq!(sugg[0].dest, kt);
    let learned = learner::accept(store, &rules_path, 3, &sugg[0].key).unwrap();

    // The learned rule handles the next one; the AI is not asked.
    let eng2 = engine(&learned, mock.clone(), &db);
    let before = mock.prompts().len();
    let f5 = desk.join("KT과금내역서_2026-11.txt");
    fs::write(&f5, "11월 과금").unwrap();
    match eng2.handle(&f5, &new_batch_id()).unwrap() {
        Action::Moved { dst, by, rule, .. } => {
            assert_eq!(by, DecidedBy::Rule);
            assert!(rule.starts_with("학습:"), "{rule}");
            assert_eq!(dst, kt.join("KT과금내역서_2026-11.txt"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(mock.prompts().len(), before);
}

#[test]
fn without_ai_nothing_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("메모.txt");
    fs::write(&f, "x").unwrap();
    let eng = Engine::new(
        RuleSet::default(),
        Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip),
        IgnoreFilter::new(&[]).unwrap(),
    );
    assert!(matches!(eng.handle(&f, "b").unwrap(), Action::NoRule(_)));
    assert!(f.exists());
}
