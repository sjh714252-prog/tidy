//! M9 (SPEC §14.2): personal files are detected, locked into AES-256 zips
//! through the mover (verified before the original goes), stay findable,
//! wait for approval in `confirm` mode, and come back with undo.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use tidy_core::classify::RuleSet;
use tidy_core::config::{Approval, DuplicatePolicy, SecurityMode};
use tidy_core::engine::{new_batch_id, reindex_undo, Action, Engine};
use tidy_core::index::{IndexOptions, Indexer};
use tidy_core::mover::Mover;
use tidy_core::pending::{resolve, Decision, Resolution};
use tidy_core::search::{search, Query, SearchOptions};
use tidy_core::security::{SecurityPolicy, VaultKeys};
use tidy_core::store::{MoveOp, Store};
use tidy_core::watcher::IgnoreFilter;
use tidy_core::Config;

const PW: &str = "보관 비밀번호 1234";

fn rrn(front: &str, gender: u32, back6: &str) -> String {
    let s = format!("{front}{gender}{back6}");
    let d: Vec<u32> = s.chars().map(|c| c.to_digit(10).unwrap()).collect();
    const W: [u32; 12] = [2, 3, 4, 5, 6, 7, 8, 9, 2, 3, 4, 5];
    let sum: u32 = d.iter().zip(W).map(|(a, w)| a * w).sum();
    format!("{front}-{gender}{back6}{}", (11 - sum % 11) % 10)
}

struct Env {
    _tmp: tempfile::TempDir,
    desk: PathBuf,
    vault: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let desk = base.join("바탕 화면");
    let vault = base.join("정리").join("보안");
    fs::create_dir_all(&desk).unwrap();
    Env {
        _tmp: tmp,
        desk,
        vault,
    }
}

fn policy(vault: Option<&Path>, approval: Approval) -> SecurityPolicy {
    let mut cfg = Config::default();
    cfg.security.mode = if vault.is_some() {
        SecurityMode::Vault
    } else {
        SecurityMode::Rules
    };
    cfg.security.vault_dir = vault.map(Path::to_path_buf);
    cfg.security.approval = approval;
    SecurityPolicy::from_config(&cfg)
}

fn engine(rules: &str, sec: SecurityPolicy, keys: VaultKeys) -> Engine {
    let mover =
        Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename).with_vault_keys(keys);
    let mut idx = IndexOptions::from_config(&Config::default());
    idx.pii = Some(sec.detector.clone());
    Engine::new(
        RuleSet::from_toml(rules).unwrap(),
        mover,
        IgnoreFilter::new(&[]).unwrap(),
    )
    .with_security(sec)
    .with_index(idx)
}

fn unzip(zip: &Path, pw: &str) -> (String, String) {
    let mut za = zip::ZipArchive::new(fs::File::open(zip).unwrap()).unwrap();
    let mut e = za.by_index_decrypt(0, pw.as_bytes()).unwrap();
    let mut s = String::new();
    e.read_to_string(&mut s).unwrap();
    (e.name().to_string(), s)
}

const DOC_RULE: &str = r#"
[[rule]]
name = "문서"
match = { ext = ["txt", "csv"] }
dest = "문서"
encrypt = "pii"
"#;

#[test]
fn vault_mode_auto_locks_finds_and_undoes() {
    let e = env();
    let eng = engine(
        "",
        policy(Some(&e.vault), Approval::Auto),
        VaultKeys::new(PW),
    );
    let src = e.desk.join("직원 명단.txt");
    let body = format!(
        "인사 기록\n홍길동 주민등록번호 {}\n김영희 010-2345-6789\n",
        rrn("850315", 1, "23456")
    );
    fs::write(&src, &body).unwrap();
    let plain = e.desk.join("회의록.txt");
    fs::write(&plain, "다음 회의는 화요일입니다").unwrap();

    let batch = new_batch_id();
    let a = eng.handle(&src, &batch).unwrap();
    let Action::Moved {
        dst,
        encrypted: true,
        ..
    } = &a
    else {
        panic!("{a:?}")
    };
    assert_eq!(dst, &e.vault.join("직원 명단.txt.zip"));
    assert!(!src.exists());
    assert_eq!(unzip(dst, PW), ("직원 명단.txt".into(), body.clone()));
    // A non-personal file is left alone (no rule).
    assert!(matches!(
        eng.handle(&plain, &batch).unwrap(),
        Action::NoRule(_)
    ));

    let store = eng.mover().store();
    let row = store.recent_moves(5).unwrap().remove(0);
    assert_eq!(row.op, MoveOp::Encrypt);
    // Index: same row, now the zip, text kept → still found, 🔒.
    let hits = search(store, &Query::parse("인사 기록"), &SearchOptions::default()).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(&hits[0].file.path, dst);
    assert!(hits[0].file.vault);
    assert!(hits[0].file.pii_flags.unwrap() != 0);
    // A later index run keeps it as is.
    Indexer::new(store)
        .index_root(&e.vault, &IndexOptions::default(), |_, _| {})
        .unwrap();
    let f = store.get_file(dst).unwrap().unwrap();
    assert!(f.vault && f.pii_flags.unwrap() != 0);
    assert_eq!(store.pii_totals(None).unwrap(), (1, 0));
    // The zip itself is "placed by Tidy".
    assert!(matches!(
        eng.handle(dst, &new_batch_id()).unwrap(),
        Action::Skipped { .. }
    ));

    let r = eng.mover().undo_last().unwrap().unwrap();
    assert_eq!(r.restored.len(), 1, "{:?}", r.conflicts);
    reindex_undo(store, &r, &IndexOptions::from_config(&Config::default()));
    assert_eq!(fs::read_to_string(&src).unwrap(), body);
    assert!(!dst.exists());
    let f = store.get_file(&src).unwrap().unwrap();
    assert!(!f.vault);
    assert_eq!(
        store.pii_totals(None).unwrap(),
        (1, 1),
        "personal, not encrypted"
    );
    // Restored by undo: not locked again.
    assert!(matches!(
        eng.handle(&src, &new_batch_id()).unwrap(),
        Action::Skipped { .. }
    ));
}

