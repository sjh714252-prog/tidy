//! Encrypted vault moves (SPEC §14.2, M9): one file → one
//! `<name>.zip` with a single AES-256 (WinZip AE-2) entry.
//!
//! Lock (all inside the mover, logged as `move_log.op = "encrypt"`):
//! 1. hash the original, write the `moving` log row (original hash);
//! 2. write the zip to `.<name>.zip.tidy-tmp.<uuid>` next to the target,
//!    hashing the bytes as they are read (a source that changed since
//!    step 1 fails);
//! 3. reopen the zip, decrypt the entry with the password and compare its
//!    hash with the original's;
//! 4. rename the verified zip into place, record its hash (`dst_hash`);
//! 5. only then remove the original. If that fails, our zip is removed
//!    again and the row withdrawn: the file is never in two places.
//!
//! Undo decrypts back to the original path the same way (scratch file,
//! hash check against the logged original hash, rename) and then removes
//! the zip. A zip whose password is not among the current/previous ones
//! stays put and is reported as a conflict.
//!
//! "Open" decrypts a copy into `<temp>/tidy-open/<id>/`; those copies are
//! the only files Tidy deletes outright ([`sweep_open_copies`]).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime as ChronoDateTime, Datelike, Local, Timelike};
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

use super::transfer::tmp_name;
use super::undo::conflict;
use super::{
    hash_file, normalize_lexical, renamed, unique_name, InFlight, MoveOutcome, MoveRequest, Mover,
    MoverError, UndoReport,
};
use crate::security::VaultKeys;
use crate::store::{MoveLogEntry, MoveOp, NewMove};

const CHUNK: usize = 1024 * 1024;

/// Name of the zip for a file name: `보고서.pdf` → `보고서.pdf.zip`.
pub fn zip_name_for(name: &str) -> String {
    format!("{name}.zip")
}

