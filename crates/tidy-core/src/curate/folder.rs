//! (M12, SPEC §14.7) A sub-folder of the folder being organized, judged
//! and moved as ONE item ("폴더 항목"): a summary for the AI (name, path,
//! file count, extension mix, sample names, inner folders, largest/newest
//! file, the start of a README) and a cache key that changes whenever a
//! file inside changes. Read-only.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, NaiveDate};

use crate::ai::prompts::FolderLine;
use crate::mover::TMP_MARKER;
use crate::watcher::IgnoreFilter;

/// Files whose names feed the summary (samples, README candidates) at
/// most (big toolchains / node_modules). Files past it still count for
/// the file count, size, cache key and settling.
pub const FOLDER_SCAN_CAP: usize = 20_000;
/// Modified times further in the future than this (unpacked archives,
/// a NAS with a wrong clock) are ignored for settling and the folder's
/// date — otherwise the folder would look "still changing" forever.
const FUTURE_SKEW: Duration = Duration::from_secs(120);
const SAMPLES: usize = 10;
const SUBDIRS: usize = 10;
const TOP_EXTS: usize = 5;

#[derive(Debug, Clone)]
pub struct FolderUnit {
    pub path: PathBuf,
    pub name: String,
    /// Relative to the folder being organized ("가온누리원 로그").
    pub rel: String,
    pub files: usize,
    pub bytes: u64,
    /// Most common extensions, ("go", 28).
    pub exts: Vec<(String, usize)>,
    /// Shallowest file names first (relative paths inside the folder).
    pub samples: Vec<String>,
    pub subdirs: Vec<String>,
    pub largest: Option<(String, u64)>,
    /// Newest file (relative path, mtime).
    pub newest: Option<(String, SystemTime)>,
    /// Latest change of anything inside (files and folders) — settling.
    pub last_change: SystemTime,
    /// Start of the first README / .md / .txt (only when asked for).
    pub doc: Option<String>,
    /// `curation.key`: "folder:<blake3 of the file list>|<name>".
    pub key: String,
    /// More than [`FOLDER_SCAN_CAP`] files: samples come from the first ones.
    pub capped: bool,
}

impl FolderUnit {
    /// Summarize `path` (a folder directly in `root`). `doc_chars = 0`
    /// reads no file contents (`ai.send_content` off).
    pub fn scan(
        path: &Path,
        root: &Path,
        filter: &IgnoreFilter,
        doc_chars: usize,
    ) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        if !meta.is_dir() {
            return Err(std::io::Error::other("not a folder"));
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rel = path
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| name.clone());
        let mut u = FolderUnit {
            path: path.to_path_buf(),
            name: name.clone(),
            rel,
            files: 0,
            bytes: 0,
            exts: Vec::new(),
            samples: Vec::new(),
            subdirs: Vec::new(),
            largest: None,
            newest: None,
            last_change: UNIX_EPOCH,
            doc: None,
            key: String::new(),
            capped: false,
        };
        let mut exts: HashMap<String, usize> = HashMap::new();
        // (depth, rel) for samples; (depth, rel) for doc candidates.
        let mut shallow: Vec<(usize, String)> = Vec::new();
        let mut docs: Vec<(usize, String, PathBuf)> = Vec::new();
        // The walk is sorted, so the file list hashes the same every time
        // without keeping it in memory.
        let mut listing = blake3::Hasher::new();
        let horizon = SystemTime::now() + FUTURE_SKEW;
        let plausible = |t: SystemTime| t <= horizon;
        if let Some(t) = meta.modified().ok().filter(|t| plausible(*t)) {
            u.last_change = t;
        }
        for e in walkdir::WalkDir::new(path)
            .min_depth(1)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                let n = e.file_name().to_string_lossy();
                !n.starts_with('.') && !n.contains(TMP_MARKER) && !filter.in_excluded_dir(e.path())
            })
            .filter_map(Result::ok)
        {
            let Ok(m) = e.metadata() else { continue };
            let mtime = m.modified().unwrap_or(UNIX_EPOCH);
            if mtime > u.last_change && plausible(mtime) {
                u.last_change = mtime;
            }
            let rel = e
                .path()
                .strip_prefix(path)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if e.file_type().is_dir() {
                if e.depth() <= 2 && u.subdirs.len() < SUBDIRS {
                    u.subdirs.push(rel);
                }
                continue;
            }
            if !e.file_type().is_file() || filter.is_ignored(e.path()) {
                continue;
            }
            if u.files >= FOLDER_SCAN_CAP {
                u.capped = true;
            }
            u.files += 1;
            u.bytes += m.len();
            let ext = crate::paths::ext_lower(e.path());
            *exts
                .entry(if ext.is_empty() {
                    "(없음)".into()
                } else {
                    ext.clone()
                })
                .or_default() += 1;
            if u.largest.as_ref().is_none_or(|(_, s)| m.len() > *s) {
                u.largest = Some((rel.clone(), m.len()));
            }
            if plausible(mtime) && u.newest.as_ref().is_none_or(|(_, t)| mtime > *t) {
                u.newest = Some((rel.clone(), mtime));
            }
            let secs = mtime
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            listing.update(format!("{rel}|{}|{secs}\n", m.len()).as_bytes());
            if u.capped {
                continue;
            }
            let lower = e.file_name().to_string_lossy().to_lowercase();
            if lower.starts_with("readme") || ext == "md" || ext == "txt" {
                docs.push((e.depth(), rel.clone(), e.path().to_path_buf()));
            }
            shallow.push((e.depth(), rel));
        }
        let mut ex: Vec<(String, usize)> = exts.into_iter().collect();
        ex.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ex.truncate(TOP_EXTS);
        u.exts = ex;
        shallow.sort();
        u.samples = shallow.into_iter().take(SAMPLES).map(|(_, r)| r).collect();
        if doc_chars > 0 {
            // README first, then the shallowest .md / .txt.
            docs.sort_by_key(|(d, r, _)| (!r.to_lowercase().contains("readme"), *d, r.clone()));
            if let Some((_, _, p)) = docs.first() {
                u.doc = read_start(p, doc_chars);
            }
        }
        let digest = listing.finalize();
        u.key = format!("folder:{}|{}", digest.to_hex(), name.to_lowercase());
        Ok(u)
    }

    /// Date of the newest file (the folder item's own date); today when
    /// no file has a believable date.
    pub fn newest_date(&self) -> NaiveDate {
        let t = match self.newest.as_ref() {
            Some((_, t)) => *t,
            None if self.last_change > UNIX_EPOCH => self.last_change,
            None => SystemTime::now(),
        };
        DateTime::<Local>::from(t).date_naive()
    }

    /// Nothing inside changed for `settle` (not being copied / unpacked).
    pub fn is_settled(&self, settle: Duration, now: SystemTime) -> bool {
        now.duration_since(self.last_change)
            .is_ok_and(|d| d >= settle)
    }

    pub fn line(&self) -> FolderLine {
        let date = |t: &SystemTime| DateTime::<Local>::from(*t).format("%Y-%m-%d").to_string();
        FolderLine {
            files: self.files,
            exts: self
                .exts
                .iter()
                .map(|(e, n)| format!("{e} {n}"))
                .collect::<Vec<_>>()
                .join(", "),
            samples: self.samples.clone(),
            subdirs: self.subdirs.clone(),
            largest: self
                .largest
                .as_ref()
                .map(|(r, s)| format!("{r} ({s} B)"))
                .unwrap_or_default(),
            newest: self
                .newest
                .as_ref()
                .map(|(r, t)| format!("{r} {}", date(t)))
                .unwrap_or_default(),
        }
    }
}

