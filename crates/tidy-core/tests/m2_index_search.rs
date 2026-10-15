//! M2 end-to-end: index a folder (Korean names, mixed types) → search by
//! file name and body with snippets → incremental re-index → removal.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tidy_core::index::{FileOutcome, IndexOptions, Indexer};
use tidy_core::search::{search, MatchedIn, Query, SearchOptions};
use tidy_core::store::{Store, TextStatus};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn populate(root: &Path) {
    let docs = root.join("업무 문서");
    let inbox = root.join("받은 파일");
    fs::create_dir_all(&docs).unwrap();
    fs::create_dir_all(&inbox).unwrap();
    for f in [
        "제안서.docx",
        "과금 내역서.xlsx",
        "발표.pptx",
        "제안서.pdf",
        "안내.html",
    ] {
        fs::copy(fixtures().join(f), docs.join(f)).unwrap();
    }
    fs::write(
        inbox.join("회의록 2026-10.txt"),
        "10월 정기 회의록\n참석자: 홍길동\n결정: 과금내역서는 매월 5일까지 제출한다.\n",
    )
    .unwrap();
    fs::write(inbox.join("사진.png"), b"\x89PNG not really").unwrap();
    fs::write(inbox.join("깨진 문서.docx"), b"not a zip at all").unwrap();
    fs::write(inbox.join(".숨김.txt"), "hidden").unwrap();
    fs::write(inbox.join("받는중.crdownload"), "partial").unwrap();
}

fn hits(store: &Store, q: &str) -> Vec<tidy_core::search::SearchHit> {
    search(store, &Query::parse(q), &SearchOptions::default()).unwrap()
}

fn names(h: &[tidy_core::search::SearchHit]) -> Vec<String> {
    h.iter().map(|h| h.file.name.clone()).collect()
}

#[test]
fn index_then_search_names_and_bodies() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("바탕 화면");
    fs::create_dir(&root).unwrap();
    populate(&root);

    let store = Store::open(&tmp.path().join("tidy.db")).unwrap();
    let opts = IndexOptions::default();
    let report = Indexer::new(&store)
        .index_root(&root, &opts, |_, _| {})
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    // hidden + .crdownload skipped
    assert_eq!(report.scanned, 8);
    assert_eq!(report.added, 8);
    assert_eq!(report.text_ok, 6);
    assert_eq!(report.text_failed, 1, "broken docx keeps its metadata");
    assert_eq!(report.text_unsupported, 1, "png is metadata only");

    let broken = store
        .get_file(&root.join("받은 파일").join("깨진 문서.docx"))
        .unwrap()
        .unwrap();
    assert_eq!(broken.text_status, TextStatus::Failed);
    assert_eq!(broken.size, 16);

    // Body match (3+ chars → trigram) with a snippet.
    let h = hits(&store, "과금내역서");
    assert!(
        names(&h).contains(&"회의록 2026-10.txt".to_string()),
        "{:?}",
        names(&h)
    );
    let meeting = h
        .iter()
        .find(|h| h.file.name == "회의록 2026-10.txt")
        .unwrap();
    assert_eq!(meeting.matched_in, MatchedIn::Content);
    assert!(
        meeting.snippet.contains("[과금내역서]"),
        "{}",
        meeting.snippet
    );

    // Name match, ranked above body-only matches (name weight 3×).
    let h = hits(&store, "제안서");
    assert!(names(&h).contains(&"제안서.docx".to_string()));
    assert!(names(&h).contains(&"제안서.pdf".to_string()));
    assert!(h[0].name_highlight.contains("[제안서]"));

    // Short (2-char) term → unicode61 prefix path: "KT" appears in docx,
    // xlsx and pdf bodies.
    let h = hits(&store, "KT");
    let n = names(&h);
    assert!(n.contains(&"제안서.docx".to_string()), "{n:?}");
    assert!(n.contains(&"과금 내역서.xlsx".to_string()), "{n:?}");

    // Hybrid: long + short term, ANDed.
    let h = hits(&store, "제안 KT ext:docx");
    assert_eq!(names(&h), vec!["제안서.docx"]);

    // Filters only.
    let h = hits(&store, "type:image");
    assert_eq!(names(&h), vec!["사진.png"]);
    assert_eq!(h[0].matched_in, MatchedIn::Filter);
    let h = hits(
        &store,
        &format!("in:\"{}\"", root.join("업무 문서").display()),
    );
    assert_eq!(h.len(), 5);
    let h = hits(&store, "회의록 size:>1kb");
    assert!(h.is_empty());

    // Phrase and html body.
    let h = hits(&store, "\"지하 2층\"");
    assert_eq!(names(&h), vec!["안내.html"]);
    // pptx notes are indexed too.
    let h = hits(&store, "질의응답");
    assert_eq!(names(&h), vec!["발표.pptx"]);
    // Nothing matches.
    assert!(hits(&store, "존재하지않는단어").is_empty());

    let st = store.index_stats().unwrap();
    assert_eq!((st.roots, st.files, st.with_text), (1, 8, 6));
}

