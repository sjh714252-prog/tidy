//! Mover + Undo (SPEC §5.4).
//!
//! This is the only module allowed to change the filesystem. Every move is
//! written to `move_log` *before* the file is touched (and withdrawn if the
//! move fails), so there is never a move without an undo record.
//! Nothing here deletes user data: the only removals are
//! - the source of a cross-volume move, after the copy was size+hash verified,
//! - our own `.tidy-tmp.*` scratch copies,
//! - `duplicate_policy = "replace"`, which sends the identical existing copy
//!   to the OS trash (`trash` crate), never `remove_file`.

mod undo;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

pub use undo::{UndoConflict, UndoReport};

use crate::config::DuplicatePolicy;
use crate::store::{DecidedBy, NewMove, Store, StoreError};

/// Marker in the names of temporary copies; the watcher ignores them.
pub const TMP_MARKER: &str = ".tidy-tmp.";

#[derive(Debug, Error)]
pub enum MoverError {
    #[error("{op} {path}: {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("not a regular file: {0}")]
    NotAFile(PathBuf),
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("copy verification failed for {0} (size or hash mismatch); source left untouched")]
    VerifyFailed(PathBuf),
    #[error("could not send {path} to trash: {msg}")]
    Trash { path: PathBuf, msg: String },
}

impl MoverError {
    fn io(op: &'static str, path: &Path) -> impl FnOnce(io::Error) -> Self {
        let path = path.to_path_buf();
        move |source| MoverError::Io { op, path, source }
    }

    /// Errors worth retrying later (file still open/locked by another app).
    pub fn is_transient(&self) -> bool {
        match self {
            MoverError::Io { source, .. } => {
                matches!(
                    source.kind(),
                    io::ErrorKind::PermissionDenied
                        | io::ErrorKind::ResourceBusy
                        | io::ErrorKind::WouldBlock
                ) || is_sharing_violation(source)
            }
            _ => false,
        }
    }
}

#[cfg(windows)]
fn is_sharing_violation(e: &io::Error) -> bool {
    // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
    matches!(e.raw_os_error(), Some(32) | Some(33))
}

#[cfg(not(windows))]
fn is_sharing_violation(_: &io::Error) -> bool {
    false
}

/// A requested move of one file into a folder.
#[derive(Debug, Clone)]
pub struct MoveRequest {
    pub src: PathBuf,
    pub dest_dir: PathBuf,
    pub batch_id: String,
    pub decided_by: DecidedBy,
    pub rule_name: Option<String>,
    pub ai_confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveOutcome {
    Moved {
        log_id: i64,
        src: PathBuf,
        dst: PathBuf,
    },
    /// An identical file already exists at the destination and the policy
    /// is `skip`; the source was left where it is.
    SkippedDuplicate { src: PathBuf, existing: PathBuf },
    /// The file is already in the destination folder.
    AlreadyInPlace(PathBuf),
}

/// Paths the mover is about to create or vacate. The watcher consults it
/// to ignore events caused by our own moves (SPEC §5.1 "expected move").
#[derive(Debug, Default)]
pub struct ExpectedMoves {
    inner: Mutex<HashMap<PathBuf, Instant>>,
}

impl ExpectedMoves {
    const TTL: Duration = Duration::from_secs(10);

    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn expect(&self, path: &Path) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.insert(path.to_path_buf(), now);
        let norm = normalize_file_path(path);
        if norm != path {
            map.insert(norm, now);
        }
    }

    pub fn is_expected(&self, path: &Path) -> bool {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, t| t.elapsed() < Self::TTL);
        map.contains_key(path)
    }
}

pub struct Mover {
    store: Store,
    policy: DuplicatePolicy,
    expected: Arc<ExpectedMoves>,
}

impl Mover {
    pub fn new(store: Store, policy: DuplicatePolicy) -> Self {
        Self {
            store,
            policy,
            expected: ExpectedMoves::new(),
        }
    }

    pub fn with_expected(mut self, expected: Arc<ExpectedMoves>) -> Self {
        self.expected = expected;
        self
    }

