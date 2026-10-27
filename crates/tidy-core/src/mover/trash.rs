//! (M14) "휴지통으로": identical copies go to the OS trash, never deleted
//! (CLAUDE.md rule 2), and every one is a `trash` row in `move_log`
//! (rule 3). The OS trash cannot be restored from programmatically on
//! Windows/macOS, so undo only points the user at the trash (SPEC §14.9 2).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{hash_file, Mover, MoverError};
use crate::store::{DecidedBy, MoveOp, NewMove};

/// What undo says about a `trash` row.
pub const TRASH_UNDO_HINT: &str = "휴지통으로 보낸 파일입니다 — 휴지통에서 복원하세요";

/// Where files sent "to the trash" go.
pub trait TrashBackend: Send + Sync {
    fn trash(&self, path: &Path) -> Result<(), String>;
}

/// The OS trash / recycle bin (`trash` crate).
#[derive(Debug, Default)]
pub struct OsTrash;

impl TrashBackend for OsTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

/// A plain folder standing in for the trash (tests, headless e2e via
/// `TIDY_E2E_TRASH`). Names never collide: `name`, `name (1)`, …
#[derive(Debug)]
pub struct FolderTrash(pub PathBuf);

impl TrashBackend for FolderTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        fs::create_dir_all(&self.0).map_err(|e| e.to_string())?;
        let name = path.file_name().ok_or("no file name")?;
        let mut dst = self.0.join(name);
        if dst.exists() {
            dst = super::unique_name(&dst);
        }
        fs::rename(path, &dst)
            .or_else(|_| fs::copy(path, &dst).and_then(|_| fs::remove_file(path)))
            .map_err(|e| e.to_string())
    }
}

/// The OS trash, or the folder named by `TIDY_E2E_TRASH` (headless GUI
/// tests count what arrives there instead of filling the real trash).
pub(super) fn default_backend() -> std::sync::Arc<dyn TrashBackend> {
    match std::env::var_os("TIDY_E2E_TRASH").filter(|v| !v.is_empty()) {
        Some(dir) => std::sync::Arc::new(FolderTrash(PathBuf::from(dir))),
        None => std::sync::Arc::new(OsTrash),
    }
}

/// Open the OS trash in the file manager (undo hint "휴지통 열기").
/// Returns the location opened, or `None` if this OS has no known one.
pub fn open_trash() -> Option<String> {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg("shell:RecycleBinFolder")
            .spawn();
        Some("shell:RecycleBinFolder".into())
    }
    #[cfg(target_os = "macos")]
    {
        let p = dirs::home_dir()?.join(".Trash");
        let _ = std::process::Command::new("open").arg(&p).spawn();
        Some(p.display().to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let p = dirs::data_dir()?.join("Trash").join("files");
        let _ = std::process::Command::new("xdg-open").arg(&p).spawn();
        Some(p.display().to_string())
    }
}

