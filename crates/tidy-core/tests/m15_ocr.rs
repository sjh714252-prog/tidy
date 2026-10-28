//! M15: images and scanned PDFs are marked for text recognition while
//! indexing, a background pass stores the recognized text as the body
//! (word search finds it), copies reuse the result by content hash, and
//! failures can be retried. The real engine test runs only where one is
//! installed (tesseract, or Windows' built-in OCR).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tidy_core::config::Config;
use tidy_core::index::ocr::{self, OcrEngine, OcrError};
use tidy_core::index::{IndexOptions, Indexer};
use tidy_core::search::{search, Query, SearchOptions};
use tidy_core::store::{OcrState, Store, TextStatus};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr")
}

/// Answers with fixed text and counts calls; fails on names with "깨진".
struct FakeEngine {
    calls: AtomicUsize,
}

impl OcrEngine for FakeEngine {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn describe(&self) -> String {
        "fake".into()
    }
    fn reads_pdf(&self) -> bool {
        true
    }
    fn image(&self, path: &Path) -> Result<String, OcrError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let name = path.file_name().unwrap().to_string_lossy();
        if name.contains("깨진") {
            return Err(OcrError::File("broken".into()));
        }
        if name.contains("풍경") {
            return Ok("  \n ".into());
        }
        Ok("가 람 정 보 견적서\nINVOICE 2026-10".into())
    }
    fn pdf(&self, path: &Path, _max_pages: u32) -> Result<String, OcrError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let _ = path;
        Ok("스캔한 계약서 가온누리원".into())
    }
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

fn state(store: &Store, root: &Path, name: &str) -> (Option<OcrState>, TextStatus) {
    let f = store.get_file(&root.join(name)).unwrap().unwrap();
    (f.ocr, f.text_status)
}

#[test]
fn mark_recognize_search_cache_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("바탕 화면");
    fs::create_dir(&root).unwrap();
    fs::copy(fixtures().join("견적서 캡처.png"), root.join("캡처 1.png")).unwrap();
    fs::copy(fixtures().join("스캔 견적서.pdf"), root.join("스캔.pdf")).unwrap();
    // A different image (different hash) with no text, and a broken one.
    let mut other = fs::read(fixtures().join("견적서 캡처.png")).unwrap();
    other.extend_from_slice(b"tail");
    fs::write(root.join("풍경.png"), &other).unwrap();
    other.extend_from_slice(b"more");
    fs::write(root.join("깨진 사진.png"), &other).unwrap();
    // Too small (an icon), and a normal text file.
    fs::write(root.join("아이콘.png"), b"\x89PNG tiny").unwrap();
    fs::write(root.join("메모.txt"), "그냥 메모").unwrap();

    let store = Store::open(&tmp.path().join("tidy.db")).unwrap();
    let opts = IndexOptions::default();
    Indexer::new(&store)
        .index_root(&root, &opts, |_, _| {})
        .unwrap();

    assert_eq!(
        state(&store, &root, "캡처 1.png").0,
        Some(OcrState::Pending)
    );
    assert_eq!(state(&store, &root, "스캔.pdf").0, Some(OcrState::Pending));
    assert_eq!(state(&store, &root, "아이콘.png").0, None);
    assert_eq!(state(&store, &root, "메모.txt").0, None);
    assert_eq!(store.ocr_counts().unwrap().pending, 4);
    assert!(names(&store, "견적서").is_empty());

    let cfg = Config::default();
    let engine = FakeEngine {
        calls: AtomicUsize::new(0),
    };
    let r = ocr::run_pass(&cfg, &store, &engine, 100, |_| {}).unwrap();
    assert_eq!((r.recognized, r.empty, r.failed, r.remaining), (2, 1, 1, 0));

    assert_eq!(
        state(&store, &root, "캡처 1.png"),
        (Some(OcrState::Ok), TextStatus::Ok)
    );
    assert_eq!(state(&store, &root, "풍경.png").0, Some(OcrState::Empty));
    assert_eq!(
        state(&store, &root, "깨진 사진.png").0,
        Some(OcrState::Failed)
    );
    // Spaced Hangul was joined, so the company name is found.
    assert_eq!(names(&store, "가람정보"), vec!["캡처 1.png"]);
    assert_eq!(names(&store, "가온누리원"), vec!["스캔.pdf"]);
    let hit = &search(&store, &Query::parse("견적서"), &SearchOptions::default()).unwrap()[0];
    assert_eq!(hit.file.ocr, Some(OcrState::Ok));

    // Unchanged files are not read again by the next index run.
    Indexer::new(&store)
        .index_root(&root, &opts, |_, _| {})
        .unwrap();
    assert_eq!(state(&store, &root, "캡처 1.png").0, Some(OcrState::Ok));

    // A copy (same content) gets the text from the cache at index time.
    fs::copy(root.join("캡처 1.png"), root.join("캡처 사본.png")).unwrap();
    Indexer::new(&store)
        .index_file(&root.join("캡처 사본.png"), &opts)
        .unwrap();
    assert_eq!(
        state(&store, &root, "캡처 사본.png"),
        (Some(OcrState::Ok), TextStatus::Ok)
    );
    let before = engine.calls.load(Ordering::SeqCst);
    ocr::run_pass(&cfg, &store, &engine, 100, |_| {}).unwrap();
    assert_eq!(engine.calls.load(Ordering::SeqCst), before);

    // Retry puts the failed one back.
    assert_eq!(store.ocr_retry_failed().unwrap(), 1);
    assert_eq!(store.ocr_counts().unwrap().pending, 1);
}

