//! (0.12.0, SPEC §14.18) Folders that already exist where work-context
//! results go (the organize root, e.g. `D:\정리`). Before a new
//! customer / issue folder is made, a folder of the same name — spelled a
//! little differently ("KT 스마트메시지" vs "KT스마트메시지"), or the same
//! issue a few days apart ("20261014_과금" for a 20261020 과금 file) — is
//! reused. The names also go to the AI as a hint, so it answers with them.
//! Reads only; nothing here creates a folder.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::mover::TMP_MARKER;

/// Customer / topic folders listed per organize root (prompt + settings).
const MAX_TOP: usize = 80;
/// Issue folders listed per customer folder in the prompt.
const MAX_SUB: usize = 8;
/// Prompt hint size cap (characters).
const MAX_HINT_CHARS: usize = 4000;

/// A name reduced for comparison: lower case, letters and digits only
/// (Hangul included), so spaces, `_`, `-`, `·`, `(주)` brackets and case
/// do not matter.
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// "20261014_과금" → (2026-10-14, "과금").
pub fn split_dated(name: &str) -> Option<(NaiveDate, &str)> {
    let (d, rest) = name.split_at_checked(8)?;
    let rest = rest.strip_prefix(['_', '-', ' '])?;
    if rest.trim().is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((NaiveDate::parse_from_str(d, "%Y%m%d").ok()?, rest))
}

/// Visible sub-folder names of `dir`, sorted.
pub fn sub_dirs(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && !n.contains(TMP_MARKER))
        .collect();
    out.sort();
    out
}

/// The existing sibling in `parent` that stands for `name`: the same name
/// apart from spacing/case, or — for a dated issue folder — the same issue
/// with a date at most `merge_days` away (the closest one).
pub fn find_existing(parent: &Path, name: &str, merge_days: u32) -> Option<String> {
    let subs = sub_dirs(parent);
    let key = norm(name);
    if key.is_empty() {
        return None;
    }
    if let Some(s) = subs.iter().find(|s| norm(s) == key) {
        return Some(s.clone());
    }
    let (date, issue) = split_dated(name)?;
    let issue = norm(issue);
    subs.iter()
        .filter_map(|s| {
            let (d, i) = split_dated(s)?;
            let gap = (d - date).num_days().abs();
            (norm(i) == issue && gap <= i64::from(merge_days)).then_some((gap, s))
        })
        .min_by_key(|(gap, _)| *gap)
        .map(|(_, s)| s.clone())
}

/// `dir` (a folder below `anchor`) with every step that does not exist yet
/// swapped for an existing folder that stands for it ([`find_existing`]).
/// Steps under a folder that is itself new stay as they are. A `dir`
/// outside `anchor` comes back unchanged.
pub fn reuse_existing(dir: &Path, anchor: &Path, merge_days: u32) -> PathBuf {
    let Ok(rel) = dir.strip_prefix(anchor) else {
        return dir.to_path_buf();
    };
    let mut cur = anchor.to_path_buf();
    let mut parent_exists = anchor.is_dir();
    for comp in rel.components() {
        let name = comp.as_os_str().to_string_lossy();
        let direct = cur.join(&*name);
        // Exact on-disk name first: on case-insensitive file systems
        // (Windows, macOS) `direct.is_dir()` is true for "kt…" when "KT…"
        // exists, and the folder's own spelling should be kept.
        if !parent_exists || sub_dirs(&cur).iter().any(|s| *s == *name) {
            parent_exists = parent_exists && direct.is_dir();
            cur = direct;
            continue;
        }
        match find_existing(&cur, &name, merge_days) {
            Some(found) => cur.push(found),
            None => {
                cur = direct;
                parent_exists = false;
            }
        }
    }
    cur
}

/// The first folder on the way from `anchor` to `dir` that does not exist
/// yet (`None`: `dir` exists — nothing new is made).
pub fn first_new(dir: &Path, anchor: &Path) -> Option<PathBuf> {
    if dir.is_dir() {
        return None;
    }
    let rel = dir.strip_prefix(anchor).ok()?;
    let mut cur = anchor.to_path_buf();
    for comp in rel.components() {
        cur.push(comp);
        if !cur.is_dir() {
            return Some(cur);
        }
    }
    None
}

