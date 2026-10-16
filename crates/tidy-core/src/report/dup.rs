//! Duplicate files (SPEC §5.7): same size first, then blake3. Read-only.
//! The index's stored hash is reused when the file is unchanged, so a
//! repeated report on an indexed folder reads almost nothing.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use super::scan::DirNode;
use super::stats::{join, walk};
use crate::mover::hash_file;
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct DupOptions {
    /// Ignore files smaller than this (tiny files are rarely worth it).
    pub min_size: u64,
    /// Never hash files above this size.
    pub max_size: u64,
    /// Report at most this many groups (largest waste first).
    pub max_groups: usize,
}

impl Default for DupOptions {
    fn default() -> Self {
        Self {
            min_size: 1024,
            max_size: 2 * 1024 * 1024 * 1024,
            max_groups: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DupGroup {
    pub hash: String,
    pub size: u64,
    /// Paths relative to the report root, sorted.
    pub paths: Vec<String>,
    /// Bytes that one copy would save: `size × (copies − 1)`.
    pub wasted: u64,
}

/// Duplicate groups in a scanned tree rooted at `root`.
pub fn find_duplicates(
    root: &Path,
    tree: &DirNode,
    opts: &DupOptions,
    index: Option<&Store>,
) -> Vec<DupGroup> {
    let mut by_size: HashMap<u64, Vec<String>> = HashMap::new();
    walk(tree, "", &mut |dir, node| {
        for f in &node.files {
            if f.size >= opts.min_size.max(1) && f.size <= opts.max_size {
                by_size.entry(f.size).or_default().push(join(dir, &f.name));
            }
        }
    });

    let mut groups = Vec::new();
    for (size, rels) in by_size.into_iter().filter(|(_, v)| v.len() > 1) {
        let mut by_hash: HashMap<String, Vec<String>> = HashMap::new();
        for rel in rels {
            let abs = root.join(&rel);
            let cached = index
                .and_then(|s| s.get_file(&abs).ok().flatten())
                .filter(|r| r.size == size)
                .and_then(|r| r.hash);
            // A stale cached hash only risks a missed/extra group in a
            // read-only report; moves never rely on it.
            let hash = match cached {
                Some(h) => h,
                None => match hash_file(&abs) {
                    Ok(h) => h,
                    Err(_) => continue,
                },
            };
            by_hash.entry(hash).or_default().push(rel);
        }
        for (hash, mut paths) in by_hash.into_iter().filter(|(_, v)| v.len() > 1) {
            paths.sort();
            groups.push(DupGroup {
                wasted: size * (paths.len() as u64 - 1),
                hash,
                size,
                paths,
            });
        }
    }
    groups.sort_by(|a, b| b.wasted.cmp(&a.wasted).then_with(|| a.paths.cmp(&b.paths)));
    groups.truncate(opts.max_groups);
    groups
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
}