#[test]
fn ocr_text_is_checked_for_personal_information() {
    struct PiiEngine;
    impl OcrEngine for PiiEngine {
        fn name(&self) -> &'static str {
            "pii"
        }
        fn describe(&self) -> String {
            "pii".into()
        }
        fn reads_pdf(&self) -> bool {
            false
        }
        fn image(&self, _: &Path) -> Result<String, OcrError> {
            Ok("성명 홍길동 주민등록번호 900101-1234568".into())
        }
        fn pdf(&self, _: &Path, _: u32) -> Result<String, OcrError> {
            unreachable!("reads_pdf is false")
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap();
    fs::copy(fixtures().join("견적서 캡처.png"), root.join("신분증.png")).unwrap();
    fs::copy(fixtures().join("스캔 견적서.pdf"), root.join("스캔.pdf")).unwrap();
    let store = Store::open(&root.join("tidy.db")).unwrap();
    Indexer::new(&store)
        .index_root(&root, &IndexOptions::default(), |_, _| {})
        .unwrap();
    let mut cfg = Config::default();
    cfg.security.pii.enabled = true;
    let r = ocr::run_pass(&cfg, &store, &PiiEngine, 100, |_| {}).unwrap();
    assert_eq!(r.recognized, 1);
    // The PDF waits for an engine that reads PDFs.
    assert_eq!(state(&store, &root, "스캔.pdf").0, Some(OcrState::Pending));
    let f = store.get_file(&root.join("신분증.png")).unwrap().unwrap();
    assert!(f.pii_flags.unwrap_or(0) != 0, "{f:?}");
}

/// Real engine on the fixtures (skipped when none is installed).
#[test]
fn real_engine_reads_fixtures() {
    let cfg = Config::default();
    let engine = match ocr::build_engine(&cfg.ocr) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    eprintln!("engine: {} ({:?})", engine.describe(), engine.note());
    let korean = engine.note().is_none_or(|n| !n.contains("한국어"));
    let text = ocr::recognize(
        engine.as_ref(),
        &fixtures().join("견적서 캡처.png"),
        &cfg.ocr,
    )
    .unwrap();
    eprintln!("image: {text}");
    assert!(text.contains("INVOICE"), "{text}");
    if korean {
        assert!(text.contains("견적서"), "{text}");
    }
    if engine.reads_pdf() {
        let text = ocr::recognize(
            engine.as_ref(),
            &fixtures().join("스캔 견적서.pdf"),
            &cfg.ocr,
        )
        .unwrap();
        eprintln!("pdf: {text}");
        assert!(text.contains("INVOICE"), "{text}");
    }
}
