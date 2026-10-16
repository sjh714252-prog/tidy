//! Recovery of moves interrupted by a crash, a power cut or a lost NAS
//! connection (SPEC §5.4, §5.9 "중단 시 .tmp 정리").
//!
//! The mover writes a `move_log` row as 'moving' before touching the file
//! and marks it 'done' afterwards. A row still 'moving' later means the
//! process stopped in between. Looking at what is on disk decides:
//!
//! | source | destination | meaning / action |
//! |---|---|---|
//! | gone | there | the move finished → mark done |
//! | there | missing | it never finished → withdraw the row, remove our scratch copies |
//! | there | there, both = logged hash | stopped before removing the source → finish (remove the source) |
//! | there | there, differ | left alone, row withdrawn, reported |
//! | gone | missing | share offline? → keep the row, try again later |
//!
//! Only rows older than a grace period and not running in this process are
//! touched, and a scratch copy written to within the grace period means a
//! copy may still be running elsewhere.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use super::transfer::tmp_prefix;
use super::{hash_file, InFlight, Mover, MoverError};
use crate::store::MoveLogEntry;

/// Rows younger than this are left alone (a long copy may be running in
/// another Tidy process).
pub const DEFAULT_RECOVERY_GRACE: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default, Serialize)]
pub struct RecoveryReport {
    /// Moves that had finished on disk; now marked done.
    pub completed: Vec<PathBuf>,
    /// Moves that never happened; the file is still at its source.
    pub rolled_back: Vec<PathBuf>,
    /// Scratch copies (`.<name>.tidy-tmp.*`) removed.
    pub tmp_removed: usize,
    /// Both copies exist and differ: nothing removed, user should look.
    pub attention: Vec<String>,
    /// Neither copy reachable (share offline): retried next time.
    pub unresolved: usize,
}

impl RecoveryReport {
    pub fn is_empty(&self) -> bool {
        self.completed.is_empty()
            && self.rolled_back.is_empty()
            && self.tmp_removed == 0
            && self.attention.is_empty()
            && self.unresolved == 0
    }
}

impl Mover {
    /// Resolve moves left 'moving' for longer than `grace`.
    pub fn recover_interrupted(&self, grace: Duration) -> Result<RecoveryReport, MoverError> {
        let before = crate::store::now_unix() - grace.as_secs() as i64;
        let mut report = RecoveryReport::default();
        for e in self.store.interrupted_moves(before)? {
            if InFlight::contains(&e.dst_path) || fresh_tmp(&e.dst_path, grace) {
                continue;
            }
            self.recover_one(&e, grace, &mut report)?;
        }
        if !report.is_empty() {
            tracing::info!(?report, "interrupted moves recovered");
        }
        Ok(report)
    }

    fn recover_one(
        &self,
        e: &MoveLogEntry,
        grace: Duration,
        report: &mut RecoveryReport,
    ) -> Result<(), MoverError> {
        let (src, dst) = (&e.src_path, &e.dst_path);
        match (src.is_file(), dst.is_file()) {
            (false, true) => {
                self.store.complete_move(e.id)?;
                report.completed.push(dst.clone());
            }
            (true, false) => {
                // Only when the destination folder is reachable: an offline
                // share also looks like "missing".
                if dst.parent().is_some_and(Path::is_dir) {
                    report.tmp_removed += remove_stale_tmp(dst, grace);
                    self.store.withdraw_move(e.id)?;
                    report.rolled_back.push(src.clone());
                } else {
                    report.unresolved += 1;
                }
            }
            (true, true) => {
                let logged = e.hash.as_deref();
                let same = logged.is_some()
                    && hash_file(dst).ok().as_deref() == logged
                    && hash_file(src).ok().as_deref() == logged;
                if same {
                    // The verified copy was renamed into place; the process
                    // stopped before removing the source. Finish the move.
                    self.expected.expect(src);
                    fs::remove_file(src)
                        .map_err(MoverError::io("remove source after verified copy", src))?;
                    self.store.complete_move(e.id)?;
                    report.completed.push(dst.clone());
                } else {
                    self.store.withdraw_move(e.id)?;
                    report.attention.push(format!(
                        "{} → {}: both copies exist and differ; nothing was removed",
                        src.display(),
                        dst.display()
                    ));
                }
            }
            (false, false) => report.unresolved += 1,
        }
        Ok(())
    }
}

fn scratch_copies(dst: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    let (Some(dir), prefix) = (dst.parent(), tmp_prefix(dst)) else {
        return Vec::new();
    };
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter(|d| d.file_name().to_string_lossy().starts_with(&prefix))
        .map(|d| (d.path(), d.metadata().and_then(|m| m.modified()).ok()))
        .collect()
}

fn is_fresh(mtime: Option<SystemTime>, grace: Duration) -> bool {
    mtime
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < grace)
}

/// A scratch copy for `dst` was written to recently: a copy may be running.
fn fresh_tmp(dst: &Path, grace: Duration) -> bool {
    scratch_copies(dst)
        .into_iter()
        .any(|(_, m)| is_fresh(m, grace))
}

