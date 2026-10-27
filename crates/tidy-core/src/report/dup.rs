//! Duplicate files (SPEC §5.7, M14 §14.9 2): same size first, then
//! blake3, names ignored. Read-only.
//! The index's stored hash is reused when the file is unchanged, so a
//! repeated report on an indexed folder reads almost nothing.
//!
//! (M14) A watch folder's "모으기" target (`organize_root`) can be scanned
//! too, so a copy already organized there joins the group; each group
//! carries per-file details and a default "keep" choice. Clean-up itself is
//! [`crate::mover::Mover::trash_duplicates`] after the user approves.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use walkdir::WalkDir;

use super::backlog::canon;
use super::scan::DirNode;
use super::stats::{join, walk};
use crate::config::WatchRoot;
use crate::mover::{hash_file, TMP_MARKER};
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct DupOptions {
    /// Ignore files smaller than this (tiny files are rarely worth it).
    /// Empty files never count.
    pub min_size: u64,
    /// Never hash files above this size.
    pub max_size: u64,
    /// Report at most this many groups (largest waste first).
    pub max_groups: usize,
    /// (M14) Folders outside the report root to look for copies in (the
    /// watch folder's `organize_root`). Only copies of files inside the
    /// report root are reported.
    pub extra_roots: Vec<PathBuf>,
    /// (M14) "정리 위치" folders: files there are kept by default.
    pub organize_roots: Vec<PathBuf>,
}

impl Default for DupOptions {
    fn default() -> Self {
        Self {
            min_size: 1024,
            max_size: 2 * 1024 * 1024 * 1024,
            max_groups: 100,
            extra_roots: Vec::new(),
            organize_roots: Vec::new(),
        }
    }
}

impl DupOptions {
    /// (M14) Options for a report on `root`: scan the `organize_root` of
    /// the watch folder `root` belongs to (collect mode), and mark every
    /// watch folder's organize root as "정리 위치".
    pub fn for_root(root: &Path, watches: &[WatchRoot]) -> Self {
        let root = canon(root);
        let organize_roots: Vec<PathBuf> = watches
            .iter()
            .filter(|w| w.anchor() != w.path.as_path())
            .map(|w| canon(w.anchor()))
            .collect();
        let mut extra_roots: Vec<PathBuf> = watches
            .iter()
            .filter(|w| w.anchor() != w.path.as_path() && root.starts_with(canon(&w.path)))
            .map(|w| canon(w.anchor()))
            .filter(|a| !a.starts_with(&root))
            .collect();
        extra_roots.dedup();
        Self {
            extra_roots,
            organize_roots,
            ..Self::default()
        }
    }
}