/// The first `chars` characters of a text file (reads at most 64 KB).
fn read_start(p: &Path, chars: usize) -> Option<String> {
    let mut buf = Vec::new();
    std::fs::File::open(p)
        .ok()?
        .take(64 * 1024)
        .read_to_end(&mut buf)
        .ok()?;
    let text = String::from_utf8_lossy(&buf);
    let t: String = text.chars().take(chars).collect();
    (!t.trim().is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn summarizes_a_go_tool_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("바탕 화면");
        let unit = root.join("가온누리원 로그");
        let go = unit.join("tools").join("go");
        fs::create_dir_all(go.join("cmd")).unwrap();
        fs::write(
            unit.join("README.md"),
            "가온누리원 선거 로그 분석 도구입니다.\n사용법: go run",
        )
        .unwrap();
        for i in 0..12 {
            fs::write(go.join(format!("f{i:02}.go")), "package main").unwrap();
        }
        fs::write(go.join("cmd").join("big.go"), "x".repeat(5000)).unwrap();
        fs::write(unit.join(".hidden"), "x").unwrap();
        let filter = IgnoreFilter::new(&[]).unwrap();
        let u = FolderUnit::scan(&unit, &root, &filter, 300).unwrap();
        assert_eq!(u.name, "가온누리원 로그");
        assert_eq!(u.rel, "가온누리원 로그");
        assert_eq!(u.files, 14);
        assert_eq!(u.exts[0], ("go".to_string(), 13));
        assert_eq!(u.samples[0], "README.md", "shallowest first");
        assert_eq!(u.samples.len(), 10);
        assert!(
            u.subdirs.contains(&"tools".to_string()) && u.subdirs.contains(&"tools/go".to_string())
        );
        assert_eq!(u.largest.as_ref().unwrap().0, "tools/go/cmd/big.go");
        assert!(u
            .doc
            .as_deref()
            .unwrap()
            .starts_with("가온누리원 선거 로그"));
        assert!(u.key.starts_with("folder:") && u.key.ends_with("|가온누리원 로그"));
        let l = u.line();
        assert!(l.exts.starts_with("go 13"));
        // No contents without send_content.
        assert!(FolderUnit::scan(&unit, &root, &filter, 0)
            .unwrap()
            .doc
            .is_none());
        // A changed file changes the key; settling looks at the newest change.
        let k1 = u.key.clone();
        fs::write(go.join("f00.go"), "package main // changed!").unwrap();
        let u2 = FolderUnit::scan(&unit, &root, &filter, 0).unwrap();
        assert_ne!(u2.key, k1);
        let now = SystemTime::now();
        assert!(!u2.is_settled(Duration::from_secs(60), now));
        assert!(u2.is_settled(Duration::from_secs(60), now + Duration::from_secs(61)));
    }

    /// A file dated in the future (unpacked archive, NAS clock) must not
    /// keep the folder "still changing" forever.
    #[test]
    fn future_dated_files_do_not_block_settling() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("바탕 화면");
        let unit = root.join("미래 날짜");
        fs::create_dir_all(&unit).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        let future = SystemTime::now() + Duration::from_secs(30 * 24 * 3600);
        for (name, t) in [("a.txt", old), ("b.txt", future)] {
            let p = unit.join(name);
            fs::write(&p, name).unwrap();
            fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        let filter = IgnoreFilter::new(&[]).unwrap();
        let u = FolderUnit::scan(&unit, &root, &filter, 0).unwrap();
        assert_eq!(u.files, 2);
        assert_eq!(u.newest.as_ref().unwrap().0, "a.txt", "future date ignored");
        // The folder's own mtime is "now" (files just written), so it
        // settles a minute later — not 30 days later.
        let later = SystemTime::now() + Duration::from_secs(61);
        assert!(u.is_settled(Duration::from_secs(60), later));
    }
}
