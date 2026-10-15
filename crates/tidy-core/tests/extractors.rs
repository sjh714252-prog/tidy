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
