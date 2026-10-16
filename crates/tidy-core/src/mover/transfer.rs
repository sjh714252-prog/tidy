//! Moving the bytes of one file (SPEC §5.4 step 2, §5.9).
//!
//! Same volume: `rename`. Another volume or a NAS share: copy to
//! `.<name>.tidy-tmp.<uuid>` next to the destination while hashing, fsync,
//! re-read the copy and compare size + blake3, give it the original mtime,
//! rename it into place, and only then remove the source. If the source
//! cannot be removed, our own verified copy is removed instead, so the file
//! is never left in two places without a record.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::{hash_file, MoverError, TMP_MARKER};

/// Progress of one copy-verify transfer (only long copies report).
#[derive(Debug, Clone, Serialize)]
pub struct TransferProgress {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub copied: u64,
    pub total: u64,
    /// "copy" while copying, "verify" while re-reading the copy, "done".
    pub phase: &'static str,
}

pub type ProgressFn = Arc<dyn Fn(&TransferProgress) + Send + Sync>;

/// How often a running copy reports (also the delay before the first
/// report, so quick copies stay silent).
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const CHUNK: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct TransferOpts {
    /// Copy + verify even when `rename` would work (tests, flaky shares).
    pub copy_always: bool,
    pub progress: Option<ProgressFn>,
}

/// Move `src` to `dst`. Never overwrites `dst`. `expected_hash` is the hash
/// recorded in `move_log`; a copy whose source no longer matches it fails.
pub(crate) fn transfer(
    src: &Path,
    dst: &Path,
    expected_hash: Option<&str>,
    opts: &TransferOpts,
) -> Result<(), MoverError> {
    if dst.exists() {
        return Err(MoverError::DestinationExists(dst.to_path_buf()));
    }
    if !opts.copy_always {
        match fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) if is_cross_device(&e) => {}
            Err(e) => return Err(MoverError::io("rename", src)(e)),
        }
    }
    copy_verify_move(src, dst, expected_hash, opts.progress.as_ref())
}

fn is_cross_device(e: &io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE (17) on Windows; EXDEV elsewhere.
    e.kind() == io::ErrorKind::CrossesDevices || (cfg!(windows) && e.raw_os_error() == Some(17))
}

/// Scratch copy name for `dst`: `.<name>.tidy-tmp.<uuid>` (the watcher and
/// the indexer ignore it; recovery cleans up leftovers).
pub(crate) fn tmp_name(dst: &Path) -> PathBuf {
    let dir = dst.parent().unwrap_or_else(|| Path::new(""));
    dir.join(format!(
        "{}{}",
        tmp_prefix(dst),
        uuid::Uuid::new_v4().simple()
    ))
}

/// `.<name>.tidy-tmp.` — what every scratch copy for `dst` starts with.
pub(crate) fn tmp_prefix(dst: &Path) -> String {
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!(".{name}{TMP_MARKER}")
}

pub(crate) fn copy_verify_move(
    src: &Path,
    dst: &Path,
    expected_hash: Option<&str>,
    progress: Option<&ProgressFn>,
) -> Result<(), MoverError> {
    let _slot = crate::netio::network_io().acquire();
    let tmp = tmp_name(dst);
    let result = copy_and_verify(src, dst, &tmp, expected_hash, progress);
    if let Err(e) = result {
        // Only our own scratch copy is cleaned up; the source is untouched.
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // The verified copy is in place; the original can now go.
    if let Err(e) = fs::remove_file(src) {
        // Keep exactly one copy: take ours back (it is identical to the
        // source, which is still there) and report the failure.
        if fs::remove_file(dst).is_err() {
            tracing::error!(src = %src.display(), dst = %dst.display(),
                "source could not be removed and neither could the verified copy");
        }
        return Err(MoverError::io("remove source after verified copy", src)(e));
    }
    Ok(())
}

fn copy_and_verify(
    src: &Path,
    dst: &Path,
    tmp: &Path,
    expected_hash: Option<&str>,
    progress: Option<&ProgressFn>,
) -> Result<(), MoverError> {
    let src_meta = fs::metadata(src).map_err(MoverError::io("stat", src))?;
    let total = src_meta.len();
    let mut reporter = Reporter::new(src, dst, total, progress);

    let mut input = File::open(src).map_err(MoverError::io("open", src))?;
    let mut out = File::options()
        .write(true)
        .create_new(true)
        .open(tmp)
        .map_err(MoverError::io("create", tmp))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut copied = 0u64;
    loop {
        let n = input.read(&mut buf).map_err(MoverError::io("read", src))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])
            .map_err(MoverError::io("write", tmp))?;
        copied += n as u64;
        reporter.tick(copied, "copy");
    }
    out.sync_all().map_err(MoverError::io("flush", tmp))?;
    drop(out);
    let sent = hasher.finalize().to_hex().to_string();

    // The source changed after it was logged/hashed: do not move it.
    if expected_hash.is_some_and(|h| h != sent) || copied != total {
        return Err(MoverError::VerifyFailed(src.to_path_buf()));
    }
    reporter.tick(total, "verify");
    let tmp_len = fs::metadata(tmp)
        .map_err(MoverError::io("stat", tmp))?
        .len();
    if tmp_len != total || hash_file(tmp)? != sent {
        return Err(MoverError::VerifyFailed(src.to_path_buf()));
    }
    // mtime is set only now, so a scratch copy that is still being written
    // always looks fresh to recovery.
    if let Ok(mtime) = src_meta.modified() {
        if let Ok(f) = File::options().write(true).open(tmp) {
            let _ = f.set_modified(mtime);
        }
    }
    if dst.exists() {
        return Err(MoverError::DestinationExists(dst.to_path_buf()));
    }
    fs::rename(tmp, dst).map_err(MoverError::io("rename", tmp))?;
    reporter.finish(total);
    Ok(())
}