    pub fn expected(&self) -> Arc<ExpectedMoves> {
        Arc::clone(&self.expected)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Move `req.src` into `req.dest_dir`, logging it in `move_log`.
    pub fn move_file(&self, req: &MoveRequest) -> Result<MoveOutcome, MoverError> {
        let src = &req.src;
        let meta = fs::symlink_metadata(src).map_err(MoverError::io("stat", src))?;
        if !meta.is_file() {
            return Err(MoverError::NotAFile(src.clone()));
        }
        let name = src
            .file_name()
            .ok_or_else(|| MoverError::NotAFile(src.clone()))?;

        if let Some(parent) = src.parent() {
            if same_dir(parent, &req.dest_dir) {
                return Ok(MoveOutcome::AlreadyInPlace(src.clone()));
            }
        }

        let hash = hash_file(src)?;
        fs::create_dir_all(&req.dest_dir).map_err(MoverError::io("create dir", &req.dest_dir))?;

        let mut dst = req.dest_dir.join(name);
        if dst.exists() {
            if hash_file(&dst)? == hash {
                match self.policy {
                    DuplicatePolicy::Skip => {
                        return Ok(MoveOutcome::SkippedDuplicate {
                            src: src.clone(),
                            existing: dst,
                        })
                    }
                    DuplicatePolicy::Replace => {
                        self.expected.expect(&dst);
                        trash::delete(&dst).map_err(|e| MoverError::Trash {
                            path: dst.clone(),
                            msg: e.to_string(),
                        })?;
                    }
                    DuplicatePolicy::Rename => dst = unique_name(&dst),
                }
            } else {
                dst = unique_name(&dst);
            }
        }

        // Log first, then move; withdraw the row if the move fails.
        let log_id = self.store.insert_move(&NewMove {
            batch_id: req.batch_id.clone(),
            src_path: src.clone(),
            dst_path: dst.clone(),
            hash: Some(hash),
            size: Some(meta.len()),
            decided_by: req.decided_by,
            rule_name: req.rule_name.clone(),
            ai_confidence: req.ai_confidence,
        })?;
        self.expected.expect(src);
        self.expected.expect(&dst);
        if let Err(e) = transfer(src, &dst) {
            self.store.withdraw_move(log_id)?;
            return Err(e);
        }
        tracing::info!(src = %src.display(), dst = %dst.display(), "moved");
        Ok(MoveOutcome::Moved {
            log_id,
            src: src.clone(),
            dst,
        })
    }
}

/// Move one file: `rename` on the same volume, otherwise copy to a temp
/// name next to `dst`, verify size + blake3, rename into place and only then
/// remove the source (SPEC §5.4 step 2). Never overwrites `dst`.
fn transfer(src: &Path, dst: &Path) -> Result<(), MoverError> {
    if dst.exists() {
        return Err(MoverError::DestinationExists(dst.to_path_buf()));
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_verify_move(src, dst),
        Err(e) => Err(MoverError::io("rename", src)(e)),
    }
}

pub(crate) fn copy_verify_move(src: &Path, dst: &Path) -> Result<(), MoverError> {
    let dir = dst.parent().unwrap_or_else(|| Path::new(""));
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}{TMP_MARKER}{}",
        uuid::Uuid::new_v4().simple()
    ));

    let result = (|| {
        let src_meta = fs::metadata(src).map_err(MoverError::io("stat", src))?;
        let src_hash = hash_file(src)?;
        fs::copy(src, &tmp).map_err(MoverError::io("copy", src))?;
        if let Ok(mtime) = src_meta.modified() {
            if let Ok(f) = File::options().write(true).open(&tmp) {
                let _ = f.set_modified(mtime);
            }
        }
        let tmp_len = fs::metadata(&tmp)
            .map_err(MoverError::io("stat", &tmp))?
            .len();
        if tmp_len != src_meta.len() || hash_file(&tmp)? != src_hash {
            return Err(MoverError::VerifyFailed(src.to_path_buf()));
        }
        if dst.exists() {
            return Err(MoverError::DestinationExists(dst.to_path_buf()));
        }
        fs::rename(&tmp, dst).map_err(MoverError::io("rename", &tmp))
    })();

    if let Err(e) = result {
        // Only our own scratch copy is cleaned up; the source is untouched.
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // The verified copy is in place; the original can now go.
    fs::remove_file(src).map_err(MoverError::io("remove source after verified copy", src))
}

