//! (0.11.2) Plan files: a cleanup prepared elsewhere (for example from a
//! folder listing the user sent) and opened in the app — 정리하기 › "제안
//! 파일 열기…". It becomes an ordinary saved plan: preview, tick, apply as
//! one undoable batch, nothing moved before that. Moves only; a plan file
//! can never delete or rename anything, and every path must stay inside
//! its declared `root`.
//!
//! ```json
//! { "tidy_plan": 1, "root": "D:\\정리", "summary": "…",
//!   "moves": [ { "from": "D:\\정리\\기타\\개발\\소스 코드",
//!                "into": "D:\\정리\\기타\\Go 소스 모음\\소스 코드",
//!                "reason": "…" } ] }
//! ```
//!
//! `from` is a file (it goes into `into`) or a folder (each file inside
//! goes into `into`, keeping its path below `from`). Several items may go
//! into the same folder; same names get "(1)" from the mover. Missing
//! sources are listed as not taken, not an error.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::config::expand_home;
use crate::paths::normalize_lexical;

use super::restructure::{Plan, PlanError, PlanSource, PlannedMove, Rejected};

/// Most files one plan file may move.
pub const IMPORT_CAP: usize = 20_000;

#[derive(Debug, Deserialize)]
struct RawFile {
    tidy_plan: u32,
    root: String,
    #[serde(default)]
    summary: String,
    moves: Vec<RawItem>,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    from: String,
    into: String,
    #[serde(default)]
    reason: String,
}

fn bad(msg: impl Into<String>) -> PlanError {
    PlanError::Corrupt(msg.into())
}

fn abs(p: &str) -> Option<PathBuf> {
    let p = expand_home(Path::new(p.trim()));
    if !p.is_absolute() || p.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    Some(normalize_lexical(&p))
}

fn key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Inside `dir` (or equal), case-insensitively on Windows.
fn within(p: &Path, dir: &Path) -> bool {
    let (p, d) = (key(p), key(dir));
    p == d || p.starts_with(&format!("{}/", d.trim_end_matches('/')))
}