/// One duplicate group to clean up: keep `keep`, send `trash` away.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DupTrash {
    /// blake3 of the group's content.
    pub hash: String,
    pub keep: PathBuf,
    pub trash: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TrashReport {
    pub batch_id: String,
    /// `(path, size)` of every file sent to the trash.
    pub trashed: Vec<(PathBuf, u64)>,
    /// Files left where they are, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

impl TrashReport {
    pub fn bytes(&self) -> u64 {
        self.trashed.iter().map(|(_, s)| s).sum()
    }
}

impl Mover {
    /// Send the unwanted copies of each group to the trash, all in batch
    /// `batch_id` (one "적용" = one batch). Right before each file goes,
    /// both it and the kept copy are hashed again; a file that changed, or
    /// a group whose kept copy is gone or different, is skipped, so the
    /// last copy of some content is never sent away.
    pub fn trash_duplicates(&self, groups: &[DupTrash], batch_id: &str) -> TrashReport {
        let mut report = TrashReport {
            batch_id: batch_id.to_string(),
            ..Default::default()
        };
        for g in groups {
            let keep_ok =
                g.keep.is_file() && hash_file(&g.keep).map(|h| h == g.hash).unwrap_or(false);
            for p in &g.trash {
                if !keep_ok {
                    report.failed.push((
                        p.clone(),
                        format!("남길 파일이 없거나 바뀌었습니다: {}", g.keep.display()),
                    ));
                    continue;
                }
                if same_path(p, &g.keep) {
                    report
                        .failed
                        .push((p.clone(), "남길 파일과 같은 파일입니다".into()));
                    continue;
                }
                let meta = match fs::symlink_metadata(p) {
                    Ok(m) if m.is_file() => m,
                    Ok(_) => {
                        report
                            .failed
                            .push((p.clone(), "일반 파일이 아닙니다".into()));
                        continue;
                    }
                    Err(e) => {
                        report
                            .failed
                            .push((p.clone(), format!("파일이 없습니다 ({e})")));
                        continue;
                    }
                };
                match hash_file(p) {
                    Ok(h) if h == g.hash => {}
                    Ok(_) => {
                        report
                            .failed
                            .push((p.clone(), "그사이 내용이 바뀌어 남겨 두었습니다".into()));
                        continue;
                    }
                    Err(e) => {
                        report.failed.push((p.clone(), e.to_string()));
                        continue;
                    }
                }
                match self.trash_logged(p, &g.keep, &g.hash, batch_id, DecidedBy::User) {
                    Ok(()) => report.trashed.push((p.clone(), meta.len())),
                    Err(e) => report.failed.push((p.clone(), e.to_string())),
                }
            }
        }
        report
    }

    /// Log a `trash` row (`src` = the file, `dst` = the copy that stays),
    /// send the file to the trash, complete the row; withdraw it when the
    /// trash refuses.
    pub(super) fn trash_logged(
        &self,
        path: &Path,
        kept: &Path,
        hash: &str,
        batch_id: &str,
        by: DecidedBy,
    ) -> Result<(), MoverError> {
        let size = fs::metadata(path).map(|m| m.len()).ok();
        let log_id = self.store.begin_move(&NewMove {
            batch_id: batch_id.to_string(),
            src_path: path.to_path_buf(),
            dst_path: kept.to_path_buf(),
            hash: Some(hash.to_string()),
            size,
            decided_by: by,
            rule_name: None,
            ai_confidence: None,
            op: MoveOp::Trash,
        })?;
        self.expected.expect(path);
        if let Err(msg) = self.trash.trash(path) {
            self.store.withdraw_move(log_id)?;
            return Err(MoverError::Trash {
                path: path.to_path_buf(),
                msg,
            });
        }
        self.store.complete_move(log_id)?;
        tracing::info!(path = %path.display(), kept = %kept.display(), "sent to trash");
        Ok(())
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::DuplicatePolicy;
    use crate::store::Store;

    fn setup() -> (tempfile::TempDir, Mover, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("휴지통");
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
            .with_trash(Arc::new(FolderTrash(bin.clone())));
        (tmp, m, bin)
    }

    #[test]
    fn trashes_copies_logs_rows_and_undo_points_at_trash() {
        let (tmp, m, bin) = setup();
        let r = tmp.path();
        fs::create_dir_all(r.join("정리").join("문서")).unwrap();
        let keep = r.join("정리").join("문서").join("보고서.pdf");
        let a = r.join("보고서 (1).pdf");
        let b = r.join("사본 보고서.pdf");
        for p in [&keep, &a, &b] {
            fs::write(p, "같은 내용").unwrap();
        }
        let hash = hash_file(&keep).unwrap();
        let rep = m.trash_duplicates(
            &[DupTrash {
                hash: hash.clone(),
                keep: keep.clone(),
                trash: vec![a.clone(), b.clone()],
            }],
            "dup1",
        );
        assert_eq!(rep.trashed.len(), 2, "{rep:?}");
        assert_eq!(rep.bytes(), 2 * "같은 내용".len() as u64);
        assert!(keep.exists() && !a.exists() && !b.exists());
        assert_eq!(fs::read_dir(&bin).unwrap().count(), 2);

        let rows = m.store().moves_in_batch("dup1").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|e| e.op == MoveOp::Trash
            && e.dst_path == keep
            && e.hash.as_deref() == Some(hash.as_str())
            && e.decided_by == DecidedBy::User
            && !e.in_progress));
        // Not something "마지막 이동 되돌리기" can do.
        assert_eq!(m.store().last_active_batch().unwrap(), None);
        let u = m.undo_batch("dup1").unwrap();
        assert!(u.restored.is_empty());
        assert_eq!(u.conflicts.len(), 2);
        assert!(u.conflicts.iter().all(|c| c.reason == TRASH_UNDO_HINT));
        // The user restores one from the trash: undo now records it.
        fs::rename(bin.join("보고서 (1).pdf"), &a).unwrap();
        let u = m.undo_batch("dup1").unwrap();
        assert_eq!(u.restored.len(), 1);
        assert_eq!(u.conflicts.len(), 1);
    }

    #[test]
    fn changed_files_and_missing_keep_are_left_alone() {
        let (tmp, m, bin) = setup();
        let r = tmp.path();
        let keep = r.join("a.txt");
        let changed = r.join("b.txt");
        fs::write(&keep, "x").unwrap();
        fs::write(&changed, "y").unwrap(); // listed as a copy, but differs now
        let hash = hash_file(&keep).unwrap();
        let rep = m.trash_duplicates(
            &[
                DupTrash {
                    hash: hash.clone(),
                    keep: keep.clone(),
                    trash: vec![changed.clone(), keep.clone()],
                },
                DupTrash {
                    hash: hash.clone(),
                    keep: r.join("없음.txt"),
                    trash: vec![keep.clone()],
                },
            ],
            "b",
        );
        assert!(rep.trashed.is_empty(), "{rep:?}");
        assert_eq!(rep.failed.len(), 3);
        assert!(keep.exists() && changed.exists());
        assert!(!bin.exists());
        assert!(m.store().moves_in_batch("b").unwrap().is_empty());
    }

    struct Refuse;
    impl TrashBackend for Refuse {
        fn trash(&self, _: &Path) -> Result<(), String> {
            Err("no trash here".into())
        }
    }

    #[test]
    fn refused_trash_withdraws_the_row() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
            .with_trash(Arc::new(Refuse));
        let keep = tmp.path().join("a");
        let other = tmp.path().join("b");
        fs::write(&keep, "z").unwrap();
        fs::write(&other, "z").unwrap();
        let rep = m.trash_duplicates(
            &[DupTrash {
                hash: hash_file(&keep).unwrap(),
                keep,
                trash: vec![other.clone()],
            }],
            "r",
        );
        assert_eq!(rep.failed.len(), 1);
        assert!(other.exists());
        assert!(m.store().moves_in_batch("r").unwrap().is_empty());
    }
}
