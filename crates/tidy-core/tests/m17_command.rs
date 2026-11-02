//! M17 end to end (SPEC §14.14): a natural-language command → files from
//! the index → saved plan (nothing moved) → apply as one batch → undo.
//! Offline parser, the AI parser (MockProvider) and the fallback when the
//! AI fails. Korean names with spaces.

use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{Local, NaiveDate, TimeZone};
use serde_json::json;
use tidy_core::ai::MockProvider;
use tidy_core::classify::{AiClassifier, RuleSet};
use tidy_core::command::{self, CommandAction, CommandError, CommandOptions, ParsedBy};
use tidy_core::config::{Config, DuplicatePolicy, OrganizeMode, WatchRoot};
use tidy_core::index::{IndexOptions, Indexer};
use tidy_core::mover::Mover;
use tidy_core::report::restructure::{self, PlanSource};
use tidy_core::store::Store;

fn at(path: &Path, y: i32, m: u32, d: u32) {
    let t = Local
        .with_ymd_and_hms(y, m, d, 12, 0, 0)
        .unwrap()
        .timestamp();
    let f = File::options().write(true).open(path).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(t as u64))
        .unwrap();
}

struct Env {
    _tmp: tempfile::TempDir,
    desk: std::path::PathBuf,
    sorted: std::path::PathBuf,
    cfg: Config,
    store: Store,
    db: std::path::PathBuf,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let sorted = base.join("정리");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(sorted.join("업무").join("프로젝트")).unwrap();
    fs::create_dir_all(sorted.join("고객사")).unwrap();
    for (name, body, date) in [
        ("KT 과금내역 9월.xlsx", "KT 과금", (2026, 9, 12)),
        ("KT 요금표.xlsx", "KT 요금", (2026, 9, 20)),
        ("KT 계약서.pdf", "KT 계약", (2026, 9, 15)),
        ("SKT 정산.xlsx", "SKT", (2026, 9, 3)),
        ("KT 과금내역 8월.xlsx", "KT 과금", (2026, 8, 10)),
        ("회의 메모.txt", "KT 회의", (2026, 9, 9)),
    ] {
        let p = desk.join(name);
        fs::write(&p, body).unwrap();
        at(&p, date.0, date.1, date.2);
    }
    let cfg = Config {
        watches: vec![WatchRoot {
            path: desk.clone(),
            alias: Some("바탕화면".into()),
            mode: OrganizeMode::Collect,
            organize_root: Some(sorted.clone()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let store = Store::open(&base.join("tidy.db")).unwrap();
    Indexer::new(&store)
        .index_root(&desk, &IndexOptions::default(), |_, _| {})
        .unwrap();
    Env {
        _tmp: tmp,
        desk,
        sorted,
        cfg,
        store,
        db: base.join("tidy.db"),
    }
}

fn opts() -> CommandOptions {
    CommandOptions {
        today: NaiveDate::from_ymd_opt(2026, 10, 29).unwrap(),
        ..CommandOptions::new(Vec::new())
    }
}

fn names(r: &command::CommandResult) -> Vec<String> {
    let mut v: Vec<String> = r
        .files
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn spec_example_plan_apply_undo() {
    let e = setup();
    let r = command::run(
        "지난달 받은 KT 관련 엑셀 전부 프로젝트 폴더로",
        &e.cfg,
        &e.store,
        None,
        &opts(),
    )
    .unwrap();
    assert_eq!(r.parsed_by, ParsedBy::Rules);
    assert_eq!(r.intent.action, CommandAction::Move);
    // "SKT" matches the word prefix "KT*"? No: unicode61 tokens start
    // with "SKT", so only the two KT spreadsheets of September.
    assert_eq!(names(&r), vec!["KT 과금내역 9월.xlsx", "KT 요금표.xlsx"]);
    let dest = r.destination.as_ref().unwrap();
    assert_eq!(dest.path, e.sorted.join("업무").join("프로젝트"));
    assert!(!dest.new_folder);
    assert!(r
        .understood
        .iter()
        .any(|u| u == "기간 2026-09-01 ~ 2026-09-30"));
    assert!(r.understood.iter().any(|u| u == "종류 엑셀"));

    // Saved, nothing moved yet (rule 6).
    let plan = r.plan.unwrap();
    assert!(plan.id > 0);
    assert_eq!(plan.source, PlanSource::Command);
    assert!(e.desk.join("KT 요금표.xlsx").is_file());

    let plan = restructure::load(&e.store, plan.id).unwrap();
    let mover = Mover::new(Store::open(&e.db).unwrap(), DuplicatePolicy::Skip);
    let rep = restructure::apply(&mover, &plan, None, &[], Some(&IndexOptions::default())).unwrap();
    assert_eq!(rep.moved.len(), 2, "{:?}", rep);
    assert!(dest.path.join("KT 요금표.xlsx").is_file());
    assert!(!e.desk.join("KT 요금표.xlsx").exists());
    assert!(e.desk.join("KT 과금내역 8월.xlsx").is_file());

    // Running it again finds nothing new to move (already there).
    let again = command::run(
        "지난달 받은 KT 관련 엑셀 전부 프로젝트 폴더로",
        &e.cfg,
        &e.store,
        None,
        &opts(),
    )
    .unwrap();
    assert!(again.files.is_empty());
    assert!(again.plan.is_none());

    // One batch, undone at once.
    let u = mover.undo_batch(&rep.batch_id).unwrap();
    assert_eq!(u.restored.len(), 2, "{u:?}");
    assert!(e.desk.join("KT 요금표.xlsx").is_file());
}

#[test]
fn find_new_folder_and_refusals() {
    let e = setup();
    let r = command::run("KT 계약서 찾아줘", &e.cfg, &e.store, None, &opts()).unwrap();
    assert_eq!(r.intent.action, CommandAction::Find);
    assert_eq!(names(&r), vec!["KT 계약서.pdf"]);
    assert!(r.plan.is_none());

    // Unknown destination → a new folder under the organize root.
    let r = command::run(
        "바탕화면에서 PDF를 보관함으로 옮겨",
        &e.cfg,
        &e.store,
        None,
        &opts(),
    );
    let r = r.unwrap();
    let d = r.destination.unwrap();
    assert_eq!(d.path, e.sorted.join("보관함"), "{:?}", r.intent);
    assert!(d.new_folder);
    assert!(r.warnings.iter().any(|w| w.contains("새 폴더")));
    assert!(r.understood.iter().any(|u| u.starts_with("있는 곳")));

    assert!(matches!(
        command::run("옮겨줘", &e.cfg, &e.store, None, &opts()),
        Err(CommandError::NoTarget)
    ));
    assert!(matches!(
        command::run("KT 엑셀 옮겨줘", &e.cfg, &e.store, None, &opts()),
        Err(CommandError::NoDestination)
    ));
    assert!(matches!(
        command::run("KT 엑셀 다 지워줘", &e.cfg, &e.store, None, &opts()),
        Err(CommandError::Unsupported(_))
    ));
}

#[test]
fn ai_interprets_and_falls_back() {
    let e = setup();
    let mut cfg = e.cfg.clone();
    cfg.ai.enabled = true;
    let rules = RuleSet::default();
    let mock = Arc::new(MockProvider::new(json!({
        "action": "move", "keywords": ["KT"], "exts": ["pdf"],
        "after": "2026-09-01", "before": "2026-10-01", "source": "바탕화면",
        "destination": "고객사", "min_size_mb": null, "max_size_mb": null,
        "explanation": "9월 KT PDF를 고객사로"
    })));
    let ai = AiClassifier::with_provider(mock.clone(), &cfg, &rules).unwrap();
    let r = command::run(
        "9월 KT 계약 pdf 고객사 폴더로",
        &cfg,
        &e.store,
        Some(&ai),
        &opts(),
    )
    .unwrap();
    assert_eq!(r.parsed_by, ParsedBy::Ai);
    assert_eq!(names(&r), vec!["KT 계약서.pdf"]);
    assert_eq!(r.destination.unwrap().path, e.sorted.join("고객사"));
    // Only the sentence, the date and folder names go to the AI.
    let (_, user) = mock.prompts().pop().unwrap();
    assert!(user.contains("명령: 9월 KT 계약 pdf 고객사 폴더로"));
    assert!(user.contains("오늘: 2026-10-29"));
    assert!(user.contains("업무"), "{user}");
    assert!(!user.contains("KT 과금"), "no file names: {user}");

    // AI down → offline parser, with a note.
    let down = Arc::new(MockProvider::sequence(vec![Err("offline".into())]));
    let ai = AiClassifier::with_provider(down, &cfg, &rules).unwrap();
    let r = command::run("KT 계약서 찾아줘", &cfg, &e.store, Some(&ai), &opts()).unwrap();
    assert_eq!(r.parsed_by, ParsedBy::Rules);
    assert!(r.note.as_ref().unwrap().contains("AI 없이"));
    assert_eq!(names(&r), vec!["KT 계약서.pdf"]);

    // The AI says it is not a move/find.
    let no = Arc::new(MockProvider::new(json!({
        "action": "unsupported", "keywords": [], "exts": [], "after": null, "before": null,
        "source": null, "destination": null, "min_size_mb": null, "max_size_mb": null,
        "explanation": "압축 요청"
    })));
    let ai = AiClassifier::with_provider(no, &cfg, &rules).unwrap();
    assert!(matches!(
        command::run("KT 파일 압축해줘", &cfg, &e.store, Some(&ai), &opts()),
        Err(CommandError::Unsupported(_))
    ));
}
