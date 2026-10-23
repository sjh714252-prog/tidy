//! Mover + Undo (SPEC §5.4).
//!
//! This is the only module allowed to change the filesystem. Every move is
//! written to `move_log` *before* the file is touched (and withdrawn if the
//! move fails), so there is never a move without an undo record.
//! Nothing here deletes user data: the only removals are
//! - the source of a cross-volume move, after the copy was size+hash verified,
//! - our own `.tidy-tmp.*` scratch copies (also leftovers of an interrupted
//!   copy, see [`Mover::recover_interrupted`]),
//! - our own verified copy at the destination when the source could not be
//!   removed (the file stays at its source, so it is never in two places),
//! - the source of an interrupted move whose verified copy is already in
//!   place with the same logged hash (finishing that move),
//! - `duplicate_policy = "replace"`, which sends the identical existing copy
//!   to the OS trash (`trash` crate), never `remove_file`.
//! - (M9) the original of an encrypt, after its zip was decrypted and
//!   hash-verified; the zip of an undone encrypt, after the original was
//!   restored and verified; our own unfinished zip when recovery keeps
//!   the unchanged original; decrypted "open" copies under
//!   `<temp>/tidy-open` (see `vault.rs`).

mod dir;
mod recover;
mod transfer;
mod undo;
mod vault;

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

pub use dir::{DirMoveReport, DirMoveRequest};
pub use recover::{RecoveryReport, DEFAULT_RECOVERY_GRACE};
pub use transfer::{ProgressFn, TransferProgress};
pub use undo::{UndoConflict, UndoReport};
pub use vault::{open_dir, open_temp_copy, sweep_open_copies, zip_name_for};

use transfer::{transfer, TransferOpts};

use crate::config::DuplicatePolicy;
use crate::paths::normalize_lexical;
use crate::security::VaultKeys;
use crate::store::{DecidedBy, MoveOp, NewMove, Store, StoreError};

/// Create an empty folder (and missing parents) for a rule destination
/// picked in the folder tree ("새 폴더", SPEC §14.1). Kept here because
/// every filesystem change goes through the mover (CLAUDE.md rule 1). It
/// moves no file, so there is nothing to log; an existing folder is fine.
/// The name must be a single plain component (no separators, `..`).
pub fn create_dir(parent: &Path, name: &str) -> io::Result<PathBuf> {
    let name = name.trim();
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control());
    if bad {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("폴더 이름으로 쓸 수 없습니다: {name}"),
        ));
    }
    if !parent.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("상위 폴더가 없습니다: {}", parent.display()),
        ));
    }
    let dir = parent.join(name);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

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
    #[error("not a folder: {0}")]
    NotAFolder(PathBuf),
    #[error("cannot move {src} into itself ({dest})")]
    DestinationInside { src: PathBuf, dest: PathBuf },
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("copy verification failed for {0} (size or hash mismatch); source left untouched")]
    VerifyFailed(PathBuf),
    #[error("invalid new file name: {0:?}")]
    InvalidName(String),
    #[error("could not send {path} to trash: {msg}")]
    Trash { path: PathBuf, msg: String },
    /// (M9) No vault password, wrong password, not a Tidy zip.
    #[error("{0}")]
    Vault(String),
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
    /// Give the file a new name (confirm popup "이름 바꿔 이동"). A plain
    /// name without separators; the original extension is kept when the
    /// new name has none.
    pub new_name: Option<String>,
    /// (M9) Lock the file into `<dest_dir>/<name>.zip` (AES-256) instead
    /// of moving it as is.
    pub encrypt: bool,
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

/// Destinations of transfers running in this process (recovery must not
/// mistake them for interrupted ones).
static IN_FLIGHT: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

struct InFlight(PathBuf);

impl InFlight {
    fn start(dst: &Path) -> Self {
        let mut g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashSet::new).insert(dst.to_path_buf());
        Self(dst.to_path_buf())
    }

    fn contains(dst: &Path) -> bool {
        let g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().is_some_and(|s| s.contains(dst))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut g = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_mut() {
            s.remove(&self.0);
        }
    }
}