/// (M14) One copy in a group.
#[derive(Debug, Clone, Serialize)]
pub struct DupFile {
    pub path: PathBuf,
    /// Path relative to the report root (`None` = outside it, e.g. in the
    /// organize root).
    pub rel: Option<String>,
    /// Unix seconds.
    pub modified: Option<i64>,
    pub in_organize_root: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DupGroup {
    pub hash: String,
    pub size: u64,
    /// Paths relative to the report root (absolute for copies outside it),
    /// sorted.
    pub paths: Vec<String>,
    /// Bytes that one copy would save: `size × (copies − 1)`.
    pub wasted: u64,
    /// (M14) The same copies with details, in `paths` order.
    pub files: Vec<DupFile>,
    /// (M14) Index in `files` of the copy kept by default
    /// ([`default_keep`]).
    pub keep: usize,
}

/// (M14) Which copy to keep by default: one in an organize root, then the
/// oldest modification time, then the shortest path, then by name.
pub fn default_keep(files: &[DupFile]) -> usize {
    (0..files.len())
        .min_by_key(|&i| {
            let f = &files[i];
            (
                !f.in_organize_root,
                f.modified.unwrap_or(i64::MAX),
                f.path.as_os_str().len(),
                f.path.clone(),
            )
        })
        .unwrap_or(0)
}

struct Cand {
    path: PathBuf,
    rel: Option<String>,
    modified: Option<i64>,
}

/// Duplicate groups in a scanned tree rooted at `root`.
pub fn find_duplicates(
    root: &Path,
    tree: &DirNode,
    opts: &DupOptions,
    index: Option<&Store>,
) -> Vec<DupGroup> {
    let min = opts.min_size.max(1);
    let mut by_size: HashMap<u64, Vec<Cand>> = HashMap::new();
    walk(tree, "", &mut |dir, node| {
        for f in &node.files {
            if f.size >= min && f.size <= opts.max_size {
                let rel = join(dir, &f.name);
                by_size.entry(f.size).or_default().push(Cand {
                    path: root.join(&rel),
                    modified: f.modified.and_then(unix),
                    rel: Some(rel),
                });
            }
        }
    });
    if !opts.extra_roots.is_empty() {
        let sizes: HashSet<u64> = by_size.keys().copied().collect();
        let root_c = canon(root);
        for extra in &opts.extra_roots {
            for e in WalkDir::new(extra)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')
                })
                .filter_map(Result::ok)
            {
                if !e.file_type().is_file()
                    || e.path().starts_with(&root_c)
                    || e.file_name().to_string_lossy().contains(TMP_MARKER)
                {
                    continue;
                }
                let Ok(m) = e.metadata() else { continue };
                if sizes.contains(&m.len()) {
                    by_size.entry(m.len()).or_default().push(Cand {
                        path: e.path().to_path_buf(),
                        rel: None,
                        modified: m.modified().ok().and_then(unix),
                    });
                }
            }
        }
    }

    let mut groups = Vec::new();
    for (size, cands) in by_size {
        if cands.len() < 2 || cands.iter().all(|c| c.rel.is_none()) {
            continue;
        }
        let mut by_hash: HashMap<String, Vec<Cand>> = HashMap::new();
        for c in cands {
            let cached = index
                .and_then(|s| s.get_file(&c.path).ok().flatten())
                .filter(|r| r.size == size)
                .and_then(|r| r.hash);
            // A stale cached hash only risks a missed/extra group in a
            // read-only report; trashing re-hashes every file first.
            let hash = match cached {
                Some(h) => h,
                None => match hash_file(&c.path) {
                    Ok(h) => h,
                    Err(_) => continue,
                },
            };
            by_hash.entry(hash).or_default().push(c);
        }
        for (hash, cands) in by_hash {
            let cands = drop_hard_links(cands);
            if cands.len() < 2 || cands.iter().all(|c| c.rel.is_none()) {
                continue;
            }
            let mut files: Vec<DupFile> = cands
                .into_iter()
                .map(|c| DupFile {
                    in_organize_root: opts
                        .organize_roots
                        .iter()
                        .any(|r| c.path.starts_with(r) || canon(&c.path).starts_with(r)),
                    path: c.path,
                    rel: c.rel,
                    modified: c.modified,
                })
                .collect();
            files.sort_by_key(label);
            groups.push(DupGroup {
                wasted: size * (files.len() as u64 - 1),
                paths: files.iter().map(label).collect(),
                keep: default_keep(&files),
                hash,
                size,
                files,
            });
        }
    }
    groups.sort_by(|a, b| b.wasted.cmp(&a.wasted).then_with(|| a.paths.cmp(&b.paths)));
    groups.truncate(opts.max_groups);
    groups
}

fn label(f: &DupFile) -> String {
    f.rel
        .clone()
        .unwrap_or_else(|| f.path.display().to_string())
}

