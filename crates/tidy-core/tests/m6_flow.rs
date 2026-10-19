//! M6: NAS folders. Completion criterion (SPEC §10): a network watch folder
//! keeps working across a disconnect/reconnect — it is polled, pauses only
//! itself while unreachable, picks up files added meanwhile once it is
//! back, and moves out of it are copy-verified, logged and undoable.
//! The "share" is a temp folder; unmounting is simulated by renaming it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use tidy_core::classify::RuleSet;
use tidy_core::config::{Config, DuplicatePolicy, RootKind, WatchRoot};
use tidy_core::engine::{Action, Engine};
use tidy_core::index::{jobs_from_config, IndexOptions, Indexer};
use tidy_core::mover::{ExpectedMoves, MoveOutcome, MoveRequest, Mover, TransferProgress};
use tidy_core::service::{run_watch, WatchEvent, WatchRunOptions};
use tidy_core::store::{DecidedBy, Store, TextStatus};
use tidy_core::watcher::{IgnoreFilter, WatchOptions};

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// Windows refuses to rename a folder while another thread briefly has it
/// open (a listing in progress).
fn rename_retry(from: &Path, to: &Path) {
    for _ in 0..50 {
        if fs::rename(from, to).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    fs::rename(from, to).unwrap();
}

#[derive(Debug)]
enum Seen {
    Started,
    Moved(PathBuf, PathBuf),
    Online(bool),
    Other,
}

fn wait<T>(rx: &mpsc::Receiver<Seen>, what: &str, mut f: impl FnMut(&Seen) -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(20);
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ev) => {
                if let Some(v) = f(&ev) {
                    return v;
                }
            }
            Err(_) => break,
        }
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn nas_folder_survives_disconnect_and_reconnect() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let nas = base.join("NAS 공유").join("스캔 받은 문서");
    let archive = base.join("내 PC 보관함");
    fs::create_dir_all(&nas).unwrap();
    fs::write(
        base.join("config.toml"),
        format!(
            "[[watch]]\npath = \"{}\"\nkind = \"network\"\npoll_only = true\n\
             [classify]\nstabilize_secs = 0\n[store]\npath = \"tidy.db\"\n",
            toml_path(&nas)
        ),
    )
    .unwrap();
    fs::write(
        base.join("rules.toml"),
        format!(
            "[[rule]]\nname = \"스캔\"\nmatch = {{ ext = [\"pdf\"] }}\ndest = \"{}/{{yyyy}}\"\n",
            toml_path(&archive)
        ),
    )
    .unwrap();
    let cfg = Config::load(&base.join("config.toml")).unwrap();
    assert!(
        !cfg.watches[0].index_content,
        "NAS = metadata only by default"
    );

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Seen>();
    let worker = {
        let (cfg, stop) = (cfg.clone(), stop.clone());
        std::thread::spawn(move || {
            let opts = WatchRunOptions {
                index: true,
                watch_options: Some(WatchOptions {
                    stabilize: Duration::ZERO,
                    tick: Duration::from_millis(100),
                    reconnect: Duration::from_millis(300),
                    poll_override: Some(Duration::from_millis(300)),
                    ..WatchOptions::from_config(&cfg.classify)
                }),
                ..Default::default()
            };
            run_watch(&cfg, &opts, ExpectedMoves::new(), &stop, |ev| {
                let seen = match ev {
                    WatchEvent::Started(_) => Seen::Started,
                    WatchEvent::Handled(_, Ok(Action::Moved { src, dst, .. })) => {
                        Seen::Moved(src.clone(), dst.clone())
                    }
                    WatchEvent::Root(r) => Seen::Online(r.online),
                    WatchEvent::Handled(..) => Seen::Other,
                    _ => Seen::Other,
                };
                let _ = tx.send(seen);
            })
            .unwrap();
        })
    };
    wait(&rx, "start", |e| matches!(e, Seen::Started).then_some(()));
    // Let the baseline listing happen.
    std::thread::sleep(Duration::from_millis(500));

    // 1) A scan lands on the NAS → polled → moved off the share.
    let scan = nas.join("스캔 2026-10-15 (1).pdf");
    fs::write(&scan, vec![1u8; 300_000]).unwrap();
    let (src, dst) = wait(&rx, "first move", |e| match e {
        Seen::Moved(s, d) => Some((s.clone(), d.clone())),
        _ => None,
    });
    assert_eq!(src, scan);
    assert!(dst.starts_with(&archive) && dst.is_file() && !scan.exists());

    // 2) The share drops: only this folder pauses.
    let unplugged = base.join("NAS 공유 (연결 끊김)");
    rename_retry(&base.join("NAS 공유"), &unplugged);
    wait(&rx, "offline", |e| {
        matches!(e, Seen::Online(false)).then_some(())
    });
    // A colleague saves a file on the NAS while we are disconnected.
    let during = unplugged.join("스캔 받은 문서").join("끊긴 동안 스캔.pdf");
    fs::write(&during, b"%PDF").unwrap();

    // 3) Reconnect → the file added meanwhile is handled.
    rename_retry(&unplugged, &base.join("NAS 공유"));
    wait(&rx, "online", |e| {
        matches!(e, Seen::Online(true)).then_some(())
    });
    let (src2, dst2) = wait(&rx, "move after reconnect", |e| match e {
        Seen::Moved(s, d) => Some((s.clone(), d.clone())),
        _ => None,
    });
    assert_eq!(src2, nas.join("끊긴 동안 스캔.pdf"));
    assert!(dst2.is_file());

    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();

    // 4) Both moves are logged and undo brings them back onto the share.
    let store = Store::open(&cfg.store.db_path()).unwrap();
    let mover = Mover::new(store, DuplicatePolicy::Skip);
    let moves = mover.store().recent_moves(10).unwrap();
    assert_eq!(moves.len(), 2);
    assert!(moves
        .iter()
        .all(|m| !m.in_progress && m.undone_at.is_none()));
    for _ in 0..2 {
        mover.undo_last().unwrap().unwrap();
    }
    assert!(scan.is_file() && nas.join("끊긴 동안 스캔.pdf").is_file());
}

