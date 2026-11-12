//! (0.14.0) "빈 폴더 정리": folders with no files in them (only empty
//! sub-folders and OS clutter like `Thumbs.db` / `desktop.ini`) are removed
//! on the user's approval. Each removed folder is a `rmdir` row in
//! `move_log` (CLAUDE.md rule 3) and undo makes it again; OS clutter files
//! go to the trash (rule 2), never deleted. `remove_dir` itself refuses a
//! folder that is not empty, so nothing with content can be lost here.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{hash_file, Mover, MoverError};
use crate::store::{DecidedBy, MoveOp, NewMove};

/// Files the OS drops into folders by itself; a folder holding only these
/// still counts as empty (compared case-insensitively).
pub const OS_CLUTTER: &[&str] = &["thumbs.db", "ehthumbs.db", "desktop.ini", ".ds_store"];

/// Deepest nesting looked at; anything deeper counts as content.
const MAX_DEPTH: usize = 32;

pub fn is_os_clutter(name: &str) -> bool {
    let n = name.to_lowercase();
    OS_CLUTTER.contains(&n.as_str())
}

/// An empty folder tree: its folders deepest first (the folder itself last)
/// and the OS clutter files inside.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EmptyTree {
    pub dirs: Vec<PathBuf>,
    pub clutter: Vec<PathBuf>,
}

