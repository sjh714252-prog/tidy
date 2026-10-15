//! Read-only directory scan producing an in-memory tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("root does not exist or is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Maximum depth to descend (0 = only the root's direct children).
    pub max_depth: Option<usize>,
    /// Skip entries whose name starts with `.`.
    pub skip_hidden: bool,
    /// Follow symbolic links. Off by default to avoid loops on network shares.
    pub follow_links: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_depth: None,
            skip_hidden: true,
            follow_links: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FileNode {
    pub name: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub ext: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DirNode {
    pub name: String,
    pub dirs: BTreeMap<String, DirNode>,
    pub files: Vec<FileNode>,
    /// Total bytes of all files in this subtree.
    pub total_size: u64,
    /// Total file count in this subtree.
    pub file_count: u64,
    /// Total directory count in this subtree (excluding self).
    pub dir_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanResult {
    pub root: PathBuf,
    pub tree: DirNode,
    /// Paths that could not be read (permission etc.). Not fatal.
    pub errors: Vec<String>,
}

/// Scan `root` and build the tree. Unreadable entries are collected into
/// `errors` rather than aborting, since Desktop folders routinely contain
/// odd items (OneDrive placeholders, broken links).
pub fn scan(root: &Path, opts: &ScanOptions) -> Result<ScanResult, ScanError> {
    let root = root.canonicalize().map_err(|source| ScanError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !root.is_dir() {
        return Err(ScanError::NotADirectory(root));
    }

    let mut walker = WalkDir::new(&root)
        .follow_links(opts.follow_links)
        .min_depth(1)
        .sort_by_file_name();
    if let Some(d) = opts.max_depth {
        walker = walker.max_depth(d + 1);
    }

    let mut tree = DirNode {
        name: root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let mut errors = Vec::new();

    let skip_hidden = opts.skip_hidden;
    let iter = walker.into_iter().filter_entry(move |e| {
        if !skip_hidden {
            return true;
        }
        e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')
    });

    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
        };
        let rel = match entry.path().strip_prefix(&root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let comps: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if comps.is_empty() {
            continue;
        }

        let ft = entry.file_type();
        if ft.is_dir() {
            insert_dir(&mut tree, &comps);
        } else if ft.is_file() {
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("{}: {e}", entry.path().display()));
                    continue;
                }
            };
            let name = comps.last().cloned().unwrap_or_default();
            let node = FileNode {
                ext: Path::new(&name)
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase()),
                name,
                size: md.len(),
                modified: md.modified().ok(),
            };
            insert_file(&mut tree, &comps[..comps.len() - 1], node);
        }
        // symlinks (when not followed) and other types are ignored.
    }

    finalize(&mut tree);
    Ok(ScanResult { root, tree, errors })
}

fn insert_dir(tree: &mut DirNode, comps: &[String]) {
    let mut cur = tree;
    for c in comps {
        cur = cur.dirs.entry(c.clone()).or_insert_with(|| DirNode {
            name: c.clone(),
            ..Default::default()
        });
    }
}

fn insert_file(tree: &mut DirNode, dir_comps: &[String], file: FileNode) {
    let mut cur = tree;
    for c in dir_comps {
        cur = cur.dirs.entry(c.clone()).or_insert_with(|| DirNode {
            name: c.clone(),
            ..Default::default()
        });
    }
    cur.files.push(file);
}

/// Compute subtree totals bottom-up.
fn finalize(node: &mut DirNode) {
    let mut size: u64 = node.files.iter().map(|f| f.size).sum();
    let mut files: u64 = node.files.len() as u64;
    let mut dirs: u64 = 0;
    for child in node.dirs.values_mut() {
        finalize(child);
        size += child.total_size;
        files += child.file_count;
        dirs += 1 + child.dir_count;
    }
    node.total_size = size;
    node.file_count = files;
    node.dir_count = dirs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scans_nested_tree_with_korean_names() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("업무/KT")).unwrap();
        fs::create_dir_all(r.join("사진")).unwrap();
        fs::write(r.join("메모.txt"), b"hello").unwrap();
        fs::write(r.join("업무/KT/과금.xlsx"), vec![0u8; 100]).unwrap();
        fs::write(r.join("사진/a.PNG"), vec![0u8; 10]).unwrap();
        fs::write(r.join(".hidden"), b"x").unwrap();

        let res = scan(r, &ScanOptions::default()).unwrap();
        assert!(res.errors.is_empty());
        assert_eq!(res.tree.file_count, 3); // hidden skipped
        assert_eq!(res.tree.dir_count, 3);
        assert_eq!(res.tree.total_size, 115);
        let kt = &res.tree.dirs["업무"].dirs["KT"];
        assert_eq!(kt.files[0].name, "과금.xlsx");
        assert_eq!(kt.files[0].ext.as_deref(), Some("xlsx"));
        assert_eq!(res.tree.dirs["사진"].files[0].ext.as_deref(), Some("png"));
    }

    #[test]
    fn max_depth_limits_descent() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("a/b/c")).unwrap();
        fs::write(r.join("a/b/c/deep.txt"), b"x").unwrap();
        let res = scan(
            r,
            &ScanOptions {
                max_depth: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(res.tree.dirs["a"].dirs.contains_key("b"));
        assert!(res.tree.dirs["a"].dirs["b"].dirs.is_empty());
    }

    #[test]
    fn missing_root_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan(&tmp.path().join("nope"), &ScanOptions::default()).is_err());
    }
}
