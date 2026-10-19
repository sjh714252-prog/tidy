//! M8: per-folder organize mode (SPEC §14.1). One watch folder organized
//! in place, one collected under another folder; the same relative
//! starter rules serve both, and every move is undoable.

use std::fs;
use std::path::Path;

use tidy_core::classify::starter::starter_cards;
use tidy_core::classify::{apply_cards, Anchors, RuleSet};
use tidy_core::config::{DuplicatePolicy, OrganizeMode, WatchRoot};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::store::Store;
use tidy_core::watcher::IgnoreFilter;

fn ym(p: &Path) -> String {
    let t: chrono::DateTime<chrono::Local> = fs::metadata(p).unwrap().modified().unwrap().into();
    t.format("%Y-%m").to_string()
}

#[test]
fn in_place_and_collect_with_the_same_relative_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("홍 길동").join("바탕 화면");
    let down = base.join("홍 길동").join("다운로드");
    let sorted = base.join("정리 보관함");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(&down).unwrap();

    let text = apply_cards("", &starter_cards(&["documents".into(), "archives".into()])).unwrap();
    let rules = RuleSet::from_toml(&text).unwrap();
    let watches = vec![
        WatchRoot {
            path: desk.clone(),
            ..Default::default()
        },
        WatchRoot {
            path: down.clone(),
            mode: OrganizeMode::Collect,
            organize_root: Some(sorted.clone()),
            ..Default::default()
        },
    ];
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let engine = Engine::new(rules, mover, IgnoreFilter::new(&[]).unwrap())
        .with_anchors(Anchors::from_watches(&watches));

    let pdf = desk.join("분기 보고서.pdf");
    let zip = down.join("사진 모음.zip");
    fs::write(&pdf, "pdf").unwrap();
    fs::write(&zip, "zip").unwrap();
    let m_pdf = ym(&pdf);

    let batch = new_batch_id();
    let a = engine.handle(&pdf, &batch).unwrap();
    let b = engine.handle(&zip, &batch).unwrap();
    let in_place = desk.join("문서").join(&m_pdf).join("분기 보고서.pdf");
    let collected = sorted.join("압축 파일").join("사진 모음.zip");
    assert!(
        matches!(a, Action::Moved { ref dst, .. } if *dst == in_place),
        "{a:?}"
    );
    assert!(
        matches!(b, Action::Moved { ref dst, .. } if *dst == collected),
        "{b:?}"
    );
    assert!(in_place.is_file() && collected.is_file());

    // The moved pdf, seen again inside the (in-place) folder, stays.
    let again = engine.handle(&in_place, &new_batch_id()).unwrap();
    assert!(!matches!(again, Action::Moved { .. }), "{again:?}");

    let report = engine.mover().undo_batch(&batch).unwrap();
    assert_eq!(report.restored.len(), 2, "{report:?}");
    assert!(pdf.is_file() && zip.is_file());
}
