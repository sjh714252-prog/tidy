//! Confirmation queue (SPEC §5.3): resolve a `pending` entry with the
//! user's decision. Shared by `tidy pending --move/--skip/--ignore` and the
//! desktop confirm popup / queue screen.
//!
//! Moves go through the [`Mover`] (logged in `move_log`, undoable) with
//! `decided_by = user`. "Skip" and "ignore" never touch the file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::DuplicatePolicy;
use crate::engine::{prepare_vault_index, reindex_for, Action};
use crate::index::IndexOptions;
use crate::mover::{MoveOutcome, MoveRequest, Mover, MoverError};
use crate::paths::normalize_lexical;
use crate::store::{DecidedBy, DecisionAction, NewDecision, PendingSource, StoreError};

#[derive(Debug, Error)]
pub enum PendingError {
    #[error("no pending entry with id {0}")]
    NotFound(i64),
    #[error("no destination: the entry has no suggestion and none was given")]
    NoDestination,
    #[error(transparent)]
    Mover(#[from] MoverError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl PendingError {
    /// Plain-Korean message for the popup/queue. A file still open in
    /// another program is the common case: say so instead of showing the
    /// OS error code.
    pub fn user_message(&self) -> String {
        match self {
            PendingError::Mover(e) if e.is_transient() => format!(
                "파일이 다른 프로그램에서 열려 있거나 잠겨 있어 옮길 수 없습니다. 그 프로그램에서 파일을 닫은 뒤 다시 '이동'을 누르세요. ({e})"
            ),
            PendingError::Mover(MoverError::DestinationExists(p)) => format!(
                "옮길 곳에 같은 이름의 파일이 이미 있습니다: {}",
                p.display()
            ),
            PendingError::NotFound(_) => "이미 처리된 항목입니다.".into(),
            PendingError::NoDestination => "옮길 곳이 정해지지 않았습니다. '다른 곳…'으로 폴더를 고르세요.".into(),
            other => format!("옮기지 못했습니다: {other}"),
        }
    }
}

/// What the user chose in the confirm popup / queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    /// Move to `dest_dir`, or to the suggested folder when `None`
    /// ("이동" / "다른 곳 선택"); with `new_name` also rename
    /// ("이름 바꿔 이동").
    Move {
        dest_dir: Option<PathBuf>,
        #[serde(default)]
        new_name: Option<String>,
        /// (M14) The user's answer to "똑같은 파일이 이미 있습니다"
        /// (그대로 두기 / 번호 붙여 이동 / 기존 것 휴지통 후 이동).
        #[serde(default)]
        on_duplicate: Option<DuplicatePolicy>,
    },
    /// Leave the file and drop it from the queue, this time only ("건너뜀").
    Skip,
    /// Leave the file and never organise this path again ("무시").
    Ignore,
}

/// Outcome of resolving one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Resolution {
    Moved {
        src: PathBuf,
        dst: PathBuf,
        log_id: i64,
        batch_id: String,
    },
    /// An identical file already exists at the destination; the source was
    /// left in place (duplicate_policy = skip).
    Duplicate {
        src: PathBuf,
        existing: PathBuf,
    },
    AlreadyInPlace {
        path: PathBuf,
    },
    Skipped {
        path: PathBuf,
    },
    Ignored {
        path: PathBuf,
    },
    /// The file no longer exists; the entry was dropped.
    Gone {
        path: PathBuf,
    },
}

/// Above this size the "already there?" check compares sizes only (SPEC
/// §14.9 1: "500MB 초과는 크기+이름만").
pub const DUP_HASH_LIMIT: u64 = 500 * 1024 * 1024;

type DupKey = (
    PathBuf,
    PathBuf,
    u64,
    u64,
    Option<std::time::SystemTime>,
    Option<std::time::SystemTime>,
);

static DUP_CACHE: std::sync::Mutex<Option<std::collections::HashMap<DupKey, bool>>> =
    std::sync::Mutex::new(None);

