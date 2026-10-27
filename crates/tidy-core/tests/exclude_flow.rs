//! Watch `exclude` sub-folders (SPEC §14.5): nothing inside them is
//! organized, scanned for a plan or counted by a preview; the config
//! round-trips and the filter compares paths component-wise.

use std::fs;
use std::path::Path;

use tidy_core::classify::preview::collect_files;
use tidy_core::classify::starter::starter_cards;
use tidy_core::classify::{apply_cards, Anchors, RuleSet};
use tidy_core::config::{write_settings, Config, DuplicatePolicy, SettingsPatch, WatchRoot};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::report::restructure::plan_from_rules;
use tidy_core::store::Store;
use tidy_core::watcher::{existing_files, IgnoreFilter};

fn rules() -> RuleSet {
    let text = apply_cards("", &starter_cards(&["documents".into()])).unwrap();
    RuleSet::from_toml(&text).unwrap()
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf, Config) {
    let tmp = tempfile::tempdir().unwrap();
    let desk = dunce::canonicalize(tmp.path()).unwrap().join("바탕 화면");
    let keep = desk.join("작업중");
    let keep2 = desk.join("보관").join("2025");
    fs::create_dir_all(&keep).unwrap();
    fs::create_dir_all(&keep2).unwrap();
    fs::write(desk.join("보고서.pdf"), "a").unwrap();
    fs::write(keep.join("초안.pdf"), "b").unwrap();
    fs::write(keep2.join("옛날.pdf"), "c").unwrap();
    let mut cfg = Config::default();
    cfg.watches.push(WatchRoot {
        path: desk.clone(),
        recursive: true,
        exclude: vec![keep, desk.join("보관")],
        ..Default::default()
    });
    (tmp, desk, cfg)
}

#[test]
fn excluded_folders_are_left_alone_everywhere() {
    let (_tmp, desk, cfg) = setup();
    let filter = cfg.ignore_filter().unwrap();

    // scan for --process-existing / plans
    let found = existing_files(&cfg.watches[0], &filter);
    assert_eq!(found, vec![desk.join("보고서.pdf")], "{found:?}");

    // preview sample used by the rule editor's counts
    let sample = collect_files(&cfg.watches, &filter);
    assert_eq!(sample.files.len(), 1);

    // rules restructure plan
    let plan = plan_from_rules(&desk, &rules(), true, &filter, &[]).unwrap();
    assert_eq!(plan.moves.len(), 1);
    assert!(plan.moves[0].from.ends_with("보고서.pdf"));

    // the live engine (watcher events for files inside the folder)
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let engine = Engine::new(rules(), mover, filter).with_anchors(Anchors::from_config(&cfg));
    let batch = new_batch_id();
    let a = engine
        .handle(&desk.join("작업중").join("초안.pdf"), &batch)
        .unwrap();
    assert!(matches!(a, Action::Skipped { .. }), "{a:?}");
    assert!(desk.join("작업중").join("초안.pdf").exists());
    let b = engine
        .handle(&desk.join("보관").join("2025").join("옛날.pdf"), &batch)
        .unwrap();
    assert!(matches!(b, Action::Skipped { .. }), "{b:?}");
    let c = engine.handle(&desk.join("보고서.pdf"), &batch).unwrap();
    assert!(matches!(c, Action::Moved { .. }), "{c:?}");
}

#[test]
fn filter_matches_whole_components_only() {
    let f = IgnoreFilter::new(&[])
        .unwrap()
        .with_dirs(vec![Path::new("/a/작업중").to_path_buf()]);
    assert!(f.in_excluded_dir(Path::new("/a/작업중")));
    assert!(f.in_excluded_dir(Path::new("/a/작업중/x/y.txt")));
    assert!(
        !f.in_excluded_dir(Path::new("/a/작업중2/y.txt")),
        "prefix of a name is not the folder"
    );
    assert!(!f.in_excluded_dir(Path::new("/a/y.txt")));
}

#[test]
fn exclude_round_trips_through_config_toml() {
    let (_tmp, desk, cfg) = setup();
    let path = desk.parent().unwrap().join("config.toml");
    let patch = SettingsPatch {
        watches: cfg.watches.clone(),
        store_path: None,
        quiet_mode: false,
        hotkey: "Ctrl+Space".into(),
        start_on_login: false,
        onboarded: None,
        check_updates: true,
        duplicate_policy: None,
    };
    write_settings(&path, &patch).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("exclude = ["), "{text}");
    let back = Config::from_toml(&text).unwrap();
    assert_eq!(back.watches[0].exclude, cfg.watches[0].exclude);
    // relative entries are resolved against the watch folder
    let rel = format!(
        "[[watch]]\npath = '{}'\nexclude = ['작업중', '보관']\n",
        desk.display()
    );
    let c2 = Config::from_toml(&rel).unwrap();
    assert_eq!(
        c2.watches[0].exclude,
        vec![desk.join("작업중"), desk.join("보관")]
    );
    assert!(c2.warnings().is_empty(), "{:?}", c2.warnings());
    let outside = format!(
        "[[watch]]\npath = '{}'\nexclude = ['/elsewhere']\n",
        desk.display()
    );
    let c3 = Config::from_toml(&outside).unwrap();
    assert!(
        c3.warnings().iter().any(|w| w.contains("제외 폴더")),
        "{:?}",
        c3.warnings()
    );
}
