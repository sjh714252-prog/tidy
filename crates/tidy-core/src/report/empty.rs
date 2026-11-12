//! (0.14.0) Empty folders under the watch folders and the organize
//! location ("빈 폴더 정리"). Read-only: removing them is
//! [`crate::mover::Mover::remove_empty_dirs`], only after the user approves.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::mover::{empty_tree, is_os_clutter};
use crate::watcher::IgnoreFilter;

/// A folder made this recently may be about to get files: left alone.
pub const MIN_AGE: Duration = Duration::from_secs(10 * 60);
/// At most this many folders are listed per run.
pub const MAX_FOUND: usize = 2000;

/// The top-most empty folder of an empty tree (its empty sub-folders go
/// with it).
#[derive(Debug, Clone, Serialize)]
pub struct EmptyFolder {
    pub path: PathBuf,
    /// The watch folder / organize location it was found in.
    pub root: PathBuf,
    /// Folders in the tree, itself included.
    pub dirs: usize,
    /// OS clutter files inside (Thumbs.db, desktop.ini): go to the trash.
    pub clutter: usize,
}

pub struct EmptyOptions<'a> {
    pub filter: &'a IgnoreFilter,
    /// Never removed, nor any folder holding one of them: protected
    /// folders, watch folders, the organize location, rule targets.
    pub keep: &'a [PathBuf],
    pub min_age: Duration,
    pub now: SystemTime,
}

/// Empty folders in every local watch folder and organize location of
/// `cfg`, each place once (a place inside another is looked at as part of
/// the outer one).
pub fn find_all(cfg: &crate::Config, now: SystemTime) -> Vec<EmptyFolder> {
    use crate::config::RootKind;
    let filter = cfg
        .ignore_filter()
        .unwrap_or_else(|_| IgnoreFilter::new(&[]).expect("empty glob set"));
    let local: Vec<_> = cfg
        .watches
        .iter()
        .filter(|w| w.kind == RootKind::Local)
        .cloned()
        .collect();
    let mut roots: Vec<PathBuf> = local.iter().map(|w| w.path.clone()).collect();
    for r in super::backlog::organize_roots(&local) {
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    let mut keep = roots.clone();
    keep.extend(cfg.report.protected.iter().cloned());
    let opts = EmptyOptions {
        filter: &filter,
        keep: &keep,
        min_age: MIN_AGE,
        now,
    };
    let mut out: Vec<EmptyFolder> = Vec::new();
    for r in &roots {
        if roots.iter().any(|o| o != r && r.starts_with(o)) || !r.is_dir() {
            continue;
        }
        out.extend(find_empty(r, &opts));
        if out.len() >= MAX_FOUND {
            out.truncate(MAX_FOUND);
            break;
        }
    }
    out
}

/// Empty folders under `root` (never `root` itself), top-most only.
pub fn find_empty(root: &Path, opts: &EmptyOptions) -> Vec<EmptyFolder> {
    let mut out = Vec::new();
    visit(root, root, opts, 0, &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Whether `dir` is an empty tree that may go. When it is not, its own
/// empty sub-trees are added to `out`.
fn visit(
    dir: &Path,
    root: &Path,
    opts: &EmptyOptions,
    depth: usize,
    out: &mut Vec<EmptyFolder>,
) -> bool {
    if depth > 32 || out.len() >= MAX_FOUND {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut empty = true;
    let mut subs: Vec<PathBuf> = Vec::new();
    for e in rd.filter_map(Result::ok) {
        let Ok(ft) = e.file_type() else {
            empty = false;
            continue;
        };
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_file() && is_os_clutter(&name) {
            continue;
        }
        if !ft.is_dir() || ft.is_symlink() || name.starts_with('.') {
            empty = false;
            continue;
        }
        let p = e.path();
        if opts.filter.in_excluded_dir(&p)
            || opts.filter.is_ignored(&p)
            || opts.keep.iter().any(|k| k.starts_with(&p))
        {
            empty = false;
            continue;
        }
        subs.push(p);
    }
    subs.sort();
    let mut empties: Vec<PathBuf> = Vec::new();
    for s in subs {
        if visit(&s, root, opts, depth + 1, out) {
            empties.push(s);
        } else {
            empty = false;
        }
    }
    let old = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .map(|t| opts.now.duration_since(t).unwrap_or_default() >= opts.min_age)
        .unwrap_or(false);
    if empty && old && depth > 0 {
        return true;
    }
    // This folder stays: its empty sub-trees are what can go.
    for s in empties {
        if out.len() >= MAX_FOUND {
            break;
        }
        if let Some(t) = empty_tree(&s) {
            out.push(EmptyFolder {
                path: s,
                root: root.to_path_buf(),
                dirs: t.dirs.len(),
                clutter: t.clutter.len(),
            });
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn opts<'a>(filter: &'a IgnoreFilter, keep: &'a [PathBuf]) -> EmptyOptions<'a> {
        EmptyOptions {
            filter,
            keep,
            min_age: Duration::ZERO,
            now: SystemTime::now(),
        }
    }

    #[test]
    fn finds_top_most_empty_folders_only() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("java").join("lib").join("ext")).unwrap();
        fs::create_dir_all(r.join("고객").join("빈 하위")).unwrap();
        fs::write(r.join("고객").join("a.txt"), b"a").unwrap();
        fs::create_dir_all(r.join("Gmail AI 답변")).unwrap();
        fs::write(r.join("Gmail AI 답변").join("Thumbs.db"), b"x").unwrap();
        fs::create_dir_all(r.join("바로가기만")).unwrap();
        fs::write(r.join("바로가기만").join("x.lnk"), b"x").unwrap();
        fs::create_dir_all(r.join(".hidden")).unwrap();
        fs::create_dir_all(r.join("정리")).unwrap();

        let filter = IgnoreFilter::new(&[]).unwrap();
        let keep = vec![r.join("정리")];
        let found = find_empty(r, &opts(&filter, &keep));
        let names: Vec<String> = found
            .iter()
            .map(|f| f.path.strip_prefix(r).unwrap().display().to_string())
            .collect();
        let sub = Path::new("고객").join("빈 하위").display().to_string();
        assert_eq!(names, vec!["Gmail AI 답변".to_string(), "java".into(), sub]);
        assert_eq!(found[1].dirs, 3);
        assert_eq!(found[0].clutter, 1);
    }

    #[test]
    fn young_folders_wait() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("새 폴더")).unwrap();
        let filter = IgnoreFilter::new(&[]).unwrap();
        let mut o = opts(&filter, &[]);
        o.min_age = MIN_AGE;
        assert!(find_empty(tmp.path(), &o).is_empty());
    }
}
