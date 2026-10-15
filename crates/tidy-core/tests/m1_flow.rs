//! M1 end-to-end: watch folder → new file → rule → move → undo (SPEC §12).
//! Folder and file names contain Korean and spaces on purpose.

use std::fs;
use std::path::Path;
use std::time::Duration;

use tidy_core::classify::RuleSet;
use tidy_core::config::{Config, DuplicatePolicy};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::store::Store;
use tidy_core::watcher::{IgnoreFilter, WatchOptions, Watcher};

fn toml_path(p: &Path) -> String {
    // Forward slashes are valid on Windows too and need no TOML escaping.
    p.display().to_string().replace('\\', "/")
}

#[test]
fn watch_move_undo_full_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("홍 길동").join("바탕 화면");
    let sorted = base.join("정리 된 파일");
    fs::create_dir_all(&desk).unwrap();

    let cfg_dir = base.join("설정 폴더");
    fs::create_dir(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.toml"),
        format!(
            "[[watch]]\npath = \"{}\"\n\n[classify]\nstabilize_secs = 1\n\n[store]\npath = \"tidy.db\"\n",
            toml_path(&desk)
        ),
    )
    .unwrap();
    fs::write(
        cfg_dir.join("rules.toml"),
        format!(
            r#"
[[rule]]
name = "스크린샷"
match = {{ ext = ["png"], name_regex = "^(스크린샷|Screenshot|화면 캡처)" }}
dest = "{}/스크린샷/{{yyyy}}"

[[rule]]
name = "설치파일"
match = {{ ext = ["exe"] }}
dest = "{}/설치 파일"
confirm = true
"#,
            toml_path(&sorted),
            toml_path(&sorted)
        ),
    )
    .unwrap();

    let cfg = Config::load(&cfg_dir.join("config.toml")).unwrap();
    let rules = RuleSet::load(&cfg.classify.rules).unwrap();
    let db = cfg.store.db_path();
    let mover = Mover::new(Store::open(&db).unwrap(), DuplicatePolicy::Skip);
    let expected = mover.expected();
    let engine = Engine::new(
        rules,
        mover,
        IgnoreFilter::new(&cfg.classify.ignore_globs).unwrap(),
    );
    let (watcher, rx) = Watcher::start(
        &cfg.watches,
        WatchOptions::from_config(&cfg.classify),
        expected,
    )
    .unwrap();
    let root = watcher.roots()[0].clone();

    // 1) a screenshot appears → moved by rule.
    let shot = root.join("화면 캡처 2026-10-15 오전 10.png");
    fs::write(&shot, b"\x89PNG fake").unwrap();
    // 2) an installer appears → confirm=true → pending, not moved.
    let exe = root.join("설치 프로그램 v1.exe");
    fs::write(&exe, b"MZ").unwrap();
    // 3) a partial download is ignored.
    fs::write(root.join("영상.mp4.crdownload"), b"..").unwrap();

    let mut moved_to = None;
    let mut pending_seen = false;
    for _ in 0..2 {
        let ev = rx.recv_timeout(Duration::from_secs(20)).expect("event");
        match engine.handle(&ev.path, &new_batch_id()).unwrap() {
            Action::Moved { src, dst, rule, .. } => {
                assert_eq!(src, shot);
                assert_eq!(rule, "스크린샷");
                moved_to = Some(dst);
            }
            Action::Pending { path, .. } => {
                assert_eq!(path, exe);
                pending_seen = true;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    let dst = moved_to.expect("screenshot moved");
    assert!(pending_seen);
    assert!(!shot.exists());
    assert!(dst.starts_with(sorted.join("스크린샷")));
    assert!(dst.is_file());
    assert!(exe.exists(), "confirm rule must not move");
    assert_eq!(engine.mover().store().list_pending().unwrap().len(), 1);
    // Our own move must not come back as an event.
    assert!(rx.recv_timeout(Duration::from_secs(3)).is_err());

    // `tidy undo --last` runs as a separate process with its own connection.
    {
        let undo_mover = Mover::new(Store::open(&db).unwrap(), DuplicatePolicy::Skip);
        let r = undo_mover.undo_last().unwrap().expect("something to undo");
        assert_eq!(r.restored, vec![(dst.clone(), shot.clone())]);
        assert!(r.conflicts.is_empty());
    }
    assert!(shot.exists());
    assert!(!dst.exists());

    // The restored file may or may not reach the watcher (the in-process
    // "expected move" window can swallow it); either way the engine must
    // leave it in place because the DB says it was undone.
    if let Ok(ev) = rx.recv_timeout(Duration::from_secs(4)) {
        assert_eq!(ev.path, shot);
    }
    assert!(matches!(
        engine.handle(&shot, &new_batch_id()).unwrap(),
        Action::Skipped { .. }
    ));
    assert!(shot.exists());

    // A *different* file with the same name is organised again.
    fs::write(&shot, b"\x89PNG another one").unwrap();
    assert!(matches!(
        engine.handle(&shot, &new_batch_id()).unwrap(),
        Action::Moved { .. }
    ));
    let m = Mover::new(Store::open(&db).unwrap(), DuplicatePolicy::Skip);
    m.undo_last().unwrap().unwrap();
    assert!(shot.exists());

    // Nothing left to undo.
    assert!(m.undo_last().unwrap().is_none());
}

#[test]
fn example_config_and_rules_parse() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config::load(&root.join("config.example.toml")).unwrap();
    assert!(!cfg.watches.is_empty());
    let rules = RuleSet::load(&root.join("rules.example.toml")).unwrap();
    assert!(!rules.is_empty());
}