impl Mover {
    /// `move_file` with `encrypt = true`: lock `req.src` into
    /// `<dest_dir>/<name>.zip`.
    pub(super) fn encrypt_file(&self, req: &MoveRequest) -> Result<MoveOutcome, MoverError> {
        let keys = self.vault_keys()?;
        let password = keys
            .current()
            .map_err(|e| MoverError::Vault(e.to_string()))?;
        let src = &req.src;
        let meta = fs::symlink_metadata(src).map_err(MoverError::io("stat", src))?;
        if !meta.is_file() {
            return Err(MoverError::NotAFile(src.clone()));
        }
        let original = src
            .file_name()
            .ok_or_else(|| MoverError::NotAFile(src.clone()))?;
        let inner: String = match &req.new_name {
            Some(n) => renamed(n, src)?,
            None => original
                .to_str()
                .ok_or_else(|| MoverError::InvalidName(original.to_string_lossy().into()))?
                .to_string(),
        };
        let dest_dir = normalize_lexical(&req.dest_dir);
        let hash = hash_file(src)?;
        fs::create_dir_all(&dest_dir).map_err(MoverError::io("create dir", &dest_dir))?;
        let mut dst = dest_dir.join(zip_name_for(&inner));
        if dst.exists() {
            dst = unique_name(&dst);
        }

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
            op: MoveOp::Encrypt,
        })?;
        self.expected.expect(src);
        self.expected.expect(&dst);
        let locked = (|| {
            write_verified_zip(src, &dst, &inner, password, &hash)?;
            let zip_hash = hash_file(&dst)?;
            self.store.set_dst_hash(log_id, &zip_hash)?;
            Ok::<_, MoverError>(())
        })();
        if let Err(e) = locked {
            if dst.is_file() && InFlight::contains(&dst) {
                let _ = fs::remove_file(&dst);
            }
            self.store.withdraw_move(log_id)?;
            return Err(e);
        }
        // The verified zip is in place; the original can now go.
        if let Err(e) = fs::remove_file(src) {
            // Keep exactly one copy: take our zip back.
            if fs::remove_file(&dst).is_err() {
                tracing::error!(src = %src.display(), zip = %dst.display(),
                    "original could not be removed and neither could the verified zip");
            }
            self.store.withdraw_move(log_id)?;
            return Err(MoverError::io("remove original after verified zip", src)(e));
        }
        self.store.complete_move(log_id)?;
        tracing::info!(src = %src.display(), zip = %dst.display(), "encrypted");
        Ok(MoveOutcome::Moved {
            log_id,
            src: src.clone(),
            dst,
        })
    }

    /// Undo an `encrypt` row: decrypt back to the original path.
    pub(super) fn undo_encrypt(
        &self,
        e: MoveLogEntry,
        report: &mut UndoReport,
    ) -> Result<(), MoverError> {
        let keys = match self.vault_keys() {
            Ok(k) => k,
            Err(err) => {
                report.conflicts.push(conflict(&e, &err.to_string()));
                return Ok(());
            }
        };
        if let Some(parent) = e.src_path.parent() {
            fs::create_dir_all(parent).map_err(MoverError::io("create dir", parent))?;
        }
        self.expected.expect(&e.dst_path);
        self.expected.expect(&e.src_path);
        match restore_from_zip(&e.dst_path, &e.src_path, &keys, e.hash.as_deref()) {
            Ok(()) => {}
            Err(err) => {
                report.conflicts.push(conflict(&e, &err.to_string()));
                return Ok(());
            }
        }
        // The original is back and verified; the zip is no longer needed.
        if let Err(err) = fs::remove_file(&e.dst_path) {
            tracing::warn!(zip = %e.dst_path.display(), "restored, but the zip could not be removed: {err}");
        }
        self.store.mark_undone(e.id)?;
        tracing::info!(from = %e.dst_path.display(), to = %e.src_path.display(), "decrypted (undo)");
        report.restored.push((e.dst_path, e.src_path));
        Ok(())
    }

    /// Recovery of an interrupted encrypt where both the original and the
    /// zip exist (stopped right before the original was removed): keep the
    /// original, drop our zip when the original is unchanged.
    pub(super) fn recover_encrypt_both(
        &self,
        e: &MoveLogEntry,
    ) -> Result<Option<String>, MoverError> {
        let unchanged = e
            .hash
            .as_deref()
            .is_some_and(|h| hash_file(&e.src_path).ok().as_deref() == Some(h));
        if unchanged {
            self.expected.expect(&e.dst_path);
            fs::remove_file(&e.dst_path)
                .map_err(MoverError::io("remove unfinished zip", &e.dst_path))?;
            self.store.withdraw_move(e.id)?;
            Ok(None)
        } else {
            self.store.withdraw_move(e.id)?;
            Ok(Some(format!(
                "{} → {}: 암호화 도중 멈췄고 원본이 그사이 바뀌었습니다 — 둘 다 그대로 둡니다",
                e.src_path.display(),
                e.dst_path.display()
            )))
        }
    }
}

fn zip_datetime(t: SystemTime) -> zip::DateTime {
    let local: ChronoDateTime<Local> = t.into();
    zip::DateTime::from_date_and_time(
        local.year().clamp(1980, 2107) as u16,
        local.month() as u8,
        local.day() as u8,
        local.hour() as u8,
        local.minute() as u8,
        local.second() as u8,
    )
    .unwrap_or_default()
}

/// Write `src` as the only entry `inner` of an AES-256 zip at `dst`
/// (via a scratch file), verify it by decrypting, then rename into place.
fn write_verified_zip(
    src: &Path,
    dst: &Path,
    inner: &str,
    password: &str,
    expected_hash: &str,
) -> Result<(), MoverError> {
    let tmp = tmp_name(dst);
    let r = write_and_verify(src, dst, &tmp, inner, password, expected_hash);
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

fn zip_err(path: &Path) -> impl Fn(zip::result::ZipError) -> MoverError + '_ {
    move |e| MoverError::Io {
        op: "zip",
        path: path.to_path_buf(),
        source: io::Error::other(e.to_string()),
    }
}