/// Remove our own stale scratch copies for `dst`; returns how many.
fn remove_stale_tmp(dst: &Path, grace: Duration) -> usize {
    scratch_copies(dst)
        .into_iter()
        .filter(|(_, m)| !is_fresh(*m, grace))
        .filter(|(p, _)| fs::remove_file(p).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DuplicatePolicy;
    use crate::store::{DecidedBy, NewMove, Store};

    fn mover() -> Mover {
        Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
    }

    /// A row as the mover writes it before touching the file.
    fn begun(m: &Mover, src: &Path, dst: &Path) -> i64 {
        m.store()
            .begin_move(&NewMove {
                batch_id: "b".into(),
                src_path: src.to_path_buf(),
                dst_path: dst.to_path_buf(),
                hash: hash_file(src).ok().or_else(|| hash_file(dst).ok()),
                size: None,
                decided_by: DecidedBy::Rule,
                rule_name: None,
                ai_confidence: None,
            })
            .unwrap()
    }

    fn age(p: &Path) {
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    #[test]
    fn crash_during_copy_rolls_back_and_cleans_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("바탕 화면 보고서.hwp");
        fs::write(&src, "원본").unwrap();
        let nas = dir.path().join("NAS 공유");
        fs::create_dir(&nas).unwrap();
        let dst = nas.join("바탕 화면 보고서.hwp");
        let m = mover();
        let id = begun(&m, &src, &dst);
        let tmp = nas.join(format!("{}{}", tmp_prefix(&dst), "deadbeef"));
        fs::write(&tmp, "원").unwrap(); // half-written copy
        m.store()
            .conn()
            .execute("UPDATE move_log SET moved_at = moved_at - 3600", [])
            .unwrap();

        // Fresh scratch copy: a copy may still be running → untouched.
        let r = m.recover_interrupted(Duration::from_secs(60)).unwrap();
        assert!(r.is_empty() && tmp.exists());
        assert!(m.store().get_move(id).unwrap().unwrap().in_progress);

        age(&tmp);
        let r = m.recover_interrupted(Duration::from_secs(60)).unwrap();
        assert_eq!(r.rolled_back, vec![src.clone()]);
        assert_eq!(r.tmp_removed, 1);
        assert!(!tmp.exists());
        assert_eq!(fs::read_to_string(&src).unwrap(), "원본");
        assert!(m.store().get_move(id).unwrap().is_none());
    }

    #[test]
    fn finished_and_half_finished_moves_are_completed() {
        let dir = tempfile::tempdir().unwrap();
        let m = mover();
        // 1) renamed, row not yet marked done.
        let a_dst = dir.path().join("a 옮김.txt");
        fs::write(&a_dst, "a").unwrap();
        let a = begun(&m, &dir.path().join("a.txt"), &a_dst);
        // 2) verified copy in place, source not yet removed.
        let b_src = dir.path().join("b.txt");
        let b_dst = dir.path().join("b 옮김.txt");
        fs::write(&b_src, "b").unwrap();
        fs::write(&b_dst, "b").unwrap();
        let b = begun(&m, &b_src, &b_dst);
        // 3) both there but the source was edited since: keep both.
        let c_src = dir.path().join("c.txt");
        let c_dst = dir.path().join("c 옮김.txt");
        fs::write(&c_src, "c").unwrap();
        fs::write(&c_dst, "c").unwrap();
        let c = begun(&m, &c_src, &c_dst);
        fs::write(&c_src, "c 수정").unwrap();
        // 4) share offline: neither reachable.
        let d = begun(
            &m,
            &dir.path().join("없음.txt"),
            &dir.path().join("끊긴 NAS").join("없음.txt"),
        );

        let r = m.recover_interrupted(Duration::ZERO).unwrap();
        assert_eq!(r.completed, vec![a_dst.clone(), b_dst.clone()]);
        assert_eq!(r.attention.len(), 1);
        assert_eq!(r.unresolved, 1);
        assert!(!b_src.exists() && b_dst.exists());
        assert!(c_src.exists() && c_dst.exists());
        for id in [a, b] {
            assert!(!m.store().get_move(id).unwrap().unwrap().in_progress);
        }
        assert!(m.store().get_move(c).unwrap().is_none());
        assert!(m.store().get_move(d).unwrap().unwrap().in_progress);
        // Completed moves are undoable like any other.
        let u = m.undo_one(b).unwrap();
        assert_eq!(u.restored.len(), 1);
        assert!(b_src.exists());
    }

    #[test]
    fn young_rows_wait_for_the_grace_period() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("x.txt");
        fs::write(&dst, "x").unwrap();
        let m = mover();
        let id = begun(&m, &dir.path().join("원래.txt"), &dst);
        assert!(m
            .recover_interrupted(DEFAULT_RECOVERY_GRACE)
            .unwrap()
            .is_empty());
        let u = m.undo_one(id).unwrap();
        assert_eq!(u.conflicts.len(), 1, "unfinished moves are not undone");
    }
}