/// Where the mover gets vault passwords (M9).
#[derive(Debug, Clone)]
enum VaultSource {
    /// Read the OS keychain each time (a changed password applies at once).
    Keychain,
    Fixed(VaultKeys),
}

pub struct Mover {
    store: Store,
    policy: DuplicatePolicy,
    expected: Arc<ExpectedMoves>,
    transfer: TransferOpts,
    vault: VaultSource,
}

impl Mover {
    pub fn new(store: Store, policy: DuplicatePolicy) -> Self {
        Self {
            store,
            policy,
            expected: ExpectedMoves::new(),
            transfer: TransferOpts::default(),
            vault: VaultSource::Keychain,
        }
    }

    /// Use these vault passwords instead of the keychain (tests, CLI with
    /// an explicit password).
    pub fn with_vault_keys(mut self, keys: VaultKeys) -> Self {
        self.vault = VaultSource::Fixed(keys);
        self
    }

    pub fn set_vault_keys(&mut self, keys: VaultKeys) {
        self.vault = VaultSource::Fixed(keys);
    }

    /// Current + previous vault passwords.
    pub fn vault_keys(&self) -> Result<VaultKeys, MoverError> {
        match &self.vault {
            VaultSource::Fixed(k) => Ok(k.clone()),
            VaultSource::Keychain => {
                VaultKeys::from_keychain().map_err(|e| MoverError::Vault(e.to_string()))
            }
        }
    }

    /// Report the progress of long copy-verify transfers (NAS moves).
    pub fn with_progress(mut self, f: ProgressFn) -> Self {
        self.transfer.progress = Some(f);
        self
    }

    /// Always copy + verify instead of `rename`, even on one volume.
    /// For tests and for shares whose server-side rename is unreliable.
    pub fn copy_always(mut self, on: bool) -> Self {
        self.transfer.copy_always = on;
        self
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
        if req.encrypt {
            return self.encrypt_file(req);
        }
        let src = &req.src;
        let meta = fs::symlink_metadata(src).map_err(MoverError::io("stat", src))?;
        if !meta.is_file() {
            return Err(MoverError::NotAFile(src.clone()));
        }
        let original = src
            .file_name()
            .ok_or_else(|| MoverError::NotAFile(src.clone()))?;
        let name: std::ffi::OsString = match &req.new_name {
            Some(n) => renamed(n, src)?.into(),
            None => original.to_os_string(),
        };

        // Whatever the caller passes, the logged paths never contain `..`.
        let dest_dir = normalize_lexical(&req.dest_dir);
        if let Some(parent) = src.parent() {
            if same_dir(parent, &dest_dir) && name.as_os_str() == original {
                return Ok(MoveOutcome::AlreadyInPlace(src.clone()));
            }
        }

        let hash = hash_file(src)?;
        fs::create_dir_all(&dest_dir).map_err(MoverError::io("create dir", &dest_dir))?;

        let mut dst = dest_dir.join(&name);
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

        // Log first ('moving'), then move, then mark it done; withdraw the
        // row if the move fails. A crash in between is found by
        // `recover_interrupted`.
        let _flight = InFlight::start(&dst);
        let log_id = self.store.begin_move(&NewMove {
            batch_id: req.batch_id.clone(),
            src_path: src.clone(),
            dst_path: dst.clone(),
            hash: Some(hash.clone()),
            size: Some(meta.len()),
            decided_by: req.decided_by,
            rule_name: req.rule_name.clone(),
            ai_confidence: req.ai_confidence,
            op: MoveOp::Move,
        })?;
        self.expected.expect(src);
        self.expected.expect(&dst);
        if let Err(e) = transfer(src, &dst, Some(&hash), &self.transfer) {
            self.store.withdraw_move(log_id)?;
            return Err(e);
        }
        self.store.complete_move(log_id)?;
        tracing::info!(src = %src.display(), dst = %dst.display(), "moved");
        Ok(MoveOutcome::Moved {
            log_id,
            src: src.clone(),
            dst,
        })
    }
}