fn write_and_verify(
    src: &Path,
    dst: &Path,
    tmp: &Path,
    inner: &str,
    password: &str,
    expected_hash: &str,
) -> Result<(), MoverError> {
    let meta = fs::metadata(src).map_err(MoverError::io("stat", src))?;
    let mtime = meta.modified().ok();
    let out = File::options()
        .write(true)
        .create_new(true)
        .open(tmp)
        .map_err(MoverError::io("create", tmp))?;
    let mut zw = ZipWriter::new(out);
    let mut opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(meta.len() >= u64::from(u32::MAX) - 1024 * 1024)
        .with_aes_encryption(AesMode::Aes256, password);
    if let Some(t) = mtime {
        opts = opts.last_modified_time(zip_datetime(t));
    }
    zw.start_file(inner, opts).map_err(zip_err(tmp))?;
    let mut input = File::open(src).map_err(MoverError::io("open", src))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = input.read(&mut buf).map_err(MoverError::io("read", src))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        zw.write_all(&buf[..n])
            .map_err(MoverError::io("write", tmp))?;
    }
    if hasher.finalize().to_hex().as_str() != expected_hash {
        // Changed after it was logged: do not lock it.
        return Err(MoverError::VerifyFailed(src.to_path_buf()));
    }
    let out = zw.finish().map_err(zip_err(tmp))?;
    out.sync_all().map_err(MoverError::io("flush", tmp))?;
    drop(out);

    // Verify: the only entry, decrypted with the password, is the original.
    let got = decrypt_hash(tmp, password)?;
    if got.0 != inner || got.1 != expected_hash {
        return Err(MoverError::VerifyFailed(src.to_path_buf()));
    }
    if let Some(t) = mtime {
        if let Ok(f) = File::options().write(true).open(tmp) {
            let _ = f.set_modified(t);
        }
    }
    if dst.exists() {
        return Err(MoverError::DestinationExists(dst.to_path_buf()));
    }
    fs::rename(tmp, dst).map_err(MoverError::io("rename", tmp))
}

/// (entry name, blake3 of the decrypted entry) of a one-entry vault zip.
fn decrypt_hash(zip_path: &Path, password: &str) -> Result<(String, String), MoverError> {
    let f = File::open(zip_path).map_err(MoverError::io("open", zip_path))?;
    let mut za = ZipArchive::new(f).map_err(zip_err(zip_path))?;
    if za.len() != 1 {
        return Err(MoverError::Vault(format!(
            "{}: Tidy 보관 파일이 아닙니다 (항목 {}개)",
            zip_path.display(),
            za.len()
        )));
    }
    let mut entry = za
        .by_index_decrypt(0, password.as_bytes())
        .map_err(zip_err(zip_path))?;
    let name = entry.name().to_string();
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = entry
            .read(&mut buf)
            .map_err(MoverError::io("decrypt", zip_path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok((name, hasher.finalize().to_hex().to_string()))
}

/// Which of `keys` opens `zip_path` (`None`: none of them).
fn find_password<'k>(zip_path: &Path, keys: &'k VaultKeys) -> Result<Option<&'k str>, MoverError> {
    let f = File::open(zip_path).map_err(MoverError::io("open", zip_path))?;
    let mut za = ZipArchive::new(f).map_err(zip_err(zip_path))?;
    if za.is_empty() {
        return Ok(None);
    }
    for pw in keys.candidates() {
        if za.by_index_decrypt(0, pw.as_bytes()).is_ok() {
            return Ok(Some(pw));
        }
    }
    Ok(None)
}

