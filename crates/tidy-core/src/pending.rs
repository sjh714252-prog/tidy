//! Confirmation queue (SPEC §5.3): resolve a `pending` entry with the
//! user's decision. Shared by `tidy pending --move/--skip/--ignore` and the
//! desktop confirm popup / queue screen.
//!
//! Moves go through the [`Mover`] (logged in `move_log`, undoable) with
//! `decided_by = user`. "Skip" and "ignore" never touch the file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::engine::{reindex_for, Action};
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
        Decision::Move { dest_dir, new_name } => {
            let dest_dir = dest_dir
                .clone()
                .or(entry.suggested_dest.clone())
                .ok_or(PendingError::NoDestination)?;
            let new_name = new_name.clone().filter(|n| !n.trim().is_empty());
            let rule = match entry.source {
                PendingSource::Rule => entry.reason.clone(),
                PendingSource::Ai => None,
            };
            let out = mover.move_file(&MoveRequest {
                src: path.clone(),
                dest_dir: dest_dir.clone(),
                batch_id: batch_id.to_string(),
                decided_by: DecidedBy::User,
                rule_name: rule.clone(),
                ai_confidence: entry.confidence,
                new_name: new_name.clone(),
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
    use crate::config::DuplicatePolicy;
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
                    new_name: None
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
