//! 0.20.0 (SPEC §14.30): archives grouped by company / topic instead of one
//! flat `압축 파일` folder — by name, by the folder inside, by the
//! documents inside. "정리 위치 새로 짜기" → apply → nothing more → undo;
//! then new archives while watching go to the folders it made.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tidy_core::classify::{Anchors, RuleSet};
use tidy_core::config::{DuplicatePolicy, OrganizeMode, WatchRoot};
use tidy_core::curate::{ContextDict, Curator};
use tidy_core::engine::{new_batch_id, Action, Engine};
use tidy_core::mover::Mover;
use tidy_core::report::rebuild::run;
use tidy_core::report::restructure;
use tidy_core::store::Store;
use tidy_core::watcher::IgnoreFilter;

/// A zip holding `entries` (name, contents; a name ending in `/` = folder).
fn zip_at(p: &Path, entries: &[(&str, &[u8])]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut z = zip::ZipWriter::new(fs::File::create(p).unwrap());
    let o: zip::write::SimpleFileOptions = Default::default();
    for (n, data) in entries {
        if n.ends_with('/') {
            z.add_directory(*n, o).unwrap();
        } else {
            z.start_file(*n, o).unwrap();
            z.write_all(data).unwrap();
        }
    }
    z.finish().unwrap();
}

/// A minimal DOCX whose body is `text`.
fn docx(text: &str) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o: zip::write::SimpleFileOptions = Default::default();
        z.start_file("word/document.xml", o).unwrap();
        write!(
            z,
            r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
        )
        .unwrap();
        z.finish().unwrap();
    }
    buf.into_inner()
}

fn plain_zip(p: &Path) {
    zip_at(p, &[("readme.txt", b"hello")]);
}

fn lines(plan: &restructure::Plan, org: &Path) -> Vec<String> {
    let mut got: Vec<String> = plan
        .moves
        .iter()
        .map(|m| {
            format!(
                "{} -> {}",
                m.from.strip_prefix(org).unwrap().display(),
                m.to.strip_prefix(org).unwrap().display()
            )
            .replace('\\', "/")
        })
        .collect();
    got.sort();
    got
}