struct Reporter<'a> {
    src: &'a Path,
    dst: &'a Path,
    total: u64,
    f: Option<&'a ProgressFn>,
    started: Instant,
    last: Option<Instant>,
}

impl<'a> Reporter<'a> {
    fn new(src: &'a Path, dst: &'a Path, total: u64, f: Option<&'a ProgressFn>) -> Self {
        Self {
            src,
            dst,
            total,
            f,
            started: Instant::now(),
            last: None,
        }
    }

    fn emit(&self, copied: u64, phase: &'static str) {
        if let Some(f) = self.f {
            f(&TransferProgress {
                src: self.src.to_path_buf(),
                dst: self.dst.to_path_buf(),
                copied,
                total: self.total,
                phase,
            });
        }
    }

    fn tick(&mut self, copied: u64, phase: &'static str) {
        let due = match self.last {
            None => self.started.elapsed() >= PROGRESS_EVERY,
            Some(t) => t.elapsed() >= PROGRESS_EVERY,
        };
        if self.f.is_some() && due {
            self.last = Some(Instant::now());
            self.emit(copied, phase);
        }
    }

    /// Final report, only if the copy reported at all.
    fn finish(&self, total: u64) {
        if self.last.is_some() {
            self.emit(total, "done");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn copy_verify_move_preserves_content_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("원본 파일.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &data).unwrap();
        let mtime = fs::metadata(&src).unwrap().modified().unwrap();
        let hash = hash_file(&src).unwrap();
        let dst_dir = dir.path().join("다른 볼륨");
        fs::create_dir(&dst_dir).unwrap();
        let dst = dst_dir.join("원본 파일.bin");
        copy_verify_move(&src, &dst, Some(&hash), None).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), data);
        assert_eq!(fs::metadata(&dst).unwrap().modified().unwrap(), mtime);
        // No scratch files left behind.
        assert_eq!(fs::read_dir(&dst_dir).unwrap().count(), 1);
    }

    #[test]
    fn changed_source_is_not_moved_and_scratch_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, "새 내용").unwrap();
        let dst_dir = dir.path().join("nas");
        fs::create_dir(&dst_dir).unwrap();
        let err =
            copy_verify_move(&src, &dst_dir.join("a.txt"), Some("옛 해시"), None).unwrap_err();
        assert!(matches!(err, MoverError::VerifyFailed(_)));
        assert_eq!(fs::read_to_string(&src).unwrap(), "새 내용");
        assert_eq!(fs::read_dir(&dst_dir).unwrap().count(), 0);
    }

    #[test]
    fn missing_destination_folder_fails_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, "x").unwrap();
        // The share went away: the destination folder is gone.
        let err = copy_verify_move(&src, &dir.path().join("끊긴 NAS").join("a.txt"), None, None)
            .unwrap_err();
        assert!(matches!(err, MoverError::Io { op: "create", .. }));
        assert!(src.exists());
    }

    #[test]
    fn reports_progress_for_slow_copies_only() {
        let seen: Arc<Mutex<Vec<(u64, &'static str)>>> = Arc::default();
        let s2 = seen.clone();
        let f: ProgressFn = Arc::new(move |p| s2.lock().unwrap().push((p.copied, p.phase)));
        let (a, b) = (Path::new("a"), Path::new("b"));
        let mut r = Reporter::new(a, b, 100, Some(&f));
        r.tick(10, "copy"); // too early
        r.finish(100);
        assert!(seen.lock().unwrap().is_empty());
        r.started -= PROGRESS_EVERY;
        r.tick(50, "copy");
        r.tick(60, "copy"); // throttled
        r.finish(100);
        assert_eq!(*seen.lock().unwrap(), vec![(50, "copy"), (100, "done")]);
    }

    #[test]
    fn copy_always_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let opts = TransferOpts {
            copy_always: true,
            progress: None,
        };
        assert!(matches!(
            transfer(&a, &b, None, &opts),
            Err(MoverError::DestinationExists(_))
        ));
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
        assert!(a.exists());
    }
}
