//! Undo: move logged files back to where they came from (SPEC §5.4).

use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use super::{transfer, Mover, MoverError};
use crate::store::{MoveLogEntry, MoveOp};

#[derive(Debug, Clone, Default, Serialize)]
pub struct UndoReport {
    pub batch_id: Option<String>,
    /// `(from, to)`: from the moved location back to the original.
    pub restored: Vec<(PathBuf, PathBuf)>,
    /// Entries left as they are; they stay active and can be retried.
    pub conflicts: Vec<UndoConflict>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UndoConflict {
    pub log_id: i64,
    pub src_path: PathBuf,
    pub dst_path: PathBuf,
    pub reason: String,
}

impl Mover {
    /// Undo the most recent batch that still has active moves.
    /// `Ok(None)` if there is nothing to undo.
    pub fn undo_last(&self) -> Result<Option<UndoReport>, MoverError> {
        match self.store.last_active_batch()? {
            Some(batch) => self.undo_batch(&batch).map(Some),
            None => Ok(None),
        }
    }

    pub fn undo_batch(&self, batch_id: &str) -> Result<UndoReport, MoverError> {
        let entries = self.store.moves_in_batch(batch_id)?;
        let mut report = UndoReport {
            batch_id: Some(batch_id.to_string()),
            ..Default::default()
        };
        // Newest first so chained moves inside one batch unwind correctly.
        for e in entries.into_iter().filter(|e| e.undone_at.is_none()) {
            self.undo_entry(e, &mut report)?;
        }
        self.remove_created_dirs(batch_id)?;
        Ok(report)
    }

    /// (M12) Folders a whole-folder move created at its destination are
    /// removed again once undo has emptied them (deepest first). One that
    /// still holds something (a file not restored, or one the user added)
    /// stays and is tried again on the next undo of the batch.
    fn remove_created_dirs(&self, batch_id: &str) -> Result<(), MoverError> {
        for d in self.store.created_dirs(batch_id)? {
            if !d.is_dir() {
                self.store.forget_created_dir(batch_id, &d)?;
                continue;
            }
            self.expected.expect(&d);
            if fs::remove_dir(&d).is_ok() {
                tracing::debug!(dir = %d.display(), "removed folder created by the undone move");
                self.store.forget_created_dir(batch_id, &d)?;
            }
        }
        Ok(())
    }

    /// Undo every finished move since `since` (unix seconds), newest first;
    /// `auto_only` = only rule/AI moves (watch-time automation), not the
    /// user's own confirmations. One report for all of them.
    pub fn undo_since(&self, since: i64, auto_only: bool) -> Result<UndoReport, MoverError> {
        let entries = self.store.active_moves_since(since, auto_only)?;
        let mut report = UndoReport::default();
        for e in entries {
            self.undo_entry(e, &mut report)?;
        }
        Ok(report)
    }

    /// Undo a single `move_log` row.
    pub fn undo_one(&self, log_id: i64) -> Result<UndoReport, MoverError> {
        let mut report = UndoReport::default();
        match self.store.get_move(log_id)? {
            Some(e) if e.undone_at.is_none() => {
                report.batch_id = Some(e.batch_id.clone());
                self.undo_entry(e, &mut report)?;
            }
            Some(e) => report.conflicts.push(conflict(&e, "already undone")),
            None => report.conflicts.push(UndoConflict {
                log_id,
                src_path: PathBuf::new(),
                dst_path: PathBuf::new(),
                reason: "no such move".into(),
            }),
        }
        Ok(report)
    }