#[test]
fn incremental_reindex_size_limit_and_removal() {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap().join("폴더");
    fs::create_dir(&root).unwrap();
    let a = root.join("가.txt");
    let b = root.join("나.txt");
    let big = root.join("큰 파일.txt");
    fs::write(&a, "첫 번째 내용 알파").unwrap();
    fs::write(&b, "두 번째 내용 베타").unwrap();
    fs::write(&big, "x".repeat(2000)).unwrap();

    let store = Store::open(&tmp.path().join("tidy.db")).unwrap();
    let opts = IndexOptions {
        max_content_bytes: 1000,
        ..Default::default()
    };
    let indexer = Indexer::new(&store);

    let r = indexer.index_root(&root, &opts, |_, _| {}).unwrap();
    assert_eq!((r.added, r.updated, r.unchanged, r.removed), (3, 0, 0, 0));
    assert_eq!(r.text_skipped, 1);
    let bigrec = store.get_file(&big).unwrap().unwrap();
    assert_eq!(bigrec.text_status, TextStatus::Skipped);
    assert!(bigrec.hash.is_none(), "no hash above the size limit");
    assert!(hits(&store, "큰 파일").len() == 1, "name still searchable");

    // Nothing changed → everything unchanged, no re-extraction.
    let mut seen = Vec::new();
    let r = indexer
        .index_root(&root, &opts, |p, o| seen.push((p.to_path_buf(), *o)))
        .unwrap();
    assert_eq!((r.added, r.updated, r.unchanged), (0, 0, 3));
    assert!(seen.iter().all(|(_, o)| *o == FileOutcome::Unchanged));

    // Same size, different mtime → re-processed; content change visible.
    fs::write(&a, "첫 번째 내용 감마").unwrap();
    let f = fs::File::options().write(true).open(&a).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    drop(f);
    // b removed.
    fs::remove_file(&b).unwrap();
    let r = indexer.index_root(&root, &opts, |_, _| {}).unwrap();
    assert_eq!((r.added, r.updated, r.unchanged, r.removed), (0, 1, 1, 1));
    assert!(hits(&store, "감마").len() == 1);
    assert!(hits(&store, "알파").is_empty(), "old text replaced");
    assert!(
        hits(&store, "베타").is_empty(),
        "removed file gone from index"
    );
    assert!(store.get_file(&b).unwrap().is_none());

    // Single-file refresh (what the watcher will call after a move).
    fs::write(&b, "다시 생긴 파일 델타").unwrap();
    let o = indexer.index_file(&b, &opts).unwrap();
    assert!(matches!(o, FileOutcome::Added(TextStatus::Ok)));
    assert_eq!(names(&hits(&store, "델타")), vec!["나.txt"]);

    // Metadata-only root (NAS default): names searchable, bodies not.
    let meta_opts = IndexOptions {
        index_content: false,
        ..opts.clone()
    };
    let r = indexer.index_root(&root, &meta_opts, |_, _| {}).unwrap();
    assert_eq!(
        r.unchanged, 3,
        "same size/mtime → not re-read even in meta mode"
    );
    store.clear_index().unwrap();
    let r = indexer.index_root(&root, &meta_opts, |_, _| {}).unwrap();
    assert_eq!(r.added, 3);
    assert!(hits(&store, "감마").is_empty());
    assert_eq!(names(&hits(&store, "가.txt")), vec!["가.txt"]);
}

/// `tidy index <file>` / watch-moved files: a file outside every root gets
/// its folder registered as a root and becomes searchable on its own.
#[test]
fn index_single_file_outside_any_root() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("정리 된 곳").join("2026");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("단독 메모.txt");
    fs::write(&file, "보안점검 체크리스트 단독 파일").unwrap();
    fs::write(dir.join("옆 파일.txt"), "보안점검 아님 옆 파일").unwrap();

    let store = Store::open_in_memory().unwrap();
    let idx = Indexer::new(&store);
    let out = idx.index_file(&file, &IndexOptions::default()).unwrap();
    assert_eq!(out, FileOutcome::Added(TextStatus::Ok));
    assert_eq!(
        idx.index_file(&file, &IndexOptions::default()).unwrap(),
        FileOutcome::Unchanged
    );
    let hits = search(&store, &Query::parse("보안점검"), &SearchOptions::default()).unwrap();
    // Only the given file is indexed, not its neighbour.
    assert_eq!(hits.len(), 1);
    assert!(hits[0].file.path.ends_with("단독 메모.txt"));
    assert!(idx.index_file(&dir, &IndexOptions::default()).is_err());
}
