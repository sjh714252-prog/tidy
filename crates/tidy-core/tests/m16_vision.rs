//! M16 with `MockProvider` only (no real API calls): pictures are marked
//! while indexing, a background pass appends the model's description to
//! the body (word search finds the picture by what it shows), copies reuse
//! the answer by content hash, the daily limit holds the rest back,
//! pictures judged personal are never sent, recognized text and the
//! description live side by side, and a new picture on the desktop is
//! described before the AI picks its folder → moved → indexed (no second
//! call) → undone.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, RuleSet};
use tidy_core::config::{Config, DuplicatePolicy};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::index::vision::{self, VisionDescriber};
use tidy_core::index::{IndexOptions, Indexer};
use tidy_core::mover::Mover;
use tidy_core::search::{search, Query, SearchOptions};
use tidy_core::store::{Store, TextStatus, VisionState};
use tidy_core::watcher::IgnoreFilter;

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// A noisy JPEG (large enough to be a picture, not an icon). `seed`
/// changes the content (and so the hash).
fn picture(path: &Path, seed: u32) {
    let img = image::RgbImage::from_fn(400, 300, |x, y| {
        let v = (x.wrapping_mul(31) ^ y.wrapping_mul(17) ^ seed.wrapping_mul(97)) % 251;
        image::Rgb([v as u8, (v * 3 % 251) as u8, (v * 7 % 251) as u8])
    });
    img.save(path).unwrap();
    assert!(fs::metadata(path).unwrap().len() > 10 * 1024);
}

/// Describes by file name: "영수증" pictures are receipts, "빈" ones are
/// empty, "깨진" ones are refused (bad answer).
fn vision_mock() -> Arc<MockProvider> {
    Arc::new(MockProvider::responder(|_sys, user| {
        if user.contains("빈 화면") {
            json!({"caption": "", "tags": [], "kind": "other"})
        } else if user.contains("깨진") {
            json!({"nope": true})
        } else if user.contains("영수증") || user.contains("IMG_2041") {
            json!({"caption": "카페에서 받은 종이 영수증", "tags": ["영수증", "카페", "경비"], "kind": "receipt"})
        } else {
            json!({"caption": "회의실 화이트보드에 그린 서버 구성도", "tags": ["화이트보드", "회의"], "kind": "diagram"})
        }
    }))
}

fn vision_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.ai.enabled = true;
    cfg.vision.enabled = true;
    cfg
}

fn names(store: &Store, q: &str) -> Vec<String> {
    let mut v: Vec<String> = search(store, &Query::parse(q), &SearchOptions::default())
        .unwrap()
        .into_iter()
        .map(|h| h.file.name)
        .collect();
    v.sort();
    v
}

fn vstate(store: &Store, p: &Path) -> Option<VisionState> {
    store.get_file(p).unwrap().unwrap().vision
}