#[test]
fn copy_verify_move_to_share_reports_progress_and_undoes() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let nas = base.join("NAS 공유").join("영상 보관");
    fs::create_dir_all(&desk).unwrap();
    let src = desk.join("회의 녹화 2026-10.mp4");
    let data: Vec<u8> = (0..8_000_000u32).map(|i| (i % 253) as u8).collect();
    fs::write(&src, &data).unwrap();

    let seen: Arc<std::sync::Mutex<Vec<TransferProgress>>> = Arc::default();
    let s2 = seen.clone();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
        .copy_always(true)
        .with_progress(Arc::new(move |p| s2.lock().unwrap().push(p.clone())));
    let out = mover
        .move_file(&MoveRequest {
            src: src.clone(),
            dest_dir: nas.clone(),
            batch_id: "nas".into(),
            decided_by: DecidedBy::User,
            rule_name: None,
            ai_confidence: None,
            new_name: None,
            encrypt: false,
        })
        .unwrap();
    let MoveOutcome::Moved { dst, .. } = out else {
        panic!("{out:?}")
    };
    assert_eq!(fs::read(&dst).unwrap(), data);
    assert!(!src.exists());
    // Only the moved file is left on the share (no scratch copies).
    assert_eq!(fs::read_dir(&nas).unwrap().count(), 1);
    // Progress reports are monotonic and end at the full size, if the copy
    // was slow enough to report at all.
    let seen = seen.lock().unwrap();
    assert!(seen.windows(2).all(|w| w[0].copied <= w[1].copied));
    if let Some(last) = seen.last() {
        assert_eq!((last.copied, last.phase), (data.len() as u64, "done"));
    }
    let r = mover.undo_last().unwrap().unwrap();
    assert_eq!(r.restored.len(), 1);
    assert_eq!(fs::read(&src).unwrap(), data);
}

