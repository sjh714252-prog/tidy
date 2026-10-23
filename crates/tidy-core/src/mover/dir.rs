//! (M12, SPEC §14.7) Moving a whole folder as one unit: every file inside
//! goes through [`Mover::move_file`] in the caller's batch (one `move_log`
//! row each — copy+verify across volumes, interrupted copies recovered per
//! file, undo restores every file to its old path). Empty folders inside
//! are recreated at the destination; afterwards only folders of the source
//! tree that are *empty* are removed (`remove_dir` refuses non-empty
//! folders, so no file can be lost).

use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use super::{MoveOutcome, MoveRequest, Mover, MoverError, TMP_MARKER};
use crate::paths::normalize_lexical;
use crate::store::DecidedBy;

/// Move folder `src` so that it becomes `dest` (its inner structure kept).
#[derive(Debug, Clone)]
pub struct DirMoveRequest {
    pub src: PathBuf,
    /// The folder's new path (not its parent). An existing folder there is
    /// merged into (name clashes get "(1)" like single moves).
    pub dest: PathBuf,
    pub batch_id: String,
    pub decided_by: DecidedBy,
    pub rule_name: Option<String>,
    pub ai_confidence: Option<f64>,
}

/// What happened to the files of one folder move.
#[derive(Debug, Default)]
pub struct DirMoveReport {
    /// Each moved file's outcome (`MoveOutcome::Moved` …), in walk order.
    pub outcomes: Vec<MoveOutcome>,
    /// Files that could not be moved (open in another app …); they and
    /// their folders stay in the source.
    pub errors: Vec<(PathBuf, String)>,
    /// Source folders removed because the move left them empty.
    pub removed_dirs: usize,
}

impl DirMoveReport {
    pub fn moved(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, MoveOutcome::Moved { .. }))
            .count()
    }
}

impl Mover {
    /// Move every file of `req.src` under `req.dest`, keeping relative
    /// paths. Per-file failures are collected, not fatal.
    pub fn move_dir(&self, req: &DirMoveRequest) -> Result<DirMoveReport, MoverError> {
        let src = normalize_lexical(&req.src);
        let dest = normalize_lexical(&req.dest);
        let meta = fs::symlink_metadata(&src).map_err(MoverError::io("stat", &src))?;
        if !meta.is_dir() {
            return Err(MoverError::NotAFolder(src));
        }
        if dest.starts_with(&src) {
            return Err(MoverError::DestinationInside {
                src,
                dest: dest.clone(),
            });
        }
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files: Vec<PathBuf> = Vec::new();
        for e in WalkDir::new(&src)
            .min_depth(1)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().contains(TMP_MARKER))
        {
            let e = e.map_err(|e| MoverError::Io {
                op: "list",
                path: src.clone(),
                source: e.into(),
            })?;
            if e.file_type().is_dir() {
                dirs.push(e.into_path());
            } else if e.file_type().is_file() {
                files.push(e.into_path());
            }
        }
        // Keep the shape, empty folders included.
        fs::create_dir_all(&dest).map_err(MoverError::io("create dir", &dest))?;
        for d in &dirs {
            if let Ok(rel) = d.strip_prefix(&src) {
                let t = dest.join(rel);
                fs::create_dir_all(&t).map_err(MoverError::io("create dir", &t))?;
            }
        }
        let mut report = DirMoveReport::default();
        for f in files {
            let rel = match f.strip_prefix(&src) {
                Ok(r) => r.to_path_buf(),
                Err(_) => continue,
            };
            let dest_dir = dest.join(rel.parent().unwrap_or(Path::new("")));
            let r = MoveRequest {
                src: f.clone(),
                dest_dir,
                batch_id: req.batch_id.clone(),
                decided_by: req.decided_by,
                rule_name: req.rule_name.clone(),
                ai_confidence: req.ai_confidence,
                new_name: None,
                encrypt: false,
            };
            match self.move_file(&r) {
                Ok(o) => report.outcomes.push(o),
                Err(e) => report.errors.push((f, e.to_string())),
            }
        }
        // Deepest first; `remove_dir` only succeeds on empty folders.
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        dirs.push(src.clone());
        for d in dirs {
            self.expected.expect(&d);
            if fs::remove_dir(&d).is_ok() {
                report.removed_dirs += 1;
            }
        }
        tracing::info!(
            src = %src.display(),
            dest = %dest.display(),
            moved = report.moved(),
            errors = report.errors.len(),
            "moved folder"
        );
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DuplicatePolicy;
    use crate::store::Store;