/// (M14) The file already at `dest_dir/<name>` (`new_name` or the source's
/// name, as the mover would name it) when it has the same content as `src`
/// — what the confirm popup / queue warns about before the user decides.
/// Files over [`DUP_HASH_LIMIT`] count as the same when the sizes match.
/// Results are cached by both files' size and mtime.
pub fn duplicate_at_dest(src: &Path, dest_dir: &Path, new_name: Option<&str>) -> Option<PathBuf> {
    let name: std::ffi::OsString = match new_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => match (Path::new(n).extension(), src.extension()) {
            (None, Some(ext)) => format!("{n}.{}", ext.to_string_lossy()).into(),
            _ => n.into(),
        },
        None => src.file_name()?.to_os_string(),
    };
    let existing = normalize_lexical(dest_dir).join(name);
    if existing == src {
        return None;
    }
    let (a, b) = (
        std::fs::metadata(src).ok()?,
        std::fs::metadata(&existing).ok()?,
    );
    if !a.is_file() || !b.is_file() || a.len() != b.len() {
        return None;
    }
    let key: DupKey = (
        src.to_path_buf(),
        existing.clone(),
        a.len(),
        b.len(),
        a.modified().ok(),
        b.modified().ok(),
    );
    let cached = DUP_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&key).copied());
    let same = match cached {
        Some(v) => v,
        None => {
            let v = a.len() > DUP_HASH_LIMIT
                || matches!(
                    (crate::mover::hash_file(src), crate::mover::hash_file(&existing)),
                    (Ok(x), Ok(y)) if x == y
                );
            let mut g = DUP_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            let m = g.get_or_insert_with(Default::default);
            if m.len() > 4096 {
                m.clear();
            }
            m.insert(key, v);
            v
        }
    };
    same.then_some(existing)
}