#[test]
fn rebuild_groups_archives_then_watch_follows() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let org = base.join("정리");
    let flat = org.join("압축 파일");
    fs::create_dir_all(org.join("가람정보/20261014_과금")).unwrap();
    for n in [
        "현대홈쇼핑_13일.zip",
        "현대홈쇼핑_CAMP_13일.zip",
        "홈쇼핑 13일 처리건.zip",
        "특수한 유형의 부가통신사업자_관련자료.zip",
        "특수한유형의부가통신사업자 수수료(22~24년)_241221(1차).xlsx.zip",
        "특수한유형의부가통신사업자.zip",
        "제니엘시스템.zip",
        "케이티_정상고객사_리스트.xlsx.zip",
        "odyssey SBOM.zip",
        "전체 고객.zip",
    ] {
        plain_zip(&flat.join(n));
    }
    // The folder inside says whose it is.
    zip_at(
        &flat.join("참고.zip"),
        &[
            ("현대홈쇼핑_요청/", b""),
            ("현대홈쇼핑_요청/메모.txt", b"x"),
        ],
    );
    // The documents inside name a known customer.
    zip_at(
        &flat.join("견적 모음.zip"),
        &[
            ("a.csv", b"no,name\n1,hello\n"),
            ("b.docx", &docx("가람정보 견적서 2026")),
        ],
    );

    let mut cfg = tidy_core::Config::default();
    cfg.curate.context = base.join("context.toml");
    let mut d = ContextDict::starter();
    d.add_entity("가람정보", &[]).unwrap();
    d.save(&cfg.curate.context).unwrap();
    let _ = Curator::from_config(&cfg).unwrap();
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
    let store = mover.store();

    let r = run(&cfg, &org, store, Vec::new()).unwrap();
    let plan = r.plan.clone().expect("a plan");
    let got = lines(&plan, &org);
    let to = |n: &str| -> String {
        got.iter()
            .find(|l| l.starts_with(&format!("압축 파일/{n} -> ")))
            .unwrap_or_else(|| panic!("{n} not moved: {got:#?}"))
            .split(" -> ")
            .nth(1)
            .unwrap()
            .to_string()
    };
    // A company group: log bundles to its day folder, the rest by kind.
    for n in ["현대홈쇼핑_13일.zip", "현대홈쇼핑_CAMP_13일.zip"] {
        let t = to(n);
        assert!(
            t.starts_with("현대홈쇼핑/로그/") && t.contains("-13 로그/"),
            "{n} -> {t}"
        );
    }
    assert_eq!(
        to("홈쇼핑 13일 처리건.zip"),
        "현대홈쇼핑/압축파일/홈쇼핑 13일 처리건.zip"
    );
    assert_eq!(to("참고.zip"), "현대홈쇼핑/압축파일/참고.zip");
    // A topic group (three of them) → 기타.
    for n in [
        "특수한 유형의 부가통신사업자_관련자료.zip",
        "특수한유형의부가통신사업자 수수료(22~24년)_241221(1차).xlsx.zip",
        "특수한유형의부가통신사업자.zip",
    ] {
        assert_eq!(to(n), format!("기타/특수한유형의부가통신사업자/{n}"));
    }
    // Lone archives named for a company.
    assert_eq!(
        to("제니엘시스템.zip"),
        "제니엘시스템/압축파일/제니엘시스템.zip"
    );
    assert_eq!(
        to("케이티_정상고객사_리스트.xlsx.zip"),
        "케이티/압축파일/케이티_정상고객사_리스트.xlsx.zip"
    );
    // The documents inside named the customer.
    assert_eq!(to("견적 모음.zip"), "가람정보/압축파일/견적 모음.zip");
    // Nothing to say: left where it was.
    assert_eq!(got.len(), 10, "{got:#?}");
    assert_eq!(r.by_group, 9, "{}", plan.summary);
    assert!(
        plan.summary.contains("업체·주제별로 묶습니다"),
        "{}",
        plan.summary
    );
    for c in ["현대홈쇼핑", "제니엘시스템", "케이티"] {
        assert!(
            r.new_customers.contains(&c.to_string()),
            "{:?}",
            r.new_customers
        );
    }

    // Apply, look again: nothing more.
    let plan = restructure::save(store, plan).unwrap();
    let a = restructure::apply(&mover, &plan, None, &[], None).unwrap();
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    assert!(flat.join("odyssey SBOM.zip").is_file());
    let again = run(&cfg, &org, store, Vec::new()).unwrap();
    assert!(again.plan.is_none(), "{:?}", again.plan.map(|p| p.moves));

    // Watching: new archives follow the folders made.
    let desk = base.join("바탕 화면");
    fs::create_dir_all(&desk).unwrap();
    let rules = RuleSet::from_toml(
        "[[rule]]\nname = \"압축 파일\"\nmatch = { ext = [\"zip\"] }\ndest = \"압축 파일\"\n",
    )
    .unwrap();
    let watches = vec![WatchRoot {
        path: desk.clone(),
        mode: OrganizeMode::Collect,
        organize_root: Some(org.clone()),
        ..Default::default()
    }];
    let e = Engine::new(
        rules,
        Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename),
        IgnoreFilter::new(&[]).unwrap(),
    )
    .with_anchors(Anchors::from_watches(&watches))
    .with_sense(ContextDict::load(&cfg.curate.context).unwrap());
    let batch = new_batch_id();
    let moved = |p: PathBuf| -> PathBuf {
        match e.handle(&p, &batch).unwrap() {
            Action::Moved { dst, .. } => dst,
            a => panic!("{}: {a:?}", p.display()),
        }
    };
    let new = |n: &str| {
        let p = desk.join(n);
        plain_zip(&p);
        p
    };
    assert_eq!(
        moved(new("특수한유형의부가통신사업자_2차.zip")),
        org.join("기타/특수한유형의부가통신사업자/특수한유형의부가통신사업자_2차.zip")
    );
    assert_eq!(
        moved(new("제니엘시스템_계약.zip")),
        org.join("제니엘시스템/압축파일/제니엘시스템_계약.zip")
    );
    let log = moved(new("현대홈쇼핑_14일.zip"));
    assert!(
        log.starts_with(org.join("현대홈쇼핑/로그")) && log.to_string_lossy().contains("-14 로그"),
        "{}",
        log.display()
    );
    let inside = desk.join("자료 모음.zip");
    zip_at(
        &inside,
        &[("메모.txt", "가람정보 담당자 연락처".as_bytes())],
    );
    assert_eq!(moved(inside), org.join("가람정보/압축파일/자료 모음.zip"));
    assert_eq!(moved(new("odyssey.zip")), org.join("압축 파일/odyssey.zip"));
    e.mover().undo_batch(&batch).unwrap();
    assert!(desk.join("제니엘시스템_계약.zip").is_file());

    // The rebuild's batch undoes too.
    mover.undo_batch(&a.batch_id).unwrap();
    assert!(flat.join("현대홈쇼핑_13일.zip").is_file());
    assert!(flat.join("특수한유형의부가통신사업자.zip").is_file());
}
