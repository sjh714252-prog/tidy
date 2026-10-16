//! M3 core flow (what the desktop app drives): run_watch in the background
//! → rule move is searchable under its new path → confirm=true file waits
//! in the queue → user resolves it → undo puts the index back.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tidy_core::config::{Config, DuplicatePolicy};
use tidy_core::engine::{reindex_undo, Action};
use tidy_core::index::IndexOptions;
use tidy_core::mover::{ExpectedMoves, Mover};
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::search::{search, Query, SearchOptions};
use tidy_core::service::{run_watch, WatchEvent, WatchRunOptions};
use tidy_core::store::Store;

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

fn hits(store: &Store, q: &str) -> Vec<std::path::PathBuf> {
    search(store, &Query::parse(q), &SearchOptions::default())
        .unwrap()
        .into_iter()
        .map(|h| h.file.path)
        .collect()
}

#[test]
fn watch_index_confirm_undo() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let sorted = base.join("정리 결과");
    fs::create_dir_all(&desk).unwrap();
    fs::write(
        base.join("config.toml"),
        format!(
            "[[watch]]\npath = \"{}\"\n[classify]\nstabilize_secs = 1\n[store]\npath = \"tidy.db\"\n",
            toml_path(&desk)
        ),
    )
    .unwrap();
    fs::write(
        base.join("rules.toml"),
        format!(
            "[[rule]]\nname = \"메모\"\nmatch = {{ ext = [\"txt\"] }}\ndest = \"{0}/메모\"\n\n\
             [[rule]]\nname = \"설치파일\"\nmatch = {{ ext = [\"exe\"] }}\ndest = \"{0}/설치\"\nconfirm = true\n",
            toml_path(&sorted)
        ),
    )
    .unwrap();
    let cfg = Config::load(&base.join("config.toml")).unwrap();

    let expected = ExpectedMoves::new();
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<String>();
    let worker = {
        let (cfg, expected, stop) = (cfg.clone(), expected.clone(), stop.clone());
        std::thread::spawn(move || {
            let opts = WatchRunOptions {
                index: true,
                ..Default::default()
            };
            run_watch(&cfg, &opts, expected, &stop, |ev| {
                let msg = match ev {
                    WatchEvent::Started(_) => "started".to_string(),
                    WatchEvent::Ai(_) => "ai".to_string(),
                    WatchEvent::Handled(_, Ok(Action::Moved { .. })) => "moved".into(),
                    WatchEvent::Handled(_, Ok(Action::Pending { .. })) => "pending".into(),
                    WatchEvent::Handled(p, other) => format!("{} {other:?}", p.display()),
                    _ => "other".into(),
                };
                let _ = tx.send(msg);
            })
            .unwrap();
        })
    };
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), "started");

    fs::write(desk.join("보안점검 메모.txt"), "수탁자 보안점검 체크리스트").unwrap();
    fs::write(desk.join("설치 프로그램.exe"), b"MZ installer").unwrap();
    let mut got: Vec<String> = (0..2)
        .map(|_| rx.recv_timeout(Duration::from_secs(20)).unwrap())
        .collect();
    got.sort();
    assert_eq!(got, ["moved", "pending"]);

    // The app's own connection (search + confirm popup) shares `expected`.
    let ui = Mover::new(
        Store::open(&cfg.store.db_path()).unwrap(),
        DuplicatePolicy::Skip,
    )
    .with_expected(expected);
    let moved = sorted.join("메모").join("보안점검 메모.txt");
    assert!(moved.is_file());
    // Searchable at its new place only; the pending file is indexed in place.
    assert_eq!(hits(ui.store(), "보안점검"), vec![moved.clone()]);
    assert_eq!(hits(ui.store(), "설치 ext:exe").len(), 1);

    // Confirm popup: "다른 곳 선택".
    let pending = ui.store().list_pending().unwrap();
    assert_eq!(pending.len(), 1);
    let elsewhere = base.join("내 프로그램");
    let idx = IndexOptions::from_config(&cfg);
    let r = resolve(
        &ui,
        pending[0].id,
        &Decision::Move {
            dest_dir: Some(elsewhere.clone()),
            new_name: None,
        },
        "ui-batch",
        Some(&idx),
    )
    .unwrap();
    let Resolution::Moved { dst, .. } = r else {
        panic!("{r:?}")
    };
    assert!(dst.starts_with(&elsewhere));
    assert_eq!(hits(ui.store(), "설치 ext:exe"), vec![dst.clone()]);
    // The watcher must not treat the popup's move as a new file.
    assert!(rx.recv_timeout(Duration::from_secs(3)).is_err());

    // Undo the popup move: file and index row go back.
    let report = ui.undo_last().unwrap().unwrap();
    reindex_undo(ui.store(), &report, &idx);
    let back = desk.join("설치 프로그램.exe");
    assert!(back.is_file());
    assert_eq!(hits(ui.store(), "설치 ext:exe"), vec![back]);

    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
}