/// Validate a user/AI-supplied new name; keep the original extension when
/// the new name has none ("보고서" + `a.hwp` → "보고서.hwp").
fn renamed(new_name: &str, src: &Path) -> Result<String, MoverError> {
    let n = new_name.trim();
    if n.is_empty()
        || n == "."
        || n == ".."
        || n.contains(['/', '\\', ':', '\0'])
        || n.chars().count() > 200
    {
        return Err(MoverError::InvalidName(new_name.to_string()));
    }
    Ok(match (Path::new(n).extension(), src.extension()) {
        (None, Some(ext)) => format!("{n}.{}", ext.to_string_lossy()),
        _ => n.to_string(),
    })
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

    #[test]
    fn create_dir_makes_one_plain_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let d = create_dir(tmp.path(), " 새 폴더 ").unwrap();
        assert_eq!(d, tmp.path().join("새 폴더"));
        assert!(d.is_dir());
        assert!(
            create_dir(tmp.path(), "새 폴더").is_ok(),
            "existing is fine"
        );
        for bad in ["", "..", "a/b", "a\\b", "x:y"] {
            assert!(create_dir(tmp.path(), bad).is_err(), "{bad}");
        }
        assert!(create_dir(&tmp.path().join("없음"), "x").is_err());
    }

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
            new_name: None,
            encrypt: false,
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
    fn dest_with_dot_dot_is_logged_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("받은 파일");
        fs::create_dir(&inbox).unwrap();
        let src = inbox.join("메모.txt");
        fs::write(&src, "x").unwrap();
        let dest = inbox.join("..").join("정리 결과").join("2026");
        let m = mover();
        let MoveOutcome::Moved { log_id, dst, .. } = m.move_file(&req(&src, &dest)).unwrap() else {
            panic!()
        };
        assert_eq!(
            dst,
            dir.path().join("정리 결과").join("2026").join("메모.txt")
        );
        let e = m.store().get_move(log_id).unwrap().unwrap();
        assert!(!e.dst_path.to_string_lossy().contains(".."));
        assert_eq!(e.dst_path, dst);
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
    fn rename_move_keeps_extension_and_rejects_paths() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("scan0001.pdf");
        fs::write(&src, "x").unwrap();
        let m = mover();
        let mut r = req(&src, &dir.path().join("영수증"));
        r.new_name = Some("2026-10 택시 영수증".into());
        let MoveOutcome::Moved { dst, .. } = m.move_file(&r).unwrap() else {
            panic!()
        };
        assert_eq!(
            dst,
            dir.path().join("영수증").join("2026-10 택시 영수증.pdf")
        );

        let other = dir.path().join("b.txt");
        fs::write(&other, "y").unwrap();
        let mut bad = req(&other, dir.path());
        bad.new_name = Some("../탈출.txt".into());
        assert!(matches!(m.move_file(&bad), Err(MoverError::InvalidName(_))));
        // Renaming in place is a move too (logged, undoable).
        let mut inplace = req(&other, dir.path());
        inplace.new_name = Some("새 이름.txt".into());
        assert!(matches!(
            m.move_file(&inplace).unwrap(),
            MoveOutcome::Moved { .. }
        ));
    }

    #[test]
    fn transfer_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        assert!(matches!(
            transfer(&a, &b, None, &TransferOpts::default()),
            Err(MoverError::DestinationExists(_))
        ));
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
        assert!(a.exists());
    }

    #[test]
    fn copy_always_moves_are_logged_done_and_undoable() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("바탕 화면").join("큰 파일.bin");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, vec![7u8; 2_000_000]).unwrap();
        let nas = dir.path().join("NAS 공유").join("보관");
        let m = mover().copy_always(true);
        let MoveOutcome::Moved { log_id, dst, .. } = m.move_file(&req(&src, &nas)).unwrap() else {
            panic!()
        };
        let e = m.store().get_move(log_id).unwrap().unwrap();
        assert!(!e.in_progress);
        assert!(!InFlight::contains(&dst));
        assert_eq!(
            m.store().placed_hash(&dst).unwrap(),
            Some(hash_file(&dst).unwrap())
        );
        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.restored.len(), 1);
        assert_eq!(fs::read(&src).unwrap().len(), 2_000_000);
        assert_eq!(fs::read_dir(&nas).unwrap().count(), 0);
        assert_eq!(m.store().placed_hash(&dst).unwrap(), None);
    }
}
