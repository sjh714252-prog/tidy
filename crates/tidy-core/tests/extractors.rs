//! Extractor fixtures (CLAUDE.md): every file in `tests/fixtures/` that has
//! an `expected/<name>.txt` is extracted, and each line of the expected
//! file must appear in the extracted text.

use std::fs;
use std::path::{Path, PathBuf};

use tidy_core::index::Extractors;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn every_fixture_matches_its_expected_lines() {
    let dir = fixtures();
    let expected_dir = dir.join("expected");
    let x = Extractors::default_set();
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let expected = expected_dir.join(format!("{name}.txt"));
        assert!(
            expected.exists(),
            "fixture {name} has no expected/{name}.txt (add one when adding an extractor sample)"
        );
        let text = x
            .extract(&path)
            .unwrap_or_else(|| panic!("no extractor for {name}"))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        for line in fs::read_to_string(&expected)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            assert!(
                text.contains(line),
                "{name}: expected line {line:?} not found in extracted text:\n{text}"
            );
        }
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} fixtures checked");
}

#[test]
fn binary_and_missing_files_fail_softly() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("가짜.txt");
    fs::write(&bin, [0u8, 1, 2, 3, 0, 0]).unwrap();
    let x = Extractors::default_set();
    assert!(x.extract(&bin).unwrap().is_err());
    assert!(x.extract(&dir.path().join("없음.docx")).unwrap().is_err());
    // A zip that is not an OOXML package.
    let fake = dir.path().join("x.docx");
    fs::write(&fake, b"PK\x03\x04 not really").unwrap();
    assert!(x.extract(&fake).unwrap().is_err());
    // Unsupported type: None, never an error.
    assert!(x.extract(&dir.path().join("a.exe")).is_none());
}

/// (M14) Regenerate the hwp/hwpx fixtures (minimal structures built in
/// code, SPEC §14.9 3): `cargo test -p tidy-core --test extractors
/// make_hwp_fixtures -- --ignored`.
#[test]
#[ignore]
fn make_hwp_fixtures() {
    use tidy_core::index::hwp_testgen::{hwp_bytes, hwpx_bytes, COMPRESSED};
    let dir = fixtures();
    fs::write(
        dir.join("한글 문서.hwp"),
        hwp_bytes(
            &[
                "2026년 정기 점검 보고서",
                "Tidy HWP sample 문서 번호 A-1024",
                "점검 결과 표:\u{b}",
                "항목",
                "서버 상태 정상",
                "응답 시간 35ms",
            ],
            COMPRESSED,
            Some("2026년 정기 점검 보고서"),
        ),
    )
    .unwrap();
    fs::write(
        dir.join("한글 문서.hwpx"),
        hwpx_bytes(
            &["2026년 하반기 사업 계획", "HWPX sample 예산 4,500만 원"],
            &["담당 부서", "정보보호팀", "기한 12월 31일"],
        ),
    )
    .unwrap();
}
