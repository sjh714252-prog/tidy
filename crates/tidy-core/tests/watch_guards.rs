//! (0.19.0, SPEC §14.29) While watching: a folder the user made and filled
//! is never scattered file by file — it waits in 확인 대기 as ONE item and
//! moves whole (undoable). A file just made to write in ("새 텍스트
//! 문서.txt", or any file first seen empty) is left alone while empty and
//! asked about — not moved — once it has content.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use tidy_core::config::{Config, DuplicatePolicy};
use tidy_core::engine::{Action, FOLDER_RULE};
use tidy_core::mover::{ExpectedMoves, Mover};
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::service::{run_watch, WatchEvent, WatchRunOptions};
use tidy_core::store::Store;

fn toml_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

struct Run {
    base: PathBuf,
    desk: PathBuf,
    sorted: PathBuf,
    cfg: Config,
    expected: Arc<ExpectedMoves>,
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<(PathBuf, String)>,
    worker: Option<std::thread::JoinHandle<()>>,
    _tmp: tempfile::TempDir,
}

impl Run {
    fn start(extra_rules: &str) -> Run {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let desk = base.join("바탕 화면");
        let sorted = base.join("정리");
        fs::create_dir_all(&desk).unwrap();
        fs::create_dir_all(&sorted).unwrap();
        fs::write(
            base.join("config.toml"),
            format!(
                "[[watch]]\npath = \"{}\"\nrecursive = true\nmode = \"collect\"\norganize_root = \"{}\"\n\
                 [classify]\nstabilize_secs = 1\n[curate]\nfolder_settle_secs = 1\n[store]\npath = \"tidy.db\"\n",
                toml_path(&desk),
                toml_path(&sorted)
            ),
        )
        .unwrap();
        fs::write(
            base.join("rules.toml"),
            format!(
                "[[rule]]\nname = \"문서\"\nmatch = {{ ext = [\"pdf\", \"docx\"] }}\ndest = \"문서\"\n\n\
                 [[rule]]\nname = \"텍스트\"\nmatch = {{ ext = [\"txt\"] }}\ndest = \"텍스트\"\n\n\
                 [[rule]]\nname = \"압축 파일\"\nmatch = {{ ext = [\"zip\"] }}\ndest = \"압축 파일\"\n{extra_rules}"
            ),
        )
        .unwrap();
        let cfg = Config::load(&base.join("config.toml")).unwrap();
        let expected = ExpectedMoves::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let worker = {
            let (cfg, expected, stop) = (cfg.clone(), expected.clone(), stop.clone());
            std::thread::spawn(move || {
                run_watch(&cfg, &WatchRunOptions::default(), expected, &stop, |ev| {
                    let m = match ev {
                        WatchEvent::Started(_) => (PathBuf::new(), "started".to_string()),
                        WatchEvent::Handled(p, Ok(Action::Moved { .. })) => {
                            (p.to_path_buf(), "moved".into())
                        }
                        WatchEvent::Handled(p, Ok(Action::Pending { rule, .. })) => {
                            (p.to_path_buf(), format!("pending {rule}"))
                        }
                        WatchEvent::Handled(p, Ok(Action::Skipped { reason, .. })) => {
                            (p.to_path_buf(), format!("skipped {reason}"))
                        }
                        WatchEvent::Handled(p, other) => (p.to_path_buf(), format!("{other:?}")),
                        _ => return,
                    };
                    let _ = tx.send(m);
                })
                .unwrap();
            })
        };
        let r = Run {
            base,
            desk,
            sorted,
            cfg,
            expected,
            stop,
            rx,
            worker: Some(worker),
            _tmp: tmp,
        };
        let (_, first) = r.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(first, "started");
        r
    }