/// Apply `decision` to pending entry `id`. The entry is removed from the
/// queue unless the move fails (then it stays for another try).
pub fn resolve(
    mover: &Mover,
    id: i64,
    decision: &Decision,
    batch_id: &str,
    index: Option<&IndexOptions>,
) -> Result<Resolution, PendingError> {
    let store = mover.store();
    let entry = store.get_pending(id)?.ok_or(PendingError::NotFound(id))?;
    let path = entry.path.clone();

    if !path.is_file() {
        store.remove_pending_id(id)?;
        return Ok(Resolution::Gone { path });
    }

    let (resolution, action, chosen) = match decision {
        Decision::Skip => (
            Resolution::Skipped { path: path.clone() },
            DecisionAction::Skip,
            None,
        ),
        Decision::Ignore => {
            store.add_ignored(&path)?;
            (
                Resolution::Ignored { path: path.clone() },
                DecisionAction::Ignore,
                None,
            )
        }
        Decision::Move {
            dest_dir,
            new_name,
            on_duplicate,
        } => {
            let dest_dir = dest_dir
                .clone()
                .or(entry.suggested_dest.clone())
                .ok_or(PendingError::NoDestination)?;
            let new_name = new_name.clone().filter(|n| !n.trim().is_empty());
            let rule = match entry.source {
                PendingSource::Rule => entry.reason.clone(),
                PendingSource::Ai => None,
            };
            if entry.encrypt {
                if let Some(opts) = index {
                    prepare_vault_index(store, &path, opts);
                }
            }
            let out = mover.move_file(&MoveRequest {
                src: path.clone(),
                dest_dir: dest_dir.clone(),
                batch_id: batch_id.to_string(),
                decided_by: DecidedBy::User,
                rule_name: rule.clone(),
                ai_confidence: entry.confidence,
                new_name: new_name.clone(),
                encrypt: entry.encrypt,
                on_duplicate: *on_duplicate,
            })?;
            let label = rule.unwrap_or_else(|| "확인".into());
            let (resolution, action) = match out {
                MoveOutcome::Moved { log_id, src, dst } => (
                    Resolution::Moved {
                        src: src.clone(),
                        dst: dst.clone(),
                        log_id,
                        batch_id: batch_id.to_string(),
                    },
                    Action::Moved {
                        src,
                        dst,
                        rule: label,
                        batch_id: batch_id.to_string(),
                        log_id,
                        by: DecidedBy::User,
                        confidence: entry.confidence,
                        encrypted: entry.encrypt,
                    },
                ),
                MoveOutcome::SkippedDuplicate { src, existing } => (
                    Resolution::Duplicate {
                        src: src.clone(),
                        existing: existing.clone(),
                    },
                    Action::Duplicate {
                        src,
                        existing,
                        rule: label,
                    },
                ),
                MoveOutcome::AlreadyInPlace(p) => (
                    Resolution::AlreadyInPlace { path: p.clone() },
                    Action::AlreadyInPlace {
                        path: p,
                        rule: label,
                    },
                ),
            };
            if let Some(opts) = index {
                reindex_for(store, &action, opts);
            }
            let kind = if new_name.is_some() {
                DecisionAction::RenameMove
            } else {
                DecisionAction::Move
            };
            (resolution, kind, Some(normalize_lexical(&dest_dir)))
        }
    };

    // Learner input (SPEC §5.2): what was suggested vs. what the user chose.
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = crate::paths::ext_lower(&path);
    let src_dir = path.parent().unwrap_or_else(|| Path::new(""));
    if let Err(e) = store.insert_decision(&NewDecision {
        file_name: &file_name,
        ext: (!ext.is_empty()).then_some(ext.as_str()),
        src_dir,
        suggested_dest: entry.suggested_dest.as_deref(),
        chosen_dest: chosen.as_deref(),
        action,
        source: entry.source,
    }) {
        tracing::warn!("could not record decision: {e}");
    }
    store.remove_pending_id(id)?;
    Ok(resolution)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::store::Store;

    fn setup() -> (tempfile::TempDir, Mover, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let desk = dir.path().join("바탕 화면");
        fs::create_dir(&desk).unwrap();
        let file = desk.join("설치 프로그램 v1.exe");
        fs::write(&file, b"MZ").unwrap();
        let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip);
        mover
            .store()
            .upsert_pending(&file, Some(&dir.path().join("설치 파일")), Some("설치파일"))
            .unwrap();
        (dir, mover, file)
    }

    fn only_id(m: &Mover) -> i64 {
        let p = m.store().list_pending().unwrap();
        assert_eq!(p.len(), 1);
        p[0].id
    }

    #[test]
    fn move_to_suggestion_is_logged_as_user_and_undoable() {
        let (dir, m, file) = setup();
        let id = only_id(&m);
        let r = resolve(
            &m,
            id,
            &Decision::Move {
                dest_dir: None,
                new_name: None,
                on_duplicate: None,
            },
            "b1",
            None,
        )
        .unwrap();
        let Resolution::Moved { dst, log_id, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(
            dst,
            dir.path().join("설치 파일").join("설치 프로그램 v1.exe")
        );
        assert!(!file.exists() && dst.is_file());
        let e = m.store().get_move(log_id).unwrap().unwrap();
        assert_eq!(e.decided_by, DecidedBy::User);
        assert_eq!(e.rule_name.as_deref(), Some("설치파일"));
        assert_eq!(m.store().pending_count().unwrap(), 0);
        m.undo_last().unwrap().unwrap();
        assert!(file.exists());
    }

    #[test]
    fn decisions_are_recorded_for_the_learner() {
        let (dir, m, file) = setup();
        let id = only_id(&m);
        resolve(
            &m,
            id,
            &Decision::Move {
                dest_dir: None,
                new_name: Some("설치 v1 (보관)".into()),
                on_duplicate: None,
            },
            "b",
            None,
        )
        .unwrap();
        let d = &m.store().recent_decisions(1).unwrap()[0];
        assert_eq!(d.action, DecisionAction::RenameMove);
        assert_eq!(d.file_name, "설치 프로그램 v1.exe");
        assert_eq!(
            d.chosen_dest.as_deref(),
            Some(dir.path().join("설치 파일").as_path())
        );
        assert!(dir
            .path()
            .join("설치 파일")
            .join("설치 v1 (보관).exe")
            .is_file());
        assert!(!file.exists());

        m.store()
            .upsert_pending(&dir.path().join("없음.txt"), None, None)
            .unwrap();
        let before = m.store().recent_decisions(10).unwrap().len();
        let gone = only_id(&m);
        resolve(&m, gone, &Decision::Skip, "b", None).unwrap();
        // "Gone" is not a user decision.
        assert_eq!(m.store().recent_decisions(10).unwrap().len(), before);
    }

    #[test]
    fn move_elsewhere() {
        let (dir, m, _file) = setup();
        let other = dir.path().join("다른 곳");
        let r = resolve(
            &m,
            only_id(&m),
            &Decision::Move {
                dest_dir: Some(other.clone()),
                new_name: None,
                on_duplicate: None,
            },
            "b",
            None,
        )
        .unwrap();
        assert!(matches!(r, Resolution::Moved { ref dst, .. } if dst.starts_with(&other)));
    }

    #[test]
    fn skip_and_ignore_leave_file_alone() {
        let (_dir, m, file) = setup();
        let r = resolve(&m, only_id(&m), &Decision::Skip, "b", None).unwrap();
        assert_eq!(r, Resolution::Skipped { path: file.clone() });
        assert!(file.exists());
        assert!(!m.store().is_ignored(&file).unwrap());

        m.store().upsert_pending(&file, None, None).unwrap();
        let r = resolve(&m, only_id(&m), &Decision::Ignore, "b", None).unwrap();
        assert_eq!(r, Resolution::Ignored { path: file.clone() });
        assert!(file.exists());
        assert!(m.store().is_ignored(&file).unwrap());
        assert_eq!(m.store().pending_count().unwrap(), 0);
    }

    #[test]
    fn gone_file_and_missing_destination() {
        let (_dir, m, file) = setup();
        m.store()
            .upsert_pending(Path::new("/없는/파일.txt"), None, None)
            .unwrap();
        let gone = m
            .store()
            .list_pending()
            .unwrap()
            .into_iter()
            .find(|p| p.path != file)
            .unwrap();
        assert!(matches!(
            resolve(&m, gone.id, &Decision::Skip, "b", None).unwrap(),
            Resolution::Gone { .. }
        ));
        // No suggestion and no explicit destination: error, entry kept.
        m.store().upsert_pending(&file, None, None).unwrap();
        let id = only_id(&m);
        assert!(matches!(
            resolve(
                &m,
                id,
                &Decision::Move {
                    dest_dir: None,
                    new_name: None,
                    on_duplicate: None,
                },
                "b",
                None
            ),
            Err(PendingError::NoDestination)
        ));
        assert_eq!(m.store().pending_count().unwrap(), 1);
        assert!(matches!(
            resolve(&m, 999, &Decision::Skip, "b", None),
            Err(PendingError::NotFound(999))
        ));
    }
}