/// Decrypt the entry of `zip_path` to `target` (never overwritten) through
/// a scratch file; with `expected_hash`, the bytes must match it.
fn restore_from_zip(
    zip_path: &Path,
    target: &Path,
    keys: &VaultKeys,
    expected_hash: Option<&str>,
) -> Result<(), MoverError> {
    if !zip_path.is_file() {
        return Err(MoverError::Vault(
            "보관 파일(zip)이 더 이상 그 자리에 없습니다".into(),
        ));
    }
    if target.exists() {
        return Err(MoverError::Vault(
            "원래 위치에 다른 파일이 생겼습니다".into(),
        ));
    }
    let password = find_password(zip_path, keys)?.ok_or_else(|| {
        MoverError::Vault(
            "보관 비밀번호(현재·이전) 중 이 파일을 여는 것이 없습니다 — zip은 그대로 둡니다".into(),
        )
    })?;
    let tmp = tmp_name(target);
    let r = (|| {
        let f = File::open(zip_path).map_err(MoverError::io("open", zip_path))?;
        let mut za = ZipArchive::new(f).map_err(zip_err(zip_path))?;
        let mut entry = za
            .by_index_decrypt(0, password.as_bytes())
            .map_err(zip_err(zip_path))?;
        let mut out = File::options()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(MoverError::io("create", &tmp))?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(MoverError::io("decrypt", zip_path))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(MoverError::io("write", &tmp))?;
        }
        out.sync_all().map_err(MoverError::io("flush", &tmp))?;
        drop(out);
        if expected_hash.is_some_and(|h| h != hasher.finalize().to_hex().as_str()) {
            return Err(MoverError::VerifyFailed(zip_path.to_path_buf()));
        }
        // The zip keeps the original's mtime.
        if let Ok(t) = fs::metadata(zip_path).and_then(|m| m.modified()) {
            if let Ok(f) = File::options().write(true).open(&tmp) {
                let _ = f.set_modified(t);
            }
        }
        if target.exists() {
            return Err(MoverError::DestinationExists(target.to_path_buf()));
        }
        fs::rename(&tmp, target).map_err(MoverError::io("rename", &tmp))
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// Folder for decrypted "open" copies: `<temp>/tidy-open`.
pub fn open_dir() -> PathBuf {
    std::env::temp_dir().join("tidy-open")
}

/// Decrypt a vault zip into a fresh folder under [`open_dir`] for viewing
/// and return the copy's path. The zip is not changed and nothing is
/// logged (no file is moved); [`sweep_open_copies`] removes the copy.
pub fn open_temp_copy(zip_path: &Path, keys: &VaultKeys) -> Result<PathBuf, MoverError> {
    let password = find_password(zip_path, keys)?.ok_or_else(|| {
        MoverError::Vault("보관 비밀번호(현재·이전) 중 이 파일을 여는 것이 없습니다".into())
    })?;
    let f = File::open(zip_path).map_err(MoverError::io("open", zip_path))?;
    let mut za = ZipArchive::new(f).map_err(zip_err(zip_path))?;
    let mut entry = za
        .by_index_decrypt(0, password.as_bytes())
        .map_err(zip_err(zip_path))?;
    // Only the plain file name of the entry (never a path from the zip).
    let name = Path::new(entry.name())
        .file_name()
        .map(|n| n.to_os_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "열기".into());
    let dir = open_dir().join(uuid::Uuid::new_v4().simple().to_string());
    fs::create_dir_all(&dir).map_err(MoverError::io("create dir", &dir))?;
    let out_path = dir.join(name);
    let mut out = File::create_new(&out_path).map_err(MoverError::io("create", &out_path))?;
    io::copy(&mut entry, &mut out).map_err(MoverError::io("decrypt", zip_path))?;
    out.sync_all().map_err(MoverError::io("flush", &out_path))?;
    Ok(out_path)
}

/// Remove decrypted copies under [`open_dir`] older than `max_age`
/// (SPEC §14.2: the only deletion Tidy does, temporary copies only).
/// Returns how many folders were removed.
pub fn sweep_open_copies(max_age: Duration) -> usize {
    sweep_in(&open_dir(), max_age)
}

fn sweep_in(dir: &Path, max_age: Duration) -> usize {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for d in rd.filter_map(Result::ok) {
        let p = d.path();
        // Only our own `<uuid>` folders.
        let ours = d
            .file_name()
            .to_str()
            .is_some_and(|s| s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()));
        if !ours || !p.is_dir() {
            continue;
        }
        let old = d
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= max_age);
        if old && fs::remove_dir_all(&p).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DuplicatePolicy;
    use crate::store::{DecidedBy, Store};

    const PW: &str = "테스트비밀번호1234";

    fn mover(keys: VaultKeys) -> Mover {
        Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip).with_vault_keys(keys)
    }

    fn req(src: &Path, dest: &Path) -> MoveRequest {
        MoveRequest {
            src: src.to_path_buf(),
            dest_dir: dest.to_path_buf(),
            batch_id: "b".into(),
            decided_by: DecidedBy::Rule,
            rule_name: Some("개인정보".into()),
            ai_confidence: None,
            new_name: None,
            encrypt: true,
            on_duplicate: None,
        }
    }

    fn read_entry(zip: &Path, pw: &str) -> (String, Vec<u8>) {
        let mut za = ZipArchive::new(File::open(zip).unwrap()).unwrap();
        let mut e = za.by_index_decrypt(0, pw.as_bytes()).unwrap();
        let mut v = Vec::new();
        e.read_to_end(&mut v).unwrap();
        (e.name().to_string(), v)
    }

    #[test]
    fn lock_verifies_logs_and_undo_restores() {
        let dir = tempfile::tempdir().unwrap();
        let desk = dir.path().join("바탕 화면");
        fs::create_dir(&desk).unwrap();
        let src = desk.join("직원 명단.csv");
        let body = "이름,휴대전화\n홍길동,010-1234-5678\n".repeat(200);
        fs::write(&src, &body).unwrap();
        let mtime = fs::metadata(&src).unwrap().modified().unwrap();
        let vault = dir.path().join("정리").join("보안");

        let m = mover(VaultKeys::new(PW));
        let MoveOutcome::Moved { log_id, dst, .. } = m.move_file(&req(&src, &vault)).unwrap()
        else {
            panic!()
        };
        assert_eq!(dst, vault.join("직원 명단.csv.zip"));
        assert!(!src.exists(), "original removed after verification");
        let (name, bytes) = read_entry(&dst, PW);
        assert_eq!(name, "직원 명단.csv");
        assert_eq!(bytes, body.as_bytes());
        // Wrong password does not open it.
        let mut za = ZipArchive::new(File::open(&dst).unwrap()).unwrap();
        assert!(za.by_index_decrypt(0, b"wrong-password").is_err());
        assert_eq!(fs::metadata(&dst).unwrap().modified().unwrap(), mtime);
        // Only the zip, no scratch files.
        assert_eq!(fs::read_dir(&vault).unwrap().count(), 1);

        let e = m.store().get_move(log_id).unwrap().unwrap();
        assert_eq!(e.op, MoveOp::Encrypt);
        assert_eq!(
            e.hash.as_deref(),
            Some(blake3::hash(body.as_bytes()).to_hex().as_str())
        );
        assert_eq!(e.dst_hash, Some(hash_file(&dst).unwrap()));
        // The engine sees the zip as "placed by Tidy".
        assert_eq!(m.store().placed_hash(&dst).unwrap(), e.dst_hash);

        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.restored.len(), 1, "{:?}", r.conflicts);
        assert_eq!(fs::read_to_string(&src).unwrap(), body);
        assert!(!dst.exists());
        assert_eq!(fs::metadata(&src).unwrap().modified().unwrap(), mtime);
    }

    #[test]
    fn undo_uses_previous_password_and_reports_unknown_one() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("계좌.txt");
        fs::write(&src, "입금 계좌 123456-12-345678").unwrap();
        let m = mover(VaultKeys::new("옛 비밀번호 1234"));
        let MoveOutcome::Moved { dst, .. } = m.move_file(&req(&src, dir.path())).unwrap() else {
            panic!()
        };
        // Password changed since, and the old one was not kept.
        let mut m = m;
        m.set_vault_keys(VaultKeys::new("다른 비밀번호 1"));
        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.conflicts.len(), 1);
        assert!(
            r.conflicts[0].reason.contains("비밀번호"),
            "{}",
            r.conflicts[0].reason
        );
        assert!(dst.exists() && !src.exists(), "zip left as is");
        // With the old one in the "previous" list it opens.
        m.set_vault_keys(VaultKeys::new("다른 비밀번호 1").with_previous(&["옛 비밀번호 1234"]));
        let r = m.undo_last().unwrap().unwrap();
        assert_eq!(r.restored.len(), 1);
        assert!(src.exists() && !dst.exists());
    }

    #[test]
    fn no_password_means_no_lock_and_no_log() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, "x").unwrap();
        let m = mover(VaultKeys::none());
        assert!(matches!(
            m.move_file(&req(&src, &dir.path().join("v"))),
            Err(MoverError::Vault(_))
        ));
        assert!(src.exists());
        assert!(m.store().recent_moves(5).unwrap().is_empty());
    }

    #[test]
    fn name_clash_gets_suffix_and_existing_zip_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("v");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("a.txt.zip"), "다른 파일").unwrap();
        let src = dir.path().join("a.txt");
        fs::write(&src, "새 내용").unwrap();
        let m = mover(VaultKeys::new(PW));
        let MoveOutcome::Moved { dst, .. } = m.move_file(&req(&src, &vault)).unwrap() else {
            panic!()
        };
        assert_eq!(dst, vault.join("a.txt (1).zip"));
        assert_eq!(
            fs::read_to_string(vault.join("a.txt.zip")).unwrap(),
            "다른 파일"
        );
        assert_eq!(read_entry(&dst, PW).0, "a.txt");
    }

    #[test]
    fn interrupted_lock_keeps_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("명단.txt");
        fs::write(&src, "홍길동 010-1234-5678").unwrap();
        let dst = dir.path().join("v").join("명단.txt.zip");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let m = mover(VaultKeys::new(PW));
        let hash = hash_file(&src).unwrap();
        // Stopped after the verified zip was renamed into place, before the
        // original was removed; plus a leftover scratch file.
        write_verified_zip(&src, &dst, "명단.txt", PW, &hash).unwrap();
        let stale = tmp_name(&dst);
        fs::write(&stale, "half").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        m.store()
            .begin_move(&NewMove {
                batch_id: "b".into(),
                src_path: src.clone(),
                dst_path: dst.clone(),
                hash: Some(hash),
                size: None,
                decided_by: DecidedBy::Rule,
                rule_name: None,
                ai_confidence: None,
                op: MoveOp::Encrypt,
            })
            .unwrap();
        let r = m.recover_interrupted(Duration::ZERO).unwrap();
        assert_eq!(r.rolled_back, vec![src.clone()]);
        assert_eq!(r.tmp_removed, 1);
        assert!(src.exists() && !dst.exists() && !stale.exists());
        assert!(m.store().recent_moves(5).unwrap().is_empty());
    }

    #[test]
    fn open_copy_and_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("명단.txt");
        fs::write(&src, "홍길동 010-1234-5678").unwrap();
        let m = mover(VaultKeys::new(PW));
        let MoveOutcome::Moved { dst, .. } = m.move_file(&req(&src, dir.path())).unwrap() else {
            panic!()
        };
        let copy = open_temp_copy(&dst, &VaultKeys::new(PW)).unwrap();
        assert_eq!(fs::read_to_string(&copy).unwrap(), "홍길동 010-1234-5678");
        assert!(copy.starts_with(open_dir()));
        assert!(open_temp_copy(&dst, &VaultKeys::new("틀린 비밀번호")).is_err());

        // Sweep removes only our own old folders.
        let base = dir.path().join("open");
        let ours = base.join("0123456789abcdef0123456789abcdef");
        let other = base.join("사용자 폴더");
        fs::create_dir_all(&ours).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(ours.join("x.txt"), "x").unwrap();
        assert_eq!(sweep_in(&base, Duration::from_secs(3600)), 0, "too new");
        assert_eq!(sweep_in(&base, Duration::ZERO), 1);
        assert!(!ours.exists() && other.exists());
        let _ = fs::remove_dir_all(copy.parent().unwrap());
    }
}
