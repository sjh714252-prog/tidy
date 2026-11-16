//! 0.18.0 (SPEC §14.24): files placed by what their names / contents say,
//! offline, while watching — then undone.
//! - customer log bundles → `{고객사}/로그/{날짜} 로그/`
//! - a log without a known customer → 확인 대기 ("어느 고객사?"), then
//!   the user's answer moves it into that customer's 로그 folder
//! - manuals → `서비스/{서비스}/매뉴얼/`, runtimes → `설치파일/{이름}/{OS}/`

use std::fs;
use std::path::Path;

use tidy_core::classify::{Anchors, RuleSet};
use tidy_core::config::{DuplicatePolicy, OrganizeMode, WatchRoot};
use tidy_core::curate::sense::{Shape, ASK_CUSTOMER_RULE};
use tidy_core::curate::ContextDict;
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::store::Store;
use tidy_core::watcher::IgnoreFilter;

fn engine(desk: &Path, org: &Path) -> Engine {
    let rules = RuleSet::from_toml(
        "[[rule]]\nname = \"압축 파일\"\nmatch = { ext = [\"zip\"] }\ndest = \"압축파일\"\n",
    )
    .unwrap();
    let watches = vec![WatchRoot {
        path: desk.to_path_buf(),
        mode: OrganizeMode::Collect,
        organize_root: Some(org.to_path_buf()),
        ..Default::default()
    }];
    let mut dict = ContextDict::default();
    dict.add_entity("현대홈쇼핑", &["HMALL".into()]).unwrap();
    dict.add_entity("KT", &[]).unwrap();
    Engine::new(
        rules,
        Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename),
        IgnoreFilter::new(&[]).unwrap(),
    )
    .with_anchors(Anchors::from_watches(&watches))
    .with_sense(dict)
}

fn put(dir: &Path, name: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    fs::write(&p, b"PK-ish").unwrap();
    p
}

#[test]
fn names_say_where_files_go_and_undo_restores() {
    let tmp = tempfile::tempdir().unwrap();
    let desk = tmp.path().join("바탕 화면");
    let org = tmp.path().join("정리");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(org.join("현대홈쇼핑").join("20261020_장애")).unwrap();
    fs::create_dir_all(org.join("가람정보").join("20261014_과금")).unwrap();
    let e = engine(&desk, &org);
    let batch = new_batch_id();

    // Customer day bundles: one dated folder for the day, tag or not.
    let mut log_dirs = Vec::new();
    for n in ["현대홈쇼핑_13일.zip", "현대홈쇼핑_CAMP_13일.zip"] {
        let a = e.handle(&put(&desk, n), &batch).unwrap();
        let Action::Moved { dst, rule, .. } = &a else {
            panic!("{n}: {a:?}")
        };
        assert!(rule.starts_with("현대홈쇼핑 · 로그 "), "{rule}");
        let dir = dst.parent().unwrap().to_path_buf();
        assert_eq!(dir.parent().unwrap(), org.join("현대홈쇼핑").join("로그"));
        let folder = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(folder.ends_with("-13 로그"), "{folder}");
        log_dirs.push(dir);
    }
    assert_eq!(log_dirs[0], log_dirs[1]);

    // A manual and a runtime.
    let a = e
        .handle(&put(&desk, "KTRCS_MANUAL_v2.0.zip"), &batch)
        .unwrap();
    assert!(
        matches!(&a, Action::Moved { dst, .. } if *dst == org.join("서비스/RCS/매뉴얼/KTRCS_MANUAL_v2.0.zip")),
        "{a:?}"
    );
    let a = e
        .handle(&put(&desk, "jre-8u121-linux-x64.zip"), &batch)
        .unwrap();
    assert!(
        matches!(&a, Action::Moved { dst, rule, .. } if *dst == org.join("설치파일/Java/Linux/jre-8u121-linux-x64.zip") && rule == "Java (Linux) 설치 파일"),
        "{a:?}"
    );

    // An ordinary zip still goes by the rules.
    let a = e.handle(&put(&desk, "사진 모음.zip"), &batch).unwrap();
    assert!(
        matches!(&a, Action::Moved { dst, .. } if dst.starts_with(org.join("압축파일"))),
        "{a:?}"
    );

    // A log nobody's name is on: asked, nothing moves.
    let unknown = put(&desk, "m2x_rcs_2026-06-28_1.log.zip");
    let a = e.handle(&unknown, &batch).unwrap();
    let Action::Pending {
        rule,
        suggested,
        reason,
        ..
    } = &a
    else {
        panic!("{a:?}")
    };
    assert_eq!(rule, ASK_CUSTOMER_RULE);
    assert_eq!(*suggested, org.join("로그").join("2026-06-28 로그"));
    assert!(reason.as_deref().unwrap().contains("어느 고객사"));
    assert!(unknown.exists());
    // Seen again (watch restart): still just waiting.
    assert!(matches!(
        e.handle(&unknown, &batch).unwrap(),
        Action::Skipped { .. } | Action::Pending { .. }
    ));
    let store = e.mover().store();
    let p = store.get_pending_by_path(&unknown).unwrap().unwrap();
    // The user picks 가람정보.
    let to = org.join("가람정보").join("로그").join("2026-06-28 로그");
    let r = resolve(
        e.mover(),
        p.id,
        &Decision::Move {
            dest_dir: Some(to.clone()),
            new_name: None,
            on_duplicate: None,
        },
        &new_batch_id(),
        None,
    )
    .unwrap();
    assert!(
        matches!(&r, Resolution::Moved { dst, .. } if *dst == to.join("m2x_rcs_2026-06-28_1.log.zip")),
        "{r:?}"
    );

    // Undo the watch batch: everything back on the desktop.
    let u = e.mover().undo_batch(&batch).unwrap();
    assert_eq!(u.restored.len(), 5, "{u:?}");
    for n in [
        "현대홈쇼핑_13일.zip",
        "KTRCS_MANUAL_v2.0.zip",
        "jre-8u121-linux-x64.zip",
    ] {
        assert!(desk.join(n).exists(), "{n}");
    }
    assert!(!org.join("설치파일").join("Java").exists());
}