/// Parse and check a plan file against the disk. The plan is not saved.
pub fn import_plan(text: &str) -> Result<Plan, PlanError> {
    let raw: RawFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| bad(format!("제안 파일을 읽을 수 없습니다: {e}")))?;
    if raw.tidy_plan != 1 {
        return Err(bad(format!(
            "이 앱이 모르는 제안 파일 형식입니다 (tidy_plan = {})",
            raw.tidy_plan
        )));
    }
    let root = abs(&raw.root).ok_or_else(|| bad("root는 절대 경로여야 합니다"))?;
    if !root.is_dir() {
        return Err(bad(format!("폴더가 없습니다: {}", root.display())));
    }
    let mut moves: Vec<PlannedMove> = Vec::new();
    let mut rejected = Vec::new();
    let mut taken: Vec<PathBuf> = Vec::new();
    let mut files_total = 0usize;
    for r in raw.moves {
        let reject = |why: &str, rejected: &mut Vec<Rejected>| {
            rejected.push(Rejected {
                from: r.from.clone(),
                to: r.into.clone(),
                reason: why.into(),
            })
        };
        let (Some(from), Some(into)) = (abs(&r.from), abs(&r.into)) else {
            reject("절대 경로가 아님", &mut rejected);
            continue;
        };
        if !within(&from, &root) || !within(&into, &root) || key(&from) == key(&root) {
            reject("root 밖", &mut rejected);
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&from) else {
            reject("원본이 없음", &mut rejected);
            continue;
        };
        if taken.iter().any(|t| within(&from, t) || within(t, &from)) {
            reject("다른 항목과 겹침", &mut rejected);
            continue;
        }
        let reason: String = r.reason.trim().chars().take(200).collect();
        let reason = if reason.is_empty() {
            "제안 파일".to_string()
        } else {
            reason
        };
        if meta.is_file() {
            if files_total + 1 > IMPORT_CAP {
                reject("한 번에 옮길 수 있는 파일 수를 넘음", &mut rejected);
                continue;
            }
            let name = from.file_name().map(PathBuf::from).unwrap_or_default();
            let to = into.join(name);
            if key(&to) == key(&from) {
                continue;
            }
            let mut warnings = Vec::new();
            if to.exists() {
                warnings.push("목적지에 같은 이름이 있어 '(1)'처럼 번호가 붙습니다".into());
            }
            files_total += 1;
            taken.push(from.clone());
            moves.push(PlannedMove {
                id: moves.len() as u32 + 1,
                from,
                to,
                is_dir: false,
                files: 1,
                bytes: meta.len(),
                reason,
                warnings,
                context: None,
                encrypt: false,
            });
        } else if meta.is_dir() {
            // A folder moves as one unit (Mover::move_dir): its files go into
            // `into` keeping their paths, then the emptied folder is removed.
            if within(&into, &from) {
                reject("폴더를 자기 안으로 옮길 수 없음", &mut rejected);
                continue;
            }
            let (mut files, mut bytes) = (0u64, 0u64);
            for e in walkdir::WalkDir::new(&from)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
            {
                files += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
            if files == 0 {
                reject("빈 폴더 (옮길 파일 없음)", &mut rejected);
                continue;
            }
            if files_total + files as usize > IMPORT_CAP {
                reject("한 번에 옮길 수 있는 파일 수를 넘음", &mut rejected);
                continue;
            }
            files_total += files as usize;
            let mut warnings = Vec::new();
            if into.is_dir() || moves.iter().any(|m| m.is_dir && key(&m.to) == key(&into)) {
                warnings.push("같은 폴더로 모입니다 (같은 이름 파일은 '(1)')".into());
            }
            taken.push(from.clone());
            moves.push(PlannedMove {
                id: moves.len() as u32 + 1,
                from,
                to: into,
                is_dir: true,
                files,
                bytes,
                reason,
                warnings,
                context: None,
                encrypt: false,
            });
        } else {
            reject("파일이나 폴더가 아님", &mut rejected);
        }
    }
    let summary: String = raw.summary.trim().chars().take(2000).collect();
    Ok(Plan {
        id: 0,
        root,
        source: PlanSource::Imported,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        summary: if summary.is_empty() {
            format!("제안 파일: 파일 {files_total}개를 옮깁니다.")
        } else {
            summary
        },
        folders: Vec::new(),
        moves,
        rejected,
        applied_batch: None,
        anchor: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn plan_json(root: &Path, moves: &[(&Path, &Path)]) -> String {
        serde_json::json!({
            "tidy_plan": 1,
            "root": root,
            "summary": "흩어진 Go 소스 모으기",
            "moves": moves.iter().map(|(f, i)| serde_json::json!({"from": f, "into": i, "reason": "테스트"})).collect::<Vec<_>>(),
        })
        .to_string()
    }

    #[test]
    fn folders_and_files_go_into_targets_inside_root_only() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("정리");
        let a = root.join("기타").join("개발").join("소스 코드");
        let b = root.join("기타").join("개발").join("소스코드");
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("main.go"), "a").unwrap();
        fs::write(a.join("sub").join("x.go"), "x").unwrap();
        fs::write(b.join("main.go"), "b").unwrap();
        let loose = root.join("메모.txt");
        fs::write(&loose, "m").unwrap();
        let outside = t.path().join("밖.txt");
        fs::write(&outside, "o").unwrap();
        let into = root.join("기타").join("Go 소스 모음").join("소스 코드");
        let json = plan_json(
            &root,
            &[
                (&a, &into),
                (&b, &into),
                (&loose, &root.join("메모")),
                (&outside, &into),
                (&root.join("없음"), &into),
                (&a.join("main.go"), &into),
                (&a, &a.join("안")),
            ],
        );
        let p = import_plan(&json).unwrap();
        assert_eq!(p.source, PlanSource::Imported);
        let got: Vec<_> = p
            .moves
            .iter()
            .map(|m| (m.from.clone(), m.to.clone(), m.is_dir, m.files))
            .collect();
        assert_eq!(
            got,
            vec![
                (a.clone(), into.clone(), true, 2),
                (b.clone(), into.clone(), true, 1),
                (loose.clone(), root.join("메모").join("메모.txt"), false, 1),
            ]
        );
        assert!(p.moves[1].warnings[0].contains("같은 폴더로 모입니다"));
        let why: Vec<_> = p.rejected.iter().map(|r| r.reason.as_str()).collect();
        assert_eq!(
            why,
            vec![
                "root 밖",
                "원본이 없음",
                "다른 항목과 겹침",
                "다른 항목과 겹침"
            ]
        );
    }

    #[test]
    fn rejects_unknown_format_and_relative_root() {
        assert!(import_plan(r#"{"tidy_plan": 2, "root": "/", "moves": []}"#).is_err());
        assert!(import_plan(r#"{"tidy_plan": 1, "root": "정리", "moves": []}"#).is_err());
        assert!(import_plan("not json").is_err());
    }
}
