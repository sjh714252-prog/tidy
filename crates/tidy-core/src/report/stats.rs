//! Aggregate statistics over a scanned tree.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use super::scan::{DirNode, FileNode};

#[derive(Debug, Clone, Serialize)]
pub struct TypeStat {
    pub ext: String,
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderStat {
    pub path: String,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub total_files: u64,
    pub total_dirs: u64,
    pub total_bytes: u64,
    /// Files directly in the root (the "desktop clutter" number).
    pub root_level_files: u64,
    pub by_type: Vec<TypeStat>,
    pub largest_files: Vec<FolderStat>,
    pub largest_dirs: Vec<FolderStat>,
    pub empty_dirs: Vec<String>,
    /// Files not modified within `stale_after`.
    pub stale_files: u64,
    /// The `top_n` longest-unused files (later of modified / accessed).
    pub stale: Vec<StaleFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleFile {
    pub path: String,
    pub bytes: u64,
    /// Unix seconds of the last modification or access.
    pub last_used: i64,
}

impl Stats {
    pub fn compute(tree: &DirNode, top_n: usize, stale_after: Duration) -> Self {
        let mut by_type: HashMap<String, TypeStat> = HashMap::new();
        let mut files: Vec<FolderStat> = Vec::new();
        let mut dirs: Vec<FolderStat> = Vec::new();
        let mut empty: Vec<String> = Vec::new();
        let now = SystemTime::now();
        let mut stale = 0u64;
        let mut stale_list: Vec<StaleFile> = Vec::new();

        walk(tree, "", &mut |path, node| {
            for f in &node.files {
                let key = f.ext.clone().unwrap_or_else(|| "(none)".into());
                let e = by_type.entry(key.clone()).or_insert(TypeStat {
                    ext: key,
                    count: 0,
                    bytes: 0,
                });
                e.count += 1;
                e.bytes += f.size;
                files.push(FolderStat {
                    path: join(path, &f.name),
                    bytes: f.size,
                    files: 1,
                });
                if is_stale(f, now, stale_after) {
                    stale += 1;
                    stale_list.push(StaleFile {
                        path: join(path, &f.name),
                        bytes: f.size,
                        last_used: last_used(f).map(unix).unwrap_or(0),
                    });
                }
            }
            if !path.is_empty() {
                dirs.push(FolderStat {
                    path: path.to_string(),
                    bytes: node.total_size,
                    files: node.file_count,
                });
                if node.file_count == 0 && node.dirs.is_empty() && !node.truncated {
                    empty.push(path.to_string());
                }
            }
        });

        let mut by_type: Vec<TypeStat> = by_type.into_values().collect();
        by_type.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.ext.cmp(&b.ext)));
        files.sort_by_key(|f| std::cmp::Reverse(f.bytes));
        files.truncate(top_n);
        dirs.sort_by_key(|d| std::cmp::Reverse(d.bytes));
        dirs.truncate(top_n);
        empty.sort();
        stale_list.sort_by_key(|f| f.last_used);
        stale_list.truncate(top_n);

        Stats {
            total_files: tree.file_count,
            total_dirs: tree.dir_count,
            total_bytes: tree.total_size,
            root_level_files: tree.files.len() as u64,
            by_type,
            largest_files: files,
            largest_dirs: dirs,
            empty_dirs: empty,
            stale_files: stale,
            stale: stale_list,
        }
    }
}

fn last_used(f: &FileNode) -> Option<SystemTime> {
    match (f.modified, f.accessed) {
        (Some(m), Some(a)) => Some(m.max(a)),
        (m, a) => m.or(a),
    }
}

fn unix(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// SPEC §5.7 "N일 이상 미접근": not modified or opened within `after`.
fn is_stale(f: &FileNode, now: SystemTime, after: Duration) -> bool {
    match last_used(f).and_then(|m| now.duration_since(m).ok()) {
        Some(age) => age > after,
        None => false,
    }
}

pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn walk<'a>(node: &'a DirNode, path: &str, f: &mut impl FnMut(&str, &'a DirNode)) {
    f(path, node);
    for (name, child) in &node.dirs {
        walk(child, &join(path, name), f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::scan::{scan, ScanOptions};
    use std::fs;

    #[test]
    fn computes_type_and_folder_stats() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("docs")).unwrap();
        fs::create_dir_all(r.join("empty")).unwrap();
        fs::write(r.join("a.pdf"), vec![0u8; 300]).unwrap();
        fs::write(r.join("docs/b.pdf"), vec![0u8; 200]).unwrap();
        fs::write(r.join("docs/c.txt"), vec![0u8; 50]).unwrap();
        fs::write(r.join("noext"), vec![0u8; 1]).unwrap();

        let res = scan(r, &ScanOptions::default()).unwrap();
        let s = Stats::compute(&res.tree, 10, Duration::from_secs(86400 * 90));
        assert_eq!(s.total_files, 4);
        assert_eq!(s.root_level_files, 2);
        assert_eq!(s.by_type[0].ext, "pdf");
        assert_eq!(s.by_type[0].bytes, 500);
        assert!(s.by_type.iter().any(|t| t.ext == "(none)"));
        assert_eq!(s.largest_files[0].path, "a.pdf");
        assert_eq!(s.largest_dirs[0].path, "docs");
        assert_eq!(s.empty_dirs, vec!["empty".to_string()]);
        assert_eq!(s.stale_files, 0);
    }
}