    /// Events until one for `path` arrives (others are returned too).
    fn wait_for(&self, path: &Path, secs: u64) -> Vec<(PathBuf, String)> {
        let end = Instant::now() + Duration::from_secs(secs);
        let mut seen = Vec::new();
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            match self.rx.recv_timeout(left) {
                Ok(e) => {
                    let hit = e.0 == path;
                    seen.push(e);
                    if hit {
                        return seen;
                    }
                }
                Err(_) => break,
            }
        }
        panic!("no event for {} (saw {seen:?})", path.display());
    }

    fn ui(&self) -> Mover {
        Mover::new(
            Store::open(&self.cfg.store.db_path()).unwrap(),
            DuplicatePolicy::Skip,
        )
        .with_expected(self.expected.clone())
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

#[test]
fn a_folder_the_user_fills_is_proposed_whole_and_moves_whole() {
    let r = Run::start("");
    let folder = r.desk.join("전송자격인증제에 따른 발신번호 개수 제한");
    fs::create_dir(&folder).unwrap();
    std::thread::sleep(Duration::from_millis(500)); // inotify: watch the new folder first
    fs::write(folder.join("조치 방안.docx"), "발신번호 조치 방안").unwrap();
    fs::write(folder.join("공지_등록_제한.pdf"), "%PDF-1.4 공지").unwrap();
    fs::write(folder.join("신청서 모음.zip"), b"PK\x05\x06zip").unwrap();
    fs::create_dir(folder.join("참고")).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    fs::write(folder.join("참고").join("메모.txt"), "메모").unwrap();

    let seen = r.wait_for(&folder, 30);
    // Not one file was moved on its own.
    assert!(
        seen.iter().all(|(_, m)| m != "moved"),
        "scattered: {seen:?}"
    );
    assert_eq!(seen.last().unwrap().1, format!("pending {FOLDER_RULE}"));
    assert!(folder.join("조치 방안.docx").is_file());
    assert!(folder.join("참고").join("메모.txt").is_file());

    let ui = r.ui();
    let queue = ui.store().list_pending().unwrap();
    assert_eq!(queue.len(), 1, "{queue:?}");
    assert_eq!(queue[0].path, folder);
    // Mixed kinds, no customer: under 기타 (the user can pick another place).
    assert_eq!(
        queue[0].suggested_dest.as_deref(),
        Some(r.sorted.join("기타").as_path())
    );
    assert!(queue[0].reason.as_deref().unwrap().contains("4개"));

    let res = resolve(
        &ui,
        queue[0].id,
        &Decision::Move {
            dest_dir: None,
            new_name: None,
            on_duplicate: None,
        },
        "ui-folder",
        None,
    )
    .unwrap();
    let Resolution::Moved { dst, .. } = res else {
        panic!("{res:?}")
    };
    let moved = r
        .sorted
        .join("기타")
        .join("전송자격인증제에 따른 발신번호 개수 제한");
    assert_eq!(dst, moved);
    assert!(moved.join("조치 방안.docx").is_file());
    assert!(moved.join("참고").join("메모.txt").is_file());
    assert!(!folder.exists(), "the emptied folder is tidied away");
    assert!(ui.store().list_pending().unwrap().is_empty());

    // One undo brings the whole folder back.
    let rep = ui.undo_batch("ui-folder").unwrap();
    assert_eq!(rep.restored.len(), 4, "{rep:?}");
    assert!(folder.join("공지_등록_제한.pdf").is_file());
    assert!(folder.join("참고").join("메모.txt").is_file());
}

#[test]
fn a_folder_whose_files_share_one_rule_goes_to_that_rule_folder() {
    let r = Run::start("");
    let folder = r.desk.join("11월 보고서");
    fs::create_dir(&folder).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    fs::write(folder.join("a.pdf"), "%PDF a").unwrap();
    fs::write(folder.join("b.pdf"), "%PDF b").unwrap();
    let seen = r.wait_for(&folder, 30);
    assert!(seen.iter().all(|(_, m)| m != "moved"), "{seen:?}");
    let q = r.ui().store().list_pending().unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(
        q[0].suggested_dest.as_deref(),
        Some(r.sorted.join("문서").as_path())
    );
}

#[test]
fn a_new_text_document_is_left_while_empty_and_asked_once_written() {
    let r = Run::start("");
    // Explorer › 새로 만들기 › 텍스트 문서: an empty file.
    let blank = r.desk.join("새 텍스트 문서.txt");
    fs::write(&blank, b"").unwrap();
    let seen = r.wait_for(&blank, 20);
    assert!(
        seen.last().unwrap().1.starts_with("skipped empty file"),
        "{seen:?}"
    );
    assert!(blank.is_file());

    // Typed in and saved: asked, still not moved.
    fs::write(&blank, "회의 메모: 발신번호 정리").unwrap();
    let seen = r.wait_for(&blank, 20);
    assert_eq!(seen.last().unwrap().1, "pending 텍스트", "{seen:?}");
    assert!(blank.is_file());

    // Renamed right after creating ("메모.txt"), filled later: also asked.
    let named = r.desk.join("메모.txt");
    fs::write(&named, b"").unwrap();
    r.wait_for(&named, 20);
    fs::write(&named, "할 일").unwrap();
    let seen = r.wait_for(&named, 20);
    assert_eq!(seen.last().unwrap().1, "pending 텍스트", "{seen:?}");
    assert!(named.is_file());

    let q = r.ui().store().list_pending().unwrap();
    assert_eq!(q.len(), 2);
    assert!(q
        .iter()
        .all(|p| p.reason.as_deref() == Some("텍스트 · 새로 만든 파일")));
    assert!(q
        .iter()
        .all(|p| p.suggested_dest.as_deref() == Some(r.sorted.join("텍스트").as_path())));

    // A finished file dropped in (download, copy) still moves on its own.
    let done = r.desk.join("받은 자료.txt");
    fs::write(&done, "내용").unwrap();
    let seen = r.wait_for(&done, 20);
    assert_eq!(seen.last().unwrap().1, "moved", "{seen:?}");
    assert!(r.sorted.join("텍스트").join("받은 자료.txt").is_file());
    let _ = &r.base;
}
