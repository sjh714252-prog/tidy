//! M5 end to end (no AI needed): report with duplicates → rules plan →
//! saved → partial apply as one batch (search index follows) → history →
//! undo the whole restructure at once. Korean names with spaces.

use std::fs;

use tidy_core::classify::RuleSet;
use tidy_core::config::DuplicatePolicy;
use tidy_core::index::IndexOptions;
use tidy_core::mover::Mover;
use tidy_core::report::{self, restructure, ReportOptions};
use tidy_core::search::{search, Query, SearchOptions};
use tidy_core::store::{DecidedBy, Store};

#[test]
fn report_plan_apply_history_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    fs::create_dir_all(desk.join("받은 파일")).unwrap();
    let body = vec![42u8; 3000];
    fs::write(desk.join("계약서 최종.pdf"), &body).unwrap();
    fs::write(desk.join("받은 파일").join("계약서 최종 (1).pdf"), &body).unwrap();
    fs::write(desk.join("회의 메모.txt"), "수탁자 보안점검 회의").unwrap();
    fs::write(desk.join("스크린샷 2026.png"), "png").unwrap();

    let store = Store::open(&base.join("tidy.db")).unwrap();
    let r = report::build(&desk, &ReportOptions::default(), Some(&store)).unwrap();
    assert_eq!(r.stats.total_files, 4);
    assert_eq!(r.duplicates.len(), 1);
    assert_eq!(r.duplicates[0].wasted, 3000);

    let b = base.display().to_string().replace('\\', "/");
    let rules = RuleSet::from_toml(&format!(
        "[[rule]]\nname = \"문서\"\nmatch = {{ ext = [\"pdf\", \"txt\"] }}\ndest = \"{b}/바탕 화면/문서\"\n\n\
         [[rule]]\nname = \"사진\"\nmatch = {{ ext = [\"png\"] }}\ndest = \"{b}/사진\"\n"
    ))
    .unwrap();
    let plan = restructure::plan_from_rules(&desk, &rules, false, &[], &[]).unwrap();
    assert_eq!(plan.moves.len(), 3); // root-level files only (not recursive)
    let plan = restructure::save(&store, plan).unwrap();

    // Approve everything except the screenshot.
    let keep: Vec<u32> = plan
        .moves
        .iter()
        .filter(|m| !m.reason.contains("사진"))
        .map(|m| m.id)
        .collect();
    let p = restructure::preview(&plan, Some(&keep)).unwrap();
    assert_eq!(p.files_moved, 2);
    assert!(p.after.contains("문서"));

    let mover = Mover::new(store, DuplicatePolicy::Skip);
    let idx = IndexOptions::default();
    let applied = restructure::apply(&mover, &plan, Some(&keep), &[], Some(&idx)).unwrap();
    assert_eq!(applied.moved.len(), 2);
    assert!(desk.join("문서").join("회의 메모.txt").is_file());
    assert!(desk.join("스크린샷 2026.png").is_file());
    let hits = search(
        mover.store(),
        &Query::parse("보안점검"),
        &SearchOptions::default(),
    )
    .unwrap();
    assert_eq!(hits[0].file.path, desk.join("문서").join("회의 메모.txt"));

    let hist = mover.store().batches(10).unwrap();
    assert_eq!(hist[0].batch_id, applied.batch_id);
    assert_eq!(hist[0].moves, 2);
    assert_eq!(hist[0].decided_by, DecidedBy::Restructure.as_str());

    let u = mover.undo_batch(&applied.batch_id).unwrap();
    assert_eq!(u.restored.len(), 2);
    assert!(desk.join("회의 메모.txt").is_file());
    assert!(desk.join("계약서 최종.pdf").is_file());
    assert_eq!(mover.store().batches(10).unwrap()[0].undone, 2);
}