/// What is already in an organize root: its folders and, for each, the
/// folders right below (customer → issue folders).
#[derive(Debug, Clone, Default)]
pub struct ExistingTree {
    pub root: PathBuf,
    pub folders: Vec<(String, Vec<String>)>,
}

impl ExistingTree {
    pub fn scan(root: &Path) -> Self {
        let folders = sub_dirs(root)
            .into_iter()
            .take(MAX_TOP)
            .map(|n| {
                let mut subs = sub_dirs(&root.join(&n));
                // Dated issue folders first (newest first): the ones to reuse.
                subs.sort_by(|a, b| {
                    let (da, db) = (split_dated(a).is_some(), split_dated(b).is_some());
                    db.cmp(&da).then_with(|| b.cmp(a))
                });
                (n, subs)
            })
            .collect();
        Self {
            root: root.to_path_buf(),
            folders,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }
}

/// Prompt section listing the existing folders of every organize root
/// (empty when there are none).
pub fn prompt_hint(trees: &[ExistingTree]) -> String {
    let mut s = String::new();
    for t in trees.iter().filter(|t| !t.is_empty()) {
        for (name, subs) in &t.folders {
            let mut line = format!("- {name}");
            if !subs.is_empty() {
                let shown: Vec<&str> = subs.iter().take(MAX_SUB).map(String::as_str).collect();
                line.push_str(&format!(": {}", shown.join(", ")));
                if subs.len() > MAX_SUB {
                    line.push_str(&format!(" 외 {}개", subs.len() - MAX_SUB));
                }
            }
            line.push('\n');
            if s.chars().count() + line.chars().count() > MAX_HINT_CHARS {
                s.push_str("- …\n");
                return s;
            }
            s.push_str(&line);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn norm_ignores_spacing_case_and_marks() {
        assert_eq!(norm("KT 스마트메시지"), norm("kt스마트메시지"));
        assert_eq!(norm("㈜모노 커뮤니케이션즈"), norm("모노커뮤니케이션즈"));
        assert_ne!(norm("DB손해보험"), norm("DB손보"));
    }

    #[test]
    fn dated_names() {
        let (d, i) = split_dated("20261014_과금 이슈").unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 10, 14).unwrap());
        assert_eq!(i, "과금 이슈");
        assert!(split_dated("2026101_과금").is_none());
        assert!(split_dated("20261399_과금").is_none());
        assert!(split_dated("20261014").is_none());
        assert!(split_dated("과금").is_none());
    }

    #[test]
    fn reuses_same_customer_and_nearby_issue() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path();
        fs::create_dir_all(a.join("KT스마트메시지/20261014_과금/로그")).unwrap();
        fs::create_dir_all(a.join("KT스마트메시지/20260901_과금")).unwrap();
        // Different spelling, issue 6 days later → the existing folders.
        let got = reuse_existing(&a.join("KT 스마트메시지/20261020_과금/안내문서"), a, 14);
        assert_eq!(got, a.join("KT스마트메시지/20261014_과금/안내문서"));
        // Too far apart → a new issue folder under the existing customer.
        let got = reuse_existing(&a.join("kt스마트메시지/20261201_과금"), a, 14);
        assert_eq!(got, a.join("KT스마트메시지/20261201_과금"));
        assert_eq!(first_new(&got, a), Some(got.clone()));
        // A new customer: everything below stays as asked.
        let got = reuse_existing(&a.join("가람정보/20261014_과금"), a, 14);
        assert_eq!(got, a.join("가람정보/20261014_과금"));
        assert_eq!(first_new(&got, a), Some(a.join("가람정보")));
        assert_eq!(first_new(&a.join("KT스마트메시지/20261014_과금"), a), None);
    }

    #[test]
    fn korean_hint_lists_issue_folders_first() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path();
        fs::create_dir_all(a.join("DB손해보험/20261020_장애")).unwrap();
        fs::create_dir_all(a.join("DB손해보험/참고")).unwrap();
        fs::create_dir_all(a.join("기타/정책")).unwrap();
        let tree = ExistingTree::scan(a);
        let h = prompt_hint(&[tree]);
        assert!(h.contains("- DB손해보험: 20261020_장애, 참고"), "{h}");
        assert!(h.contains("- 기타: 정책"), "{h}");
    }
}