/// blake3 hex digest of a file.
pub fn hash_file(path: &Path) -> Result<String, MoverError> {
    let mut f = File::open(path).map_err(MoverError::io("open", path))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(MoverError::io("read", path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// `name.ext` → `name (1).ext`, `name (2).ext`, … (first free one).
fn unique_name(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    (1u32..)
        .map(|i| {
            let name = match &ext {
                Some(e) => format!("{stem} ({i}).{e}"),
                None => format!("{stem} ({i})"),
            };
            dir.join(name)
        })
        .find(|p| !p.exists())
        .expect("unbounded counter")
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (dunce::canonicalize(a), dunce::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Canonicalize the parent folder (if it exists) and re-attach the name.
fn normalize_file_path(p: &Path) -> PathBuf {
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => dunce::canonicalize(parent)
            .map(|d| d.join(name))
            .unwrap_or_else(|_| p.to_path_buf()),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mover() -> Mover {
        Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip)
    }

    fn req(src: &Path, dest_dir: &Path) -> MoveRequest {
        MoveRequest {
            src: src.to_path_buf(),
            dest_dir: dest_dir.to_path_buf(),
            batch_id: "b1".into(),
            decided_by: DecidedBy::Rule,
            rule_name: Some("test".into()),
            ai_confidence: None,
        }
    }

    #[test]
    fn moves_and_logs_korean_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("바탕 화면");
        fs::create_dir(&src_dir).unwrap();
        let src = src_dir.join("회의 메모 (최종).txt");
        fs::write(&src, "내용").unwrap();
        let dest = dir.path().join("정리 된 파일").join("2026");

        let m = mover();
        let out = m.move_file(&req(&src, &dest)).unwrap();
        let MoveOutcome::Moved { log_id, dst, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(dst, dest.join("회의 메모 (최종).txt"));
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "내용");
        let e = m.store().get_move(log_id).unwrap().unwrap();
        assert_eq!(e.src_path, src);
        assert_eq!(e.dst_path, dst);
        assert_eq!(e.size, Some("내용".len() as u64));
        assert!(m.expected().is_expected(&src));
    }

    #[test]
    fn name_clash_with_different_content_gets_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("a b.txt"), "old").unwrap();
        fs::write(dest.join("a b (1).txt"), "older").unwrap();
        let src = dir.path().join("a b.txt");
        fs::write(&src, "new").unwrap();
        let out = mover().move_file(&req(&src, &dest)).unwrap();
        let MoveOutcome::Moved { dst, .. } = out else {
            panic!()
        };
        assert_eq!(dst, dest.join("a b (2).txt"));
        assert_eq!(fs::read_to_string(dest.join("a b.txt")).unwrap(), "old");
    }

    #[test]
    fn identical_duplicate_is_skipped_and_not_logged() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("x.txt"), "same").unwrap();
        let src = dir.path().join("x.txt");
        fs::write(&src, "same").unwrap();
        let m = mover();
        let out = m.move_file(&req(&src, &dest)).unwrap();
        assert!(matches!(out, MoveOutcome::SkippedDuplicate { .. }));
        assert!(src.exists());
        assert!(m.store().recent_moves(10).unwrap().is_empty());
    }

    #[test]
    fn identical_duplicate_rename_policy() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dest");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("x.txt"), "same").unwrap();
        let src = dir.path().join("x.txt");
        fs::write(&src, "same").unwrap();
        let m = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Rename);
        let MoveOutcome::Moved { dst, .. } = m.move_file(&req(&src, &dest)).unwrap() else {
            panic!()
        };
        assert_eq!(dst, dest.join("x (1).txt"));
    }

    #[test]
    fn already_in_place_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.txt");
        fs::write(&src, "x").unwrap();
        let out = mover().move_file(&req(&src, dir.path())).unwrap();
        assert_eq!(out, MoveOutcome::AlreadyInPlace(src.clone()));
        assert!(src.exists());
    }

    #[test]
    fn failed_move_withdraws_log_row() {
        let dir = tempfile::tempdir().unwrap();
        let m = mover();
        let err = m
            .move_file(&req(&dir.path().join("없음.txt"), &dir.path().join("d")))
            .unwrap_err();
        assert!(matches!(err, MoverError::Io { .. }));
        assert!(m.store().recent_moves(10).unwrap().is_empty());
    }

    #[test]
    fn copy_verify_move_preserves_content_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("원본 파일.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &data).unwrap();
        let mtime = fs::metadata(&src).unwrap().modified().unwrap();
        let dst_dir = dir.path().join("다른 볼륨");
        fs::create_dir(&dst_dir).unwrap();
        let dst = dst_dir.join("원본 파일.bin");
        copy_verify_move(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), data);
        assert_eq!(fs::metadata(&dst).unwrap().modified().unwrap(), mtime);
        // No scratch files left behind.
        assert_eq!(fs::read_dir(&dst_dir).unwrap().count(), 1);
    }

    #[test]
    fn transfer_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        assert!(matches!(
            transfer(&a, &b),
            Err(MoverError::DestinationExists(_))
        ));
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
        assert!(a.exists());
    }
}