    fn undo_entry(&self, e: MoveLogEntry, report: &mut UndoReport) -> Result<(), MoverError> {
        if e.in_progress {
            report.conflicts.push(conflict(
                &e,
                "this move has not finished (still copying, or interrupted — recovered on the next watch start)",
            ));
            return Ok(());
        }
        if !e.dst_path.is_file() {
            report
                .conflicts
                .push(conflict(&e, "moved file is no longer at its destination"));
            return Ok(());
        }
        if e.src_path.exists() {
            report.conflicts.push(conflict(
                &e,
                "another file now exists at the original location",
            ));
            return Ok(());
        }
        if e.op == MoveOp::Encrypt {
            return self.undo_encrypt(e, report);
        }
        if let Some(parent) = e.src_path.parent() {
            fs::create_dir_all(parent).map_err(MoverError::io("create dir", parent))?;
        }
        self.expected.expect(&e.dst_path);
        self.expected.expect(&e.src_path);
        match transfer(&e.dst_path, &e.src_path, None, &self.transfer) {
            Ok(()) => {
                self.store.mark_undone(e.id)?;
                tracing::info!(from = %e.dst_path.display(), to = %e.src_path.display(), "undone");
                report.restored.push((e.dst_path, e.src_path));
            }
            Err(err) => report.conflicts.push(conflict(&e, &err.to_string())),
        }
        Ok(())
    }
}

pub(super) fn conflict(e: &MoveLogEntry, reason: &str) -> UndoConflict {
    UndoConflict {
        log_id: e.id,
        src_path: e.src_path.clone(),
        dst_path: e.dst_path.clone(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::DuplicatePolicy;
    use crate::mover::{MoveOutcome, MoveRequest};
    use crate::store::{DecidedBy, Store};

    fn mv(m: &Mover, src: &Path, dest: &Path, batch: &str) -> PathBuf {
        match m
            .move_file(&MoveRequest {
                src: src.to_path_buf(),
                dest_dir: dest.to_path_buf(),
                batch_id: batch.into(),
                decided_by: DecidedBy::Rule,
                rule_name: None,
                ai_confidence: None,
                new_name: None,
                encrypt: false,
            })
            .unwrap()
        {
            MoveOutcome::Moved { dst, .. } => dst,
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn undo_last_restores_whole_batch_then_previous() {
        let dir = tempfile::tempdir().unwrap();
        let desk = dir.path().join("바탕 화면");
        let dest = dir.path().join("보관 함");
        fs::create_dir(&desk).unwrap();
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip);

        let a = desk.join("가 1.txt");
        let b = desk.join("나 2.txt");
        let c = desk.join("다 3.txt");
        for p in [&a, &b, &c] {
            fs::write(p, p.to_string_lossy().as_bytes()).unwrap();
        }
        mv(&m, &a, &dest, "first");
        mv(&m, &b, &dest, "second");
        mv(&m, &c, &dest, "second");

        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.batch_id.as_deref(), Some("second"));
        assert_eq!(r.restored.len(), 2);
        assert!(r.conflicts.is_empty());
        assert!(b.exists() && c.exists() && !a.exists());

        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.batch_id.as_deref(), Some("first"));
        assert!(a.exists());
        assert!(m.undo_last().unwrap().is_none());
        assert!(m
            .store()
            .was_undone(&a, &super::super::hash_file(&a).unwrap())
            .unwrap());
    }

    #[test]
    fn undo_refuses_to_overwrite_new_file_at_origin() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("보고서.txt");
        fs::write(&src, "v1").unwrap();
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip);
        let dst = mv(&m, &src, &dir.path().join("d"), "b");
        fs::write(&src, "new file with same name").unwrap();

        let r = m.undo_last().unwrap().unwrap();
        assert!(r.restored.is_empty());
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(fs::read_to_string(&src).unwrap(), "new file with same name");
        assert_eq!(fs::read_to_string(&dst).unwrap(), "v1");
        // Still active, so it can be retried once the clash is resolved.
        assert_eq!(m.store().last_active_batch().unwrap().as_deref(), Some("b"));
    }

    #[test]
    fn undo_recreates_missing_origin_folder_and_reports_missing_dst() {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("사라질 폴더");
        fs::create_dir(&origin).unwrap();
        let src = origin.join("x.txt");
        fs::write(&src, "x").unwrap();
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip);
        mv(&m, &src, &dir.path().join("d"), "b");
        fs::remove_dir(&origin).unwrap();
        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.restored.len(), 1);
        assert!(src.exists());

        let y = dir.path().join("y.txt");
        fs::write(&y, "y").unwrap();
        let dst = mv(&m, &y, &dir.path().join("d"), "c");
        fs::rename(&dst, dir.path().join("elsewhere")).unwrap();
        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.conflicts.len(), 1);
        assert!(r.conflicts[0].reason.contains("no longer"));
    }
}