/// `Some` when `dir` holds nothing but folders and OS clutter. Links,
/// hidden entries (other than clutter) and anything unreadable count as
/// content: when in doubt, the folder is not empty.
pub fn empty_tree(dir: &Path) -> Option<EmptyTree> {
    let meta = fs::symlink_metadata(dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let mut t = EmptyTree::default();
    walk(dir, 0, &mut t).then_some(t)
}

fn walk(dir: &Path, depth: usize, t: &mut EmptyTree) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    for e in rd {
        let Ok(e) = e else { return false };
        let Ok(ft) = e.file_type() else { return false };
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_file() && is_os_clutter(&name) {
            t.clutter.push(e.path());
            continue;
        }
        if ft.is_dir() && !ft.is_symlink() && !name.starts_with('.') {
            if !walk(&e.path(), depth + 1, t) {
                return false;
            }
            continue;
        }
        return false;
    }
    t.dirs.push(dir.to_path_buf());
    true
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EmptyDirReport {
    pub batch_id: String,
    /// Folders asked for that are gone now (with everything inside).
    pub removed: Vec<PathBuf>,
    /// Folders removed in all, nested ones included.
    pub dirs: usize,
    /// Folders left where they are, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

impl Mover {
    /// Remove each folder in `folders` (with its empty sub-folders) in one
    /// batch. Each is checked again right before: one that got a file in
    /// the meantime stays.
    pub fn remove_empty_dirs(&self, folders: &[PathBuf], batch_id: &str) -> EmptyDirReport {
        let mut report = EmptyDirReport {
            batch_id: batch_id.to_string(),
            ..Default::default()
        };
        for f in folders {
            let Some(tree) = empty_tree(f) else {
                report.failed.push((
                    f.clone(),
                    "비어 있지 않거나 열 수 없어 그대로 두었습니다".into(),
                ));
                continue;
            };
            match self.remove_tree(&tree, batch_id) {
                Ok(n) => {
                    report.dirs += n;
                    report.removed.push(f.clone());
                }
                Err(e) => report.failed.push((f.clone(), e.to_string())),
            }
        }
        report
    }

    fn remove_tree(&self, tree: &EmptyTree, batch_id: &str) -> Result<usize, MoverError> {
        for c in &tree.clutter {
            let hash = hash_file(c)?;
            let folder = c.parent().unwrap_or(c);
            self.trash_logged(c, folder, &hash, batch_id, DecidedBy::User)?;
        }
        let mut n = 0;
        for d in &tree.dirs {
            self.remove_dir_logged(d, batch_id)?;
            n += 1;
        }
        Ok(n)
    }

    fn remove_dir_logged(&self, dir: &Path, batch_id: &str) -> Result<(), MoverError> {
        let log_id = self.store.begin_move(&NewMove {
            batch_id: batch_id.to_string(),
            src_path: dir.to_path_buf(),
            dst_path: dir.to_path_buf(),
            hash: None,
            size: None,
            decided_by: DecidedBy::User,
            rule_name: None,
            ai_confidence: None,
            op: MoveOp::RemoveDir,
        })?;
        self.expected.expect(dir);
        if let Err(e) = fs::remove_dir(dir) {
            self.store.withdraw_move(log_id)?;
            return Err(MoverError::io("remove empty folder", dir)(e));
        }
        self.store.complete_move(log_id)?;
        tracing::info!(dir = %dir.display(), "removed empty folder");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::config::DuplicatePolicy;
    use crate::mover::FolderTrash;
    use crate::store::Store;

    fn setup() -> (tempfile::TempDir, Mover, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("휴지통");
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
            .with_trash(Arc::new(FolderTrash(bin.clone())));
        (tmp, m, bin)
    }

    #[test]
    fn empty_tree_sees_only_folders_and_clutter() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("빈").join("java").join("lib")).unwrap();
        fs::write(r.join("빈").join("Thumbs.db"), b"x").unwrap();
        let t = empty_tree(&r.join("빈")).unwrap();
        assert_eq!(t.dirs.len(), 3);
        assert_eq!(
            t.dirs.last().unwrap(),
            &r.join("빈"),
            "the folder itself last"
        );
        assert_eq!(t.clutter.len(), 1);

        fs::create_dir_all(r.join("찬").join("sub")).unwrap();
        fs::write(r.join("찬").join("sub").join("a.txt"), b"x").unwrap();
        assert!(empty_tree(&r.join("찬")).is_none());
        fs::create_dir_all(r.join("숨김").join(".git")).unwrap();
        assert!(
            empty_tree(&r.join("숨김")).is_none(),
            "hidden entries are content"
        );
        fs::create_dir_all(r.join("바로가기")).unwrap();
        fs::write(r.join("바로가기").join("x.lnk"), b"x").unwrap();
        assert!(empty_tree(&r.join("바로가기")).is_none());
    }

    #[test]
    fn removes_logs_and_undo_makes_the_folders_again() {
        let (tmp, m, bin) = setup();
        let r = tmp.path().join("Downloads");
        fs::create_dir_all(r.join("java").join("lib").join("x")).unwrap();
        fs::create_dir_all(r.join("files")).unwrap();
        fs::write(r.join("files").join("desktop.ini"), b"[x]").unwrap();
        fs::create_dir_all(r.join("찬 폴더")).unwrap();
        fs::write(r.join("찬 폴더").join("a.txt"), b"a").unwrap();

        let rep = m.remove_empty_dirs(&[r.join("java"), r.join("files"), r.join("찬 폴더")], "b1");
        assert_eq!(rep.removed.len(), 2, "{rep:?}");
        assert_eq!(rep.dirs, 4);
        assert_eq!(rep.failed.len(), 1);
        assert!(!r.join("java").exists() && !r.join("files").exists());
        assert!(r.join("찬 폴더").join("a.txt").is_file());
        assert!(
            bin.join("desktop.ini").is_file(),
            "clutter went to the trash"
        );
        assert!(
            !m.store().has_moves_into(&r).unwrap(),
            "removed folders are not 'moved into'"
        );

        let u = m.undo_batch("b1").unwrap();
        assert!(r.join("java").join("lib").join("x").is_dir());
        assert!(r.join("files").is_dir());
        assert_eq!(u.restored.len(), 4);
        assert_eq!(
            u.conflicts.len(),
            1,
            "the trashed desktop.ini says where it is"
        );
    }
}
