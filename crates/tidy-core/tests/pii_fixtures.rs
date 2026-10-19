//! Korean PII fixtures (M9, SPEC §14.2): a positive and a negative sample
//! for each kind, read through the same extractors as the indexer.

use std::path::{Path, PathBuf};

use tidy_core::config::PiiCfg;
use tidy_core::pii::{Detector, PiiStatus};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pii")
        .join(name)
}

fn judge(name: &str) -> tidy_core::pii::PiiFinding {
    Detector::new(&PiiCfg::default()).unwrap().judge_path(
        &fixture(name),
        200 * 1024 * 1024,
        1_000_000,
    )
}

#[test]
fn resident_numbers() {
    let f = judge("주민번호_양성.txt");
    assert!(f.is_pii(), "{f:?}");
    assert_eq!(f.counts.rrn, 3);
    assert_eq!(f.summary(), "주민번호 3");
    let f = judge("주민번호_음성.txt");
    assert_eq!(f.status, PiiStatus::Clean, "{f:?}");
    assert_eq!(f.counts.rrn, 0);
}

#[test]
fn bank_accounts() {
    let f = judge("계좌_양성.txt");
    assert!(f.is_pii(), "{f:?}");
    assert_eq!(f.counts.account, 3);
    let f = judge("계좌_음성.txt");
    assert_eq!(f.status, PiiStatus::Clean, "{f:?}");
}

#[test]
fn names_with_mobile_numbers_in_spreadsheet_rows() {
    let f = judge("이름휴대전화_양성.xlsx");
    assert!(f.is_pii(), "{f:?}");
    assert_eq!(f.counts.name_phone, 15);
    assert_eq!(f.summary(), "이름+휴대전화 15");
    let f = judge("이름휴대전화_음성.txt");
    assert_eq!(f.status, PiiStatus::Clean, "{f:?}");
}

#[test]
fn names_with_emails_in_a_document() {
    let f = judge("이름이메일_양성.docx");
    assert!(f.is_pii(), "{f:?}");
    assert_eq!(f.counts.name_email, 2);
    let f = judge("이름이메일_음성.txt");
    assert_eq!(f.status, PiiStatus::Clean, "{f:?}");
}

#[test]
fn files_without_text_cannot_be_judged() {
    assert_eq!(judge("회의 사진.png").status, PiiStatus::Unknown);
}
