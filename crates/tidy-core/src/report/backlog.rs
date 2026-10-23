//! (M11, SPEC §14.6) "기존 파일 미정리 N개": what is still sitting in a
//! watch folder from before it was watched. Counts only — nothing moves.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::classify::{FileFacts, RuleSet};
use crate::config::WatchRoot;
use crate::mover::TMP_MARKER;
use crate::watcher::IgnoreFilter;

/// `p` with its nearest existing ancestor canonicalized (symlinks such as
/// macOS `/var` → `/private/var` resolved) and the missing rest appended,
/// so a folder that does not exist yet still compares with real ones.
fn canon(p: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(c) = dunce::canonicalize(cur) {
            return rest.iter().rev().fold(c, |acc, part| acc.join(part));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

/// Files looked at per folder at most ("20,000+" beyond).
pub const BACKLOG_CAP: usize = 20_000;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Backlog {
    /// Files a rule matches that are not at the rule's destination yet
    /// (what "규칙으로 정리 미리보기" would propose).
    pub to_move: usize,
    /// Files directly in the folder that no rule matches (left for the
    /// work-context organizer or the user).
    pub no_rule: usize,
    /// Files looked at.
    pub scanned: usize,
    /// Stopped at the cap: the counts are lower bounds.
    pub capped: bool,
}

impl Backlog {
    pub fn total(&self) -> usize {
        self.to_move + self.no_rule
    }
}

/// Count the backlog of one watch folder. `skip` holds folders that are
/// already organized results (other watch folders' `organize_root`s) and
/// are not counted even when they lie inside this one; the watch folder's
/// own `exclude` folders come with `filter`.
pub fn backlog(
    w: &WatchRoot,
    rules: &RuleSet,
    filter: &IgnoreFilter,
    skip: &[PathBuf],
    cap: usize,
) -> Backlog {
    let root = canon(&w.path);
    let anchor = canon(w.anchor());
    let skip: Vec<PathBuf> = skip
        .iter()
        .map(|p| canon(p))
        .filter(|p| p != &root)
        .collect();
    let mut out = Backlog::default();
    let walker = walkdir::WalkDir::new(&root)
        .min_depth(1)
        .max_depth(if w.recursive { usize::MAX } else { 1 })
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !n.starts_with('.')
                && !n.contains(TMP_MARKER)
                && !filter.in_excluded_dir(e.path())
                && !skip.iter().any(|s| e.path().starts_with(s))
        });
    for e in walker.filter_map(Result::ok) {
        if !e.file_type().is_file() || filter.is_ignored(e.path()) {
            continue;
        }
        if out.scanned >= cap {
            out.capped = true;
            break;
        }
        out.scanned += 1;
        let Ok(facts) = FileFacts::from_path(e.path()) else {
            continue;
        };
        match rules.classify_in(&facts, Some(anchor.as_path())) {
            Some(c) => {
                if e.path().parent() != Some(c.dest_dir.as_path()) {
                    out.to_move += 1;
                }
            }
            None if e.depth() == 1 => out.no_rule += 1,
            None => {}
        }
    }
    out
}

/// `organize_root`s of every "지정 위치로 모으기" watch folder: results
/// that are never counted as backlog (SPEC §14.6).
pub fn organize_roots(watches: &[WatchRoot]) -> Vec<PathBuf> {
    watches
        .iter()
        .filter(|w| w.anchor() != w.path.as_path())
        .map(|w| w.anchor().to_path_buf())
        .collect()
}

/// The watch folder whose `organize_root` is `p` or holds it.
pub fn organize_root_of<'a>(watches: &'a [WatchRoot], p: &Path) -> Option<&'a WatchRoot> {
    let p = canon(p);
    watches
        .iter()
        .find(|w| w.anchor() != w.path.as_path() && p.starts_with(canon(w.anchor())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OrganizeMode;

    fn watch(path: &Path) -> WatchRoot {
        crate::Config::from_toml(&format!("[[watch]]\npath = {:?}\n", path.to_string_lossy()))
            .unwrap()
            .watches
            .remove(0)
    }

    #[test]
    fn counts_rule_matches_and_unmatched_top_level_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for n in ["a.pdf", "b.pdf", "memo.xyz", ".hidden.pdf"] {
            std::fs::write(root.join(n), "x").unwrap();
        }
        std::fs::create_dir_all(root.join("문서")).unwrap();
        std::fs::write(root.join("문서").join("c.pdf"), "x").unwrap();
        let rules = RuleSet::from_toml("[[rule]]\nname='문서'\nmatch={ext=['pdf']}\ndest='문서'\n")
            .unwrap();
        let filter = IgnoreFilter::new(&[]).unwrap();
        let w = watch(root);
        let b = backlog(&w, &rules, &filter, &[], BACKLOG_CAP);
        assert_eq!((b.to_move, b.no_rule, b.capped), (2, 1, false));

        // Recursive: 문서/c.pdf is already in place.
        let mut wr = w.clone();
        wr.recursive = true;
        let b = backlog(&wr, &rules, &filter, &[], BACKLOG_CAP);
        assert_eq!((b.to_move, b.no_rule), (2, 1));

        // Cap.
        let b = backlog(&w, &rules, &filter, &[], 2);
        assert!(b.capped);
        assert_eq!(b.scanned, 2);

        // An organize root inside the watch folder is not counted.
        let mut c = wr.clone();
        c.mode = OrganizeMode::Collect;
        c.organize_root = Some(root.join("문서"));
        std::fs::write(root.join("문서").join("old.xyz"), "x").unwrap();
        let b = backlog(
            &wr,
            &rules,
            &filter,
            &organize_roots(&[c.clone()]),
            BACKLOG_CAP,
        );
        assert_eq!(b.scanned, 3);
        assert!(organize_root_of(&[c.clone()], &root.join("문서").join("2026")).is_some());
        assert!(organize_root_of(&[c], root).is_none());
    }

    /// macOS: the temp dir is `/var/…`, a symlink to `/private/var/…`. A
    /// folder that does not exist yet must still match its organize root.
    #[cfg(unix)]
    #[test]
    fn symlinked_parents_and_missing_folders_compare() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real");
        std::fs::create_dir_all(real.join("정리")).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            canon(&link.join("정리").join("없음")),
            canon(&real).join("정리").join("없음")
        );
        let mut w = watch(&link);
        w.mode = OrganizeMode::Collect;
        w.organize_root = Some(link.join("정리"));
        assert!(
            organize_root_of(std::slice::from_ref(&w), &link.join("정리").join("2026")).is_some()
        );
        assert!(organize_root_of(std::slice::from_ref(&w), &real.join("정리")).is_some());
        assert!(organize_root_of(&[w], &link.join("다른")).is_none());
    }
}