#[test]
fn mark_describe_search_cache_limit_personal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("바탕 화면");
    fs::create_dir(&root).unwrap();
    picture(&root.join("영수증 1.jpg"), 1);
    fs::copy(root.join("영수증 1.jpg"), root.join("영수증 사본.jpg")).unwrap();
    picture(&root.join("회의.png"), 2);
    picture(&root.join("빈 화면.jpg"), 3);
    picture(&root.join("깨진.jpg"), 4);
    // The name alone is personal information (M9): never sent.
    picture(&root.join("주민등록번호 900101-1234568 사본.jpg"), 5);
    fs::write(root.join("아이콘.png"), b"\x89PNG tiny").unwrap();
    fs::write(root.join("메모.txt"), "그냥 메모").unwrap();

    let store = Store::open(&tmp.path().join("tidy.db")).unwrap();
    let mut cfg = vision_cfg();
    let mut opts = IndexOptions::from_config(&cfg);
    opts.pii = Some(Arc::new(tidy_core::pii::Detector::lenient(
        &cfg.security.pii,
    )));
    Indexer::new(&store)
        .index_root(&root, &opts, |_, _| {})
        .unwrap();
    assert_eq!(
        vstate(&store, &root.join("영수증 1.jpg")),
        Some(VisionState::Pending)
    );
    assert_eq!(vstate(&store, &root.join("아이콘.png")), None);
    assert_eq!(vstate(&store, &root.join("메모.txt")), None);
    assert_eq!(store.vision_counts().unwrap().pending, 6);
    assert!(names(&store, "화이트보드").is_empty());

    // Daily limit 2: two calls today, the rest waits.
    cfg.vision.daily_limit = 2;
    let mock = vision_mock();
    let d = VisionDescriber::with_provider(mock.clone(), &cfg).unwrap();
    let r = vision::run_pass(&cfg, &store, &d, 100, false, |_| {}).unwrap();
    assert!(r.limited, "{r:?}");
    assert_eq!(r.skipped, 1, "personal picture skipped");
    assert_eq!(mock.images().len(), 2);
    assert_eq!(
        vstate(&store, &root.join("주민등록번호 900101-1234568 사본.jpg")),
        Some(VisionState::Skipped)
    );

    // No limit: everything else is done; the copy comes from the cache.
    cfg.vision.daily_limit = 100;
    let r = vision::run_pass(&cfg, &store, &d, 100, false, |_| {}).unwrap();
    assert!(!r.limited && r.stopped.is_none(), "{r:?}");
    assert_eq!(r.remaining, 0);
    let c = store.vision_counts().unwrap();
    assert_eq!((c.ok, c.empty, c.failed, c.skipped), (3, 1, 1, 1), "{c:?}");
    assert_eq!(mock.images().len(), 4, "the copy was not sent again");
    // Every picture went shrunk or as it is, never more than one per call.
    assert!(mock.images().iter().all(|(t, _)| t.starts_with("image/")));

    // Found by what it shows (caption and tags), not only by name.
    assert_eq!(names(&store, "화이트보드"), vec!["회의.png"]);
    assert_eq!(
        names(&store, "경비"),
        vec!["영수증 1.jpg", "영수증 사본.jpg"]
    );
    let f = store.get_file(&root.join("회의.png")).unwrap().unwrap();
    assert_eq!(f.text_status, TextStatus::Ok);
    assert!(f
        .excerpt
        .as_deref()
        .unwrap()
        .starts_with("[사진 설명] 회의실 화이트보드"));

    // The log has the kind, never the description.
    let log = store.recent_ai_log(10).unwrap();
    assert!(log.iter().all(|l| l.purpose == "vision"));
    assert!(log
        .iter()
        .filter_map(|l| l.result.as_deref())
        .all(|r| !r.contains("화이트보드")));

    // Failed ones can be retried.
    assert_eq!(store.vision_retry_failed().unwrap(), 1);
    let st = vision::status(&cfg, &store).unwrap();
    assert_eq!(st.counts.pending, 1);
    assert_eq!(
        st.today, 3,
        "answered calls today (the refused one is not counted)"
    );
}

#[test]
fn recognized_text_and_description_live_side_by_side() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap();
    let p = root.join("견적 캡처.png");
    picture(&p, 9);
    let store = Store::open_in_memory().unwrap();
    let cfg = vision_cfg();
    Indexer::new(&store)
        .index_root(&root, &IndexOptions::from_config(&cfg), |_, _| {})
        .unwrap();
    let id = store.get_file(&p).unwrap().unwrap().id;
    // Description first, recognized text later (M15) …
    store
        .set_vision_caption(id, "견적서 화면 캡처 (태그: 견적서)")
        .unwrap();
    store.set_ocr_text(id, "가람정보 견적 2026", None).unwrap();
    let body = store.file_content(id).unwrap().unwrap();
    assert_eq!(
        body,
        "가람정보 견적 2026\n\n[사진 설명] 견적서 화면 캡처 (태그: 견적서)"
    );
    // … and a new description keeps the text.
    store.set_vision_caption(id, "다른 설명").unwrap();
    assert_eq!(
        store.file_content(id).unwrap().unwrap(),
        "가람정보 견적 2026\n\n[사진 설명] 다른 설명"
    );
    assert_eq!(names(&store, "가람정보"), vec!["견적 캡처.png"]);
    assert_eq!(names(&store, "다른 설명"), vec!["견적 캡처.png"]);
}