fn unix(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Hard links to one file are not copies: keep one entry per file (the one
/// inside the report root first).
fn drop_hard_links(mut cands: Vec<Cand>) -> Vec<Cand> {
    cands.sort_by_key(|c| c.rel.is_none());
    let mut seen: Vec<same_file::Handle> = Vec::new();
    cands
        .into_iter()
        .filter(|c| match same_file::Handle::from_path(&c.path) {
            Ok(h) => {
                if seen.contains(&h) {
                    false
                } else {
                    seen.push(h);
                    true
                }
            }
            Err(_) => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{scan, ScanOptions};
    use std::fs;

    #[test]
    fn groups_by_content_not_name() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("백업 폴더")).unwrap();
        let a = vec![7u8; 4096];
        let mut b = a.clone();
        b[100] = 8; // same size, different content
        fs::write(r.join("보고서.pdf"), &a).unwrap();
        fs::write(r.join("백업 폴더").join("보고서 (1).pdf"), &a).unwrap();
        fs::write(r.join("백업 폴더").join("다른 것.pdf"), &b).unwrap();
        fs::write(r.join("작은.txt"), b"x").unwrap();
        fs::write(r.join("작은 사본.txt"), b"x").unwrap();

        let res = scan(r, &ScanOptions::default()).unwrap();
        let g = find_duplicates(&res.root, &res.tree, &DupOptions::default(), None);
        assert_eq!(g.len(), 1, "{g:#?}");
        assert_eq!(g[0].paths, vec!["백업 폴더/보고서 (1).pdf", "보고서.pdf"]);
        assert_eq!(g[0].wasted, 4096);
    }

    #[test]
    fn organize_root_copies_join_and_are_kept_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let desk = tmp.path().join("바탕 화면");
        let org = tmp.path().join("정리").join("문서");
        fs::create_dir_all(&desk).unwrap();
        fs::create_dir_all(&org).unwrap();
        let body = vec![3u8; 2048];
        fs::write(desk.join("보고서.pdf"), &body).unwrap();
        fs::write(desk.join("보고서 사본.pdf"), &body).unwrap();
        fs::write(org.join("보고서.pdf"), &body).unwrap();
        // Only in the organize root: not this report's business.
        fs::write(org.join("a.bin"), vec![9u8; 4000]).unwrap();
        fs::write(org.join("b.bin"), vec![9u8; 4000]).unwrap();
        let cfg = crate::Config::from_toml(&format!(
            "[[watch]]\npath = {:?}\nmode = \"collect\"\norganize_root = {:?}\n",
            desk.to_string_lossy(),
            tmp.path().join("정리").to_string_lossy()
        ))
        .unwrap();
        let opts = DupOptions::for_root(&desk, &cfg.watches);
        assert_eq!(opts.extra_roots.len(), 1);
        let res = scan(&desk, &ScanOptions::default()).unwrap();
        let g = find_duplicates(&res.root, &res.tree, &opts, None);
        assert_eq!(g.len(), 1, "{g:#?}");
        assert_eq!(g[0].files.len(), 3);
        let kept = &g[0].files[g[0].keep];
        assert!(kept.in_organize_root, "{kept:?}");
        assert!(kept.rel.is_none());
        assert_eq!(g[0].files.iter().filter(|f| f.rel.is_some()).count(), 2);
        assert_eq!(g[0].wasted, 2 * 2048);
    }

    #[test]
    fn default_keep_prefers_organized_then_oldest_then_shortest() {
        let f = |p: &str, m: Option<i64>, org: bool| DupFile {
            path: PathBuf::from(p),
            rel: None,
            modified: m,
            in_organize_root: org,
        };
        assert_eq!(
            default_keep(&[
                f("/a/b.txt", Some(1), false),
                f("/정리/b.txt", Some(9), true)
            ]),
            1
        );
        assert_eq!(
            default_keep(&[
                f("/a/new.txt", Some(9), false),
                f("/a/b/old.txt", Some(1), false)
            ]),
            1
        );
        assert_eq!(
            default_keep(&[
                f("/a/long/x.txt", Some(5), false),
                f("/a/x.txt", Some(5), false)
            ]),
            1
        );
        assert_eq!(
            default_keep(&[f("/b", None, false), f("/a", None, false)]),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_and_symlinks_are_not_copies() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::write(r.join("원본.bin"), vec![1u8; 3000]).unwrap();
        fs::hard_link(r.join("원본.bin"), r.join("링크.bin")).unwrap();
        std::os::unix::fs::symlink(r.join("원본.bin"), r.join("심볼릭.bin")).unwrap();
        fs::write(r.join("빈 파일 1"), b"").unwrap();
        fs::write(r.join("빈 파일 2"), b"").unwrap();
        let res = scan(r, &ScanOptions::default()).unwrap();
        let opts = DupOptions {
            min_size: 0,
            ..DupOptions::default()
        };
        assert!(find_duplicates(&res.root, &res.tree, &opts, None).is_empty());
        fs::write(r.join("진짜 사본.bin"), vec![1u8; 3000]).unwrap();
        let res = scan(r, &ScanOptions::default()).unwrap();
        let g = find_duplicates(&res.root, &res.tree, &opts, None);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].files.len(), 2, "{:?}", g[0].paths);
    }
}