#[test]
fn confirm_mode_queues_with_badge_and_locks_on_approval() {
    let e = env();
    let eng = engine(
        DOC_RULE,
        policy(None, Approval::Confirm),
        VaultKeys::new(PW),
    );
    let src = e.desk.join("연락망.csv");
    let names = ["홍길동", "김영희", "이민준"];
    let body: String = (0..15)
        .map(|i| format!("{},010-2000-{:04}\n", names[i % 3], 1000 + i))
        .collect();
    fs::write(&src, &body).unwrap();
    let other = e.desk.join("메모.txt");
    fs::write(&other, "장보기: 우유, 빵").unwrap();

    let batch = new_batch_id();
    let a = eng.handle(&src, &batch).unwrap();
    let Action::Pending {
        encrypt: true,
        pii: Some(f),
        ..
    } = &a
    else {
        panic!("{a:?}")
    };
    assert_eq!(f.summary(), "이름+휴대전화 15");
    assert!(src.exists(), "nothing moves before approval");
    // The same rule moves a non-personal file normally.
    let b = eng.handle(&other, &batch).unwrap();
    assert!(
        matches!(&b, Action::Moved { encrypted: false, dst, .. } if dst == &e.desk.join("문서").join("메모.txt")),
        "{b:?}"
    );

    let store = eng.mover().store();
    let p = store.list_pending().unwrap().remove(0);
    assert!(p.encrypt);
    assert_eq!(p.pii.unwrap().summary(), "이름+휴대전화 15");
    let res = resolve(
        eng.mover(),
        p.id,
        &Decision::Move {
            dest_dir: None,
            new_name: None,
        },
        &new_batch_id(),
        Some(&IndexOptions::from_config(&Config::default())),
    )
    .unwrap();
    let Resolution::Moved { dst, .. } = res else {
        panic!("{res:?}")
    };
    assert_eq!(dst, e.desk.join("문서").join("연락망.csv.zip"));
    assert_eq!(unzip(&dst, PW).1, body);
    assert!(!src.exists());
    assert!(store.get_file(&dst).unwrap().unwrap().vault);
}

#[test]
fn without_a_password_even_auto_mode_asks_first() {
    let e = env();
    let eng = engine(DOC_RULE, policy(None, Approval::Auto), VaultKeys::none());
    let src = e.desk.join("계좌.txt");
    fs::write(&src, "환불 입금 계좌: 123456-12-345678 (예금주 홍길동)").unwrap();
    let a = eng.handle(&src, &new_batch_id()).unwrap();
    assert!(matches!(a, Action::Pending { encrypt: true, .. }), "{a:?}");
    assert!(src.exists());
    // Approving without a password fails and keeps the entry.
    let store = eng.mover().store();
    let p = store.list_pending().unwrap().remove(0);
    assert!(resolve(
        eng.mover(),
        p.id,
        &Decision::Move {
            dest_dir: None,
            new_name: None
        },
        &new_batch_id(),
        None
    )
    .is_err());
    assert!(src.exists());
    assert_eq!(store.pending_count().unwrap(), 1);
}

#[test]
fn always_rule_and_dry_run() {
    let e = env();
    let rules = r#"
[[rule]]
name = "계약서"
match = { name_contains = "계약" }
dest = "보관"
encrypt = "always"
"#;
    let sec = policy(None, Approval::Auto);
    let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename)
        .with_vault_keys(VaultKeys::new(PW));
    let dry = Engine::new(
        RuleSet::from_toml(rules).unwrap(),
        mover,
        IgnoreFilter::new(&[]).unwrap(),
    )
    .with_security(sec.clone())
    .dry_run(true);
    let src = e.desk.join("임대 계약서.pdf");
    fs::write(&src, "binary-ish").unwrap();
    let a = dry.handle(&src, "b").unwrap();
    assert!(
        matches!(
            a,
            Action::WouldMove {
                encrypt: true,
                confirm: false,
                ..
            }
        ),
        "{a:?}"
    );
    assert!(src.exists());

    let eng = engine(rules, sec, VaultKeys::new(PW));
    let a = eng.handle(&src, "b").unwrap();
    assert!(
        matches!(
            a,
            Action::Moved {
                encrypted: true,
                ..
            }
        ),
        "{a:?}"
    );
    assert!(e.desk.join("보관").join("임대 계약서.pdf.zip").is_file());
}