#[test]
fn off_by_default_and_never_without_send_content() {
    let mut cfg = Config::default();
    assert!(VisionDescriber::from_config(&cfg).is_err());
    assert!(vision::config_blocker(&cfg).is_some());
    cfg.vision.enabled = true;
    cfg.ai.enabled = true;
    cfg.ai.send_content = false;
    let why = vision::config_blocker(&cfg).unwrap();
    assert!(why.contains("send_content"), "{why}");
    assert!(VisionDescriber::from_config(&cfg).is_err());
}

#[test]
fn new_picture_is_described_then_sorted_indexed_and_undone() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let docs = base.join("정리");
    let receipts = docs.join("경비").join("영수증");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(&receipts).unwrap();
    let rules = RuleSet::from_toml(&format!(
        "[[rule]]\nname = \"정리\"\nmatch = {{ ext = [\"never\"] }}\ndest = \"{}/{{yyyy}}\"\nai_auto = true\n",
        toml_path(&docs)
    ))
    .unwrap();
    let cfg = vision_cfg();

    // The classify model picks the receipts folder only when it is told
    // the picture shows a receipt.
    let receipts2 = receipts.clone();
    let classify = Arc::new(MockProvider::responder(move |_sys, user| {
        let dest = if user.contains("[사진 설명] 카페에서 받은 종이 영수증") {
            receipts2.display().to_string()
        } else {
            "/nowhere".into()
        };
        json!({"dest": dest, "confidence": 0.95, "reason": "사진이 영수증", "new_folder": false, "suggested_rename": null})
    }));
    let vmock = vision_mock();
    let ai = AiClassifier::with_provider(classify.clone(), &cfg, &rules)
        .unwrap()
        .with_vision(VisionDescriber::with_provider(vmock.clone(), &cfg).unwrap());
    let db = base.join("tidy.db");
    let mover = Mover::new(Store::open(&db).unwrap(), DuplicatePolicy::Skip);
    let eng = Engine::new(
        rules.clone(),
        mover,
        IgnoreFilter::new(&cfg.classify.ignore_globs).unwrap(),
    )
    .with_ai(ai)
    .with_index(IndexOptions::from_config(&cfg));

    // A camera name says nothing; the picture does.
    let shot = desk.join("IMG_2041.jpg");
    picture(&shot, 7);
    let batch = new_batch_id();
    let dst = match eng.handle(&shot, &batch).unwrap() {
        Action::Moved { dst, .. } => dst,
        other => panic!("{other:?}"),
    };
    assert!(dst.starts_with(&receipts), "{}", dst.display());
    assert_eq!(vmock.images().len(), 1);
    let (_, user) = classify.prompts().pop().unwrap();
    assert!(user.contains("태그: 영수증, 카페, 경비"), "{user}");

    // Indexing the moved file takes the description from the cache.
    let store = Store::open(&db).unwrap();
    Indexer::new(&store)
        .index_root(&receipts, &IndexOptions::from_config(&cfg), |_, _| {})
        .unwrap();
    assert_eq!(vstate(&store, &dst), Some(VisionState::Ok));
    assert_eq!(names(&store, "영수증"), vec!["IMG_2041.jpg"]);
    assert_eq!(vmock.images().len(), 1, "not sent again");

    // Undo puts it back.
    let undo = eng.mover().undo_batch(&batch).unwrap();
    assert_eq!(undo.restored.len(), 1, "{:?}", undo.conflicts);
    assert!(shot.exists() && !dst.exists());
}
