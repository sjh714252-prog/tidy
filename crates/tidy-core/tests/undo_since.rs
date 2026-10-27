//! "최근 자동 이동 한꺼번에 되돌리기": every rule/AI move of the last N
//! seconds goes back in one call, the user's own confirmations stay.

use std::fs;

use tidy_core::config::DuplicatePolicy;
use tidy_core::mover::{MoveRequest, Mover};
use tidy_core::store::{DecidedBy, Store};

fn req(src: &std::path::Path, dest: &std::path::Path, by: DecidedBy, batch: &str) -> MoveRequest {
    MoveRequest {
        src: src.to_path_buf(),
        dest_dir: dest.to_path_buf(),
        batch_id: batch.into(),
        decided_by: by,
        rule_name: None,
        ai_confidence: None,
        new_name: None,
        encrypt: false,
        on_duplicate: None,
    }
}

#[test]
fn undo_since_reverts_auto_moves_and_keeps_user_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let dest = base.join("정리").join("기타").join("개발");
    fs::create_dir_all(&desk).unwrap();
    let names = ["a.go", "b.go", "c.go", "내가 옮김.txt"];
    for n in names {
        fs::write(desk.join(n), n).unwrap();
    }
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    // Watch-time automation: one batch per file, like the service does.
    mover
        .move_file(&req(&desk.join("a.go"), &dest, DecidedBy::Ai, "b1"))
        .unwrap();
    mover
        .move_file(&req(&desk.join("b.go"), &dest, DecidedBy::Rule, "b2"))
        .unwrap();
    mover
        .move_file(&req(&desk.join("c.go"), &dest, DecidedBy::Ai, "b3"))
        .unwrap();
    mover
        .move_file(&req(
            &desk.join("내가 옮김.txt"),
            &dest,
            DecidedBy::User,
            "b4",
        ))
        .unwrap();
    assert_eq!(fs::read_dir(&desk).unwrap().count(), 0);

    let now = chrono::Utc::now().timestamp();
    assert_eq!(
        mover
            .store()
            .active_moves_since(now - 60, true)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        mover
            .store()
            .active_moves_since(now - 60, false)
            .unwrap()
            .len(),
        4
    );

    let r = mover.undo_since(now - 60, true).unwrap();
    assert_eq!(r.restored.len(), 3, "{r:?}");
    assert!(r.conflicts.is_empty(), "{r:?}");
    for n in ["a.go", "b.go", "c.go"] {
        assert!(desk.join(n).exists(), "{n} back on the desktop");
    }
    assert!(
        dest.join("내가 옮김.txt").exists(),
        "user's own move untouched"
    );
    // Nothing left to undo automatically; a second call is a no-op.
    assert_eq!(
        mover
            .store()
            .active_moves_since(now - 60, true)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(mover.undo_since(now - 60, true).unwrap().restored.len(), 0);
    // Older than the window → not touched.
    assert_eq!(
        mover
            .store()
            .active_moves_since(now + 10, false)
            .unwrap()
            .len(),
        0
    );
}