    fn req(src: &Path, dest: &Path) -> DirMoveRequest {
        DirMoveRequest {
            src: src.to_path_buf(),
            dest: dest.to_path_buf(),
            batch_id: "dir1".into(),
            decided_by: DecidedBy::Restructure,
            rule_name: Some("업무 맥락 #1".into()),
            ai_confidence: Some(0.9),
        }
    }

    fn tree(root: &Path) {
        let go = root.join("tools").join("go");
        fs::create_dir_all(go.join("cmd")).unwrap();
        fs::create_dir_all(root.join("빈 폴더")).unwrap();
        fs::write(root.join("README.md"), "중앙선관위 로그 분석 도구").unwrap();
        fs::write(go.join("main.go"), "package main").unwrap();
        fs::write(go.join("main_test.go"), "package main // test").unwrap();
        fs::write(go.join("cmd").join("run.go"), "package cmd").unwrap();
    }

    fn check_moved(dest: &Path) {
        assert_eq!(
            fs::read_to_string(dest.join("README.md")).unwrap(),
            "중앙선관위 로그 분석 도구"
        );
        assert!(dest.join("tools").join("go").join("main_test.go").is_file());
        assert!(dest
            .join("tools")
            .join("go")
            .join("cmd")
            .join("run.go")
            .is_file());
        assert!(
            dest.join("빈 폴더").is_dir(),
            "empty folders keep their place"
        );
    }

    fn run(copy_always: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let desk = tmp.path().join("바탕 화면");
        let src = desk.join("중앙선관위 로그");
        tree(&src);
        let dest = tmp
            .path()
            .join("정리")
            .join("중앙선관위")
            .join("20261023_로그분석")
            .join("개발")
            .join("중앙선관위 로그");
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
            .copy_always(copy_always);
        let r = m.move_dir(&req(&src, &dest)).unwrap();
        assert_eq!(r.moved(), 4, "{:?}", r.errors);
        assert!(r.errors.is_empty());
        check_moved(&dest);
        assert!(!src.exists(), "emptied source folder is removed");
        assert!(desk.is_dir(), "the watch folder itself stays");
        // One batch, one row per file → undo puts everything back.
        let rows = m.store().moves_in_batch("dir1").unwrap();
        assert_eq!(rows.len(), 4);
        let u = m.undo_batch("dir1").unwrap();
        assert_eq!(u.restored.len(), 4, "{:?}", u.conflicts);
        assert_eq!(
            fs::read_to_string(src.join("README.md")).unwrap(),
            "중앙선관위 로그 분석 도구"
        );
        assert!(src
            .join("tools")
            .join("go")
            .join("cmd")
            .join("run.go")
            .is_file());
    }

    #[test]
    fn moves_a_korean_folder_tree_and_undoes_it() {
        run(false);
    }

    #[test]
    fn copy_verify_path_works_for_folders_too() {
        run(true);
    }

    #[test]
    fn refuses_files_and_moving_into_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a");
        tree(&src);
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip);
        assert!(matches!(
            m.move_dir(&req(&src, &src.join("b"))),
            Err(MoverError::DestinationInside { .. })
        ));
        assert!(matches!(
            m.move_dir(&req(&src.join("README.md"), &tmp.path().join("x"))),
            Err(MoverError::NotAFolder(_))
        ));
    }

    /// A file that cannot be moved stays, with its folder; the rest moves.
    #[cfg(unix)]
    #[test]
    fn a_failing_file_stays_with_its_folder() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("업무");
        tree(&src);
        let locked = src.join("tools").join("go").join("cmd");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        if fs::write(locked.join("probe"), "x").is_ok() {
            // Running as root: permissions do not stop us; nothing to test.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let dest = tmp.path().join("정리").join("업무");
        let m =
            Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip).copy_always(true);
        let r = m.move_dir(&req(&src, &dest)).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.errors.len(), 1);
        assert!(locked.join("run.go").is_file(), "left in place");
        assert!(dest.join("README.md").is_file());
    }
}