#[cfg(test)]
mod user_message_tests {
    use super::*;

    #[test]
    fn locked_file_gets_a_plain_korean_message() {
        let locked = MoverError::Io {
            op: "rename",
            path: PathBuf::from("a.docx"),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let m = PendingError::from(locked).user_message();
        assert!(m.contains("다른 프로그램에서 열려"), "{m}");
        assert!(m.contains("다시 '이동'"), "{m}");
        let gone = PendingError::NotFound(3).user_message();
        assert_eq!(gone, "이미 처리된 항목입니다.");
    }

    /// (M14) The queue warns before the decision; the three answers.
    #[test]
    fn duplicate_at_dest_and_on_duplicate_answers() {
        use crate::config::DuplicatePolicy;
        use crate::store::Store;
        use std::fs;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("정리").join("문서");
        fs::create_dir_all(&dest).unwrap();
        let bin = dir.path().join("bin");
        let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
            .with_trash(Arc::new(crate::mover::FolderTrash(bin.clone())));
        let src = dir.path().join("보고서.pdf");
        fs::write(&src, "same").unwrap();
        fs::write(dest.join("보고서.pdf"), "same").unwrap();
        assert_eq!(
            duplicate_at_dest(&src, &dest, None),
            Some(dest.join("보고서.pdf"))
        );
        assert_eq!(duplicate_at_dest(&src, &dest, Some("다른 이름")), None);
        fs::write(dest.join("다른 이름.pdf"), "diff").unwrap();
        assert_eq!(duplicate_at_dest(&src, &dest, Some("다른 이름")), None);

        let queue = |m: &Mover| {
            m.store()
                .upsert_pending(&src, Some(&dest), Some("문서"))
                .unwrap();
            m.store().list_pending().unwrap()[0].id
        };
        let mv = |on| Decision::Move {
            dest_dir: None,
            new_name: None,
            on_duplicate: Some(on),
        };
        // 그대로 두기
        let id = queue(&mover);
        let r = resolve(&mover, id, &mv(DuplicatePolicy::Skip), "b0", None).unwrap();
        assert!(matches!(r, Resolution::Duplicate { .. }), "{r:?}");
        assert!(src.exists());
        assert_eq!(mover.store().pending_count().unwrap(), 0);
        // 번호 붙여 이동
        let id = queue(&mover);
        let r = resolve(&mover, id, &mv(DuplicatePolicy::Rename), "b1", None).unwrap();
        let Resolution::Moved { dst, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(dst, dest.join("보고서 (1).pdf"));
        // 기존 것 휴지통 후 이동
        fs::write(&src, "same").unwrap();
        let id = queue(&mover);
        let r = resolve(&mover, id, &mv(DuplicatePolicy::Replace), "b2", None).unwrap();
        let Resolution::Moved { dst, .. } = r else {
            panic!("{r:?}")
        };
        assert_eq!(dst, dest.join("보고서.pdf"));
        assert_eq!(fs::read_dir(&bin).unwrap().count(), 1);
    }
}
