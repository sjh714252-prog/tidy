//! M16: the NAS agent (`tidy serve`) driven by the PC client, end to end on
//! a temp folder as the NAS: settings from the PC → new file moved by rule
//! → confirm queue answered → existing files organized → undo, plus the
//! password and the "only inside the data root" fences (SPEC §14.13).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tidy_core::config::OrganizeMode;
use tidy_core::remote::api::{AgentSettings, AgentWatch, RemoteDecision};
use tidy_core::remote::server::{start, AgentOptions};
use tidy_core::remote::{NasClient, RemoteError};
use tidy_core::watcher::WatchOptions;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(30);
    while Instant::now() < until {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

#[test]
fn agent_organizes_on_the_nas_and_answers_the_pc() {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let data = base.join("data");
    let work = data.join("업무");
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(data.join("사진")).unwrap();
    fs::write(work.join("기존 보고서.pdf"), b"%PDF-1.4 old").unwrap();
    let config = base.join("config").join("config.toml");

    let agent = start(AgentOptions {
        config_path: config.clone(),
        data_root: data.clone(),
        listen: "127.0.0.1:0".into(),
        token: "correct-horse".into(),
        watch_options: Some(WatchOptions {
            stabilize: Duration::from_millis(200),
            ignore_globs: Vec::new(),
            tick: Duration::from_millis(50),
            reconnect: Duration::from_millis(500),
            poll_override: None,
        }),
        reindex_every: Duration::from_secs(3600),
    })
    .unwrap();
    assert!(config.exists(), "a starter config is written");
    let url = format!("http://{}", agent.addr());
    let rt = rt();
    let nas = NasClient::new(&url, "correct-horse").unwrap();

    // Wrong password: refused, and said so plainly.
    let wrong = NasClient::new(&url, "wrong").unwrap();
    assert!(matches!(
        rt.block_on(wrong.status()),
        Err(RemoteError::Unauthorized)
    ));

    // Fresh agent: no folders, nothing watched.
    let st = rt.block_on(nas.status()).unwrap();
    assert!(st.watches.is_empty());
    assert!(!st.watching);
    assert_eq!(st.data_root, s(&data));

    // Folder picker: the data root's sub-folders.
    let dirs = rt.block_on(nas.dirs(None)).unwrap();
    let names: Vec<_> = dirs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["사진", "업무"]);
    assert!(
        rt.block_on(nas.dirs(Some(&s(&base)))).is_err(),
        "outside the data root"
    );

    // Settings from the PC. A folder outside the data root is refused.
    let rules = "[[rule]]\nname = \"PDF\"\nmatch = { ext = [\"pdf\"] }\ndest = \"문서\"\n\n\
                 [[rule]]\nname = \"메모\"\nmatch = { ext = [\"txt\"] }\ndest = \"메모\"\nconfirm = true\n";
    let mut settings = AgentSettings {
        data_root: String::new(),
        watches: vec![AgentWatch {
            path: s(&base),
            alias: None,
            mode: OrganizeMode::InPlace,
            organize_root: None,
            recursive: false,
            exclude: Vec::new(),
        }],
        index_only: vec![s(&data.join("사진"))],
        rules: rules.into(),
    };
    assert!(matches!(
        rt.block_on(nas.save_settings(&settings)),
        Err(RemoteError::Agent(_))
    ));
    settings.watches[0].path = s(&work);
    settings.rules = "[[rule]\nbroken".into();
    assert!(
        rt.block_on(nas.save_settings(&settings)).is_err(),
        "bad rules"
    );
    settings.rules = rules.into();
    let saved = rt.block_on(nas.save_settings(&settings)).unwrap();
    assert_eq!(saved.watches.len(), 1);
    assert_eq!(saved.index_only, [s(&data.join("사진"))]);
    assert!(saved.rules.contains("메모"));
    wait_for("watching", || rt.block_on(nas.status()).unwrap().watching);

    // Existing files are left alone by watching…
    std::thread::sleep(Duration::from_millis(600));
    assert!(work.join("기존 보고서.pdf").exists());

    // …a new one is moved by its rule, on the NAS.
    fs::write(work.join("새 견적.pdf"), b"%PDF-1.4 new").unwrap();
    wait_for("rule move", || {
        work.join("문서").join("새 견적.pdf").exists()
    });

    // A confirm rule queues the file; the PC answers.
    fs::write(work.join("회의 메모.txt"), "다음 주 견적서 합계 확인").unwrap();
    let mut pending = Vec::new();
    wait_for("pending", || {
        pending = rt.block_on(nas.pending()).unwrap();
        !pending.is_empty()
    });
    assert_eq!(pending[0].name, "회의 메모.txt");
    assert_eq!(pending[0].rule.as_deref(), Some("메모"));
    // A destination outside the data root is refused.
    let out = RemoteDecision::Move {
        dest_dir: Some(s(&base)),
        new_name: None,
        on_duplicate: None,
    };
    assert!(rt.block_on(nas.resolve(vec![pending[0].id], out)).is_err());
    let res = rt
        .block_on(nas.resolve(
            vec![pending[0].id],
            RemoteDecision::Move {
                dest_dir: None,
                new_name: None,
                on_duplicate: None,
            },
        ))
        .unwrap();
    assert_eq!(res[0].kind, "moved", "{res:?}");
    assert!(work.join("메모").join("회의 메모.txt").exists());
    assert!(rt.block_on(nas.pending()).unwrap().is_empty());

    // Search on the NAS index (name and body).
    wait_for("search", || {
        let r = rt.block_on(nas.search("견적서", 20)).unwrap();
        r.hits.iter().any(|h| h.name == "회의 메모.txt")
    });
    let r = rt.block_on(nas.search("기존 보고서", 20)).unwrap();
    assert!(r
        .hits
        .iter()
        .any(|h| h.path == s(&work.join("기존 보고서.pdf"))));

    // "규칙대로 지금 정리": preview, then one batch.
    let p = rt.block_on(nas.organize_preview(&s(&work))).unwrap();
    assert_eq!(p.to_move, 1, "{p:?}");
    assert!(
        work.join("기존 보고서.pdf").exists(),
        "preview moves nothing"
    );
    let o = rt.block_on(nas.organize(&s(&work))).unwrap();
    assert_eq!(o.moved, 1, "{o:?}");
    let moved_to: PathBuf = work.join("문서").join("기존 보고서.pdf");
    assert!(moved_to.exists());
    assert!(
        rt.block_on(nas.organize(&s(&data.join("사진")))).is_err(),
        "not a watch folder"
    );

    // History + undo of that batch.
    let batches = rt.block_on(nas.batches(50)).unwrap();
    assert!(batches.len() >= 3, "{batches:?}");
    let moves = rt.block_on(nas.batch(&o.batch_id)).unwrap();
    assert_eq!(moves.len(), 1);
    let today = rt.block_on(nas.today(0)).unwrap();
    assert!(today.moved >= 3, "{today:?}");
    let u = rt.block_on(nas.undo(&o.batch_id)).unwrap();
    assert_eq!(u.restored.len(), 1);
    assert!(work.join("기존 보고서.pdf").exists());
    assert!(!moved_to.exists());

    let st = rt.block_on(nas.status()).unwrap();
    assert_eq!(st.watches.len(), 1);
    assert!(st.watches[0].online);
    assert_eq!(st.pending, 0);
    assert!(st.files >= 3, "{st:?}");

    agent.stop();
}