#[test]
fn nas_index_is_metadata_only_and_hashes_small_files() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let nas = base.join("NAS 공유");
    fs::create_dir_all(&nas).unwrap();
    fs::write(nas.join("회의록.txt"), "분기 실적 회의록").unwrap();
    fs::write(nas.join("큰 파일.bin"), vec![0u8; 2 * 1024 * 1024]).unwrap();
    let mut cfg = Config::default();
    cfg.index.max_hash_size_mb_network = 1;
    cfg.watches.push(WatchRoot {
        path: nas.clone(),
        kind: RootKind::Network,
        index_content: false,
        ..Default::default()
    });
    let store = Store::open_in_memory().unwrap();
    let indexer = Indexer::new(&store);
    let (path, opts) = jobs_from_config(&cfg, false).remove(0);
    indexer.index_root(&path, &opts, |_, _| {}).unwrap();
    let small = store.get_file(&nas.join("회의록.txt")).unwrap().unwrap();
    assert_eq!(small.text_status, TextStatus::None);
    assert!(small.hash.is_some());
    let big = store.get_file(&nas.join("큰 파일.bin")).unwrap().unwrap();
    assert!(big.hash.is_none(), "over max_hash_size_mb_network");

    // A single file handled on the share keeps the folder's settings even
    // when the caller passes the generic options.
    fs::write(nas.join("새 메모.txt"), "새 메모").unwrap();
    indexer
        .index_file(&nas.join("새 메모.txt"), &IndexOptions::from_config(&cfg))
        .unwrap();
    let f = store.get_file(&nas.join("새 메모.txt")).unwrap().unwrap();
    assert_eq!(f.text_status, TextStatus::None);

    // Turning content on (or entering the content window) extracts text
    // for files indexed as metadata only, although they did not change.
    cfg.watches[0].index_content = true;
    let (path, opts) = jobs_from_config(&cfg, true).remove(0);
    let r = indexer.index_root(&path, &opts, |_, _| {}).unwrap();
    assert_eq!(r.updated, 3);
    let small = store.get_file(&nas.join("회의록.txt")).unwrap().unwrap();
    assert_eq!(small.text_status, TextStatus::Ok);
    let r = indexer.index_root(&path, &opts, |_, _| {}).unwrap();
    assert_eq!(r.unchanged, 3);
}

#[test]
fn files_tidy_placed_on_a_share_are_not_reclassified() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let nas = base.join("NAS 공유");
    let inbox = nas.join("받은 파일");
    fs::create_dir_all(&inbox).unwrap();
    let rules = RuleSet::from_toml(&format!(
        "[[rule]]\nname = \"엑셀\"\nmatch = {{ ext = [\"xlsx\"] }}\ndest = \"{}/엑셀\"\n",
        toml_path(&nas)
    ))
    .unwrap();
    let store = Store::open(&base.join("tidy.db")).unwrap();
    let mover = Mover::new(store, DuplicatePolicy::Skip);
    let engine = Engine::new(rules, mover, IgnoreFilter::new(&[]).unwrap());
    let f = inbox.join("정산.xlsx");
    fs::write(&f, "x").unwrap();
    let Action::Moved { dst, .. } = engine.handle(&f, "b1").unwrap() else {
        panic!()
    };
    // Minutes later the next listing of the share reports the moved file.
    let again = engine.handle(&dst, "b2").unwrap();
    assert!(
        matches!(again, Action::Skipped { reason, .. } if reason.contains("Tidy")),
        "{again:?}"
    );
    // Once the user edits it, it is a new file again.
    fs::write(&dst, "수정됨").unwrap();
    assert!(!matches!(
        engine.handle(&dst, "b3").unwrap(),
        Action::Skipped { .. }
    ));
}