#[test]
fn a_new_customer_folder_is_asked_for() {
    let tmp = tempfile::tempdir().unwrap();
    let desk = tmp.path().join("d");
    let org = tmp.path().join("o");
    fs::create_dir_all(&desk).unwrap();
    fs::create_dir_all(&org).unwrap();
    let e = engine(&desk, &org);
    let a = e
        .handle(&put(&desk, "HMALL_0613_agent.zip"), &new_batch_id())
        .unwrap();
    let Action::Pending {
        suggested, reason, ..
    } = &a
    else {
        panic!("{a:?}")
    };
    assert!(
        suggested.starts_with(org.join("현대홈쇼핑").join("로그")),
        "{suggested:?}"
    );
    assert!(
        suggested.to_string_lossy().ends_with("-06-13 Agent 로그"),
        "{suggested:?}"
    );
    assert!(reason.as_deref().unwrap().contains("새 고객사 폴더"));
    // Shape is public for the app's 확인 대기 text.
    assert!(
        Shape::log_folder(
            chrono::NaiveDate::from_ymd_opt(2026, 6, 13).unwrap(),
            "로그"
        ) == "2026-06-13 로그"
    );
}

/// "정리 위치 새로 짜기" reads names too: loose bundles in `압축파일/`
/// and in a customer's kind folder go to dated log folders, manuals and
/// runtimes to 서비스 / 설치파일, unknown logs are queued as questions.
#[test]
fn rebuild_reads_names_too() {
    use tidy_core::curate::Curator;
    use tidy_core::report::rebuild::run;
    use tidy_core::report::restructure;

    let tmp = tempfile::tempdir().unwrap();
    let org = dunce::canonicalize(tmp.path()).unwrap().join("정리");
    let w = |rel: &str| {
        let p = org.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
    };
    w("현대홈쇼핑/20261020_장애/a.log");
    w("현대홈쇼핑/압축파일/현대홈쇼핑_CAMP_0613.zip");
    w("압축파일/현대홈쇼핑_0613.zip");
    w("압축파일/m2x_rcs_2026-06-28_1.log.zip");
    w("압축파일/KTRCS_MANUAL_v2.0.zip");
    w("압축파일/jre-8u121-linux-x64.zip");
    w("압축파일/사진 모음.zip");

    let mut cfg = tidy_core::Config::default();
    cfg.curate.context = tmp.path().join("context.toml");
    let mut d = ContextDict::starter();
    d.add_entity("현대홈쇼핑", &[]).unwrap();
    d.save(&cfg.curate.context).unwrap();
    let _ = Curator::from_config(&cfg).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let store = mover.store();
    let r = run(&cfg, &org, store, Vec::new()).unwrap();
    let plan = r.plan.clone().expect("a plan");
    let mut got: Vec<String> = plan
        .moves
        .iter()
        .map(|m| {
            format!(
                "{} -> {}",
                m.from.strip_prefix(&org).unwrap().display(),
                m.to.strip_prefix(&org).unwrap().display()
            )
            .replace('\\', "/")
        })
        .collect();
    got.sort();
    let day = |n: &str| {
        let s = got.iter().find(|l| l.contains(n)).unwrap().clone();
        s
    };
    assert_eq!(got.len(), 4, "{got:#?}\n{}", plan.summary);
    let camp = day("CAMP");
    let plain = day("현대홈쇼핑_0613.zip");
    assert!(
        camp.contains("-> 현대홈쇼핑/로그/")
            && camp.ends_with("-06-13 로그/현대홈쇼핑_CAMP_0613.zip"),
        "{camp}"
    );
    assert!(
        plain.starts_with("압축파일/현대홈쇼핑_0613.zip -> 현대홈쇼핑/로그/"),
        "{plain}"
    );
    assert!(got.contains(
        &"압축파일/KTRCS_MANUAL_v2.0.zip -> 서비스/RCS/매뉴얼/KTRCS_MANUAL_v2.0.zip".to_string()
    ));
    assert!(got.contains(
        &"압축파일/jre-8u121-linux-x64.zip -> 설치파일/Java/Linux/jre-8u121-linux-x64.zip"
            .to_string()
    ));
    assert_eq!(r.by_name, 3, "{}", plan.summary);
    assert!(
        plan.summary.contains("고객사를 알 수 없는 로그 1개"),
        "{}",
        plan.summary
    );
    // The unknown log waits in 확인 대기 with the question.
    let q = store.list_pending().unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].reason.as_deref(), Some(ASK_CUSTOMER_RULE));
    assert_eq!(
        q[0].suggested_dest.as_deref(),
        Some(org.join("로그").join("2026-06-28 로그").as_path())
    );

    // Apply, look again: nothing more; undo restores.
    let plan = restructure::save(store, plan).unwrap();
    let a = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    let again = run(&cfg, &org, store, Vec::new()).unwrap();
    assert!(again.plan.is_none(), "{:?}", again.plan.map(|p| p.moves));
    mover.undo_batch(&a.batch_id).unwrap();
    assert!(org.join("압축파일/jre-8u121-linux-x64.zip").is_file());
    assert!(org
        .join("현대홈쇼핑/압축파일/현대홈쇼핑_CAMP_0613.zip")
        .is_file());
}
