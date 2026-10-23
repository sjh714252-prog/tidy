//! "지금 N개 해당" (SPEC §14.1): which existing files in the watch folders
//! each rule would take. Reads folder listings and metadata only; nothing
//! is moved.

use std::path::PathBuf;

use serde::Serialize;

use super::rules::{Anchors, FileFacts, RuleSet};
use crate::config::{RootKind, WatchRoot};
use crate::watcher::IgnoreFilter;

/// Files looked at, over all folders (a huge desktop stays responsive).
pub const MAX_FILES: usize = 20_000;
const EXAMPLES: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RulePreview {
    /// Position of the rule's `[[rule]]` table (matches card order).
    pub index: usize,
    pub name: String,
    pub count: usize,
    /// File names, at most three.
    pub examples: Vec<String>,
    /// Where the first example would go.
    pub sample_dest: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PreviewReport {
    pub rules: Vec<RulePreview>,
    /// Files looked at.
    pub scanned: usize,
    /// Of those, matched by no rule (`exclusive` only).
    pub unmatched: usize,
    /// Stopped at [`MAX_FILES`].
    pub truncated: bool,
    /// Network folders are not listed here (slow); shown as a note.
    pub skipped: Vec<PathBuf>,
    /// Folders that could not be read (missing, offline).
    pub missing: Vec<PathBuf>,
}

/// Files now in the watch folders, with the anchor their relative rule
/// destinations use. Collect once, match many times ([`match_files`]).
#[derive(Debug, Clone, Default)]
pub struct FileSample {
    pub files: Vec<(FileFacts, PathBuf)>,
    pub truncated: bool,
    pub skipped: Vec<PathBuf>,
    pub missing: Vec<PathBuf>,
}

/// List the files of `watches` as the watcher would see them (recursion
/// and ignore globs as configured), at most [`MAX_FILES`]. Network folders
/// are skipped (slow); missing folders are reported.
pub fn collect_files(watches: &[WatchRoot], filter: &IgnoreFilter) -> FileSample {
    let anchors = Anchors::from_watches(watches);
    let mut out = FileSample::default();
    'roots: for w in watches {
        if w.kind == RootKind::Network {
            out.skipped.push(w.path.clone());
            continue;
        }
        if !w.path.is_dir() {
            out.missing.push(w.path.clone());
            continue;
        }
        let depth = if w.recursive { usize::MAX } else { 1 };
        let walker = walkdir::WalkDir::new(&w.path)
            .min_depth(1)
            .max_depth(depth)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                !e.file_name().to_string_lossy().starts_with('.')
                    && !filter.in_excluded_dir(e.path())
            });
        for e in walker.filter_map(Result::ok) {
            if !e.file_type().is_file() || filter.is_ignored(e.path()) {
                continue;
            }
            if out.files.len() >= MAX_FILES {
                out.truncated = true;
                break 'roots;
            }
            let Ok(facts) = FileFacts::from_path(e.path()) else {
                continue;
            };
            let anchor = anchors
                .for_path(e.path())
                .unwrap_or(w.anchor())
                .to_path_buf();
            out.files.push((facts, anchor));
        }
    }
    out
}

/// Count matches of every enabled rule over `sample`. `exclusive`: a file
/// counts only for the first rule it matches (what would really happen);
/// otherwise for every rule it matches (wizard checklist, where rules are
/// picked independently).
pub fn match_files(rules: &RuleSet, sample: &FileSample, exclusive: bool) -> PreviewReport {
    let mut report = PreviewReport {
        rules: rules
            .rules()
            .iter()
            .map(|r| RulePreview {
                index: r.index,
                name: r.name.clone(),
                count: 0,
                examples: Vec::new(),
                sample_dest: None,
            })
            .collect(),
        scanned: sample.files.len(),
        truncated: sample.truncated,
        skipped: sample.skipped.clone(),
        missing: sample.missing.clone(),
        ..Default::default()
    };
    for (facts, anchor) in &sample.files {
        let mut hit = false;
        for (r, p) in rules.rules().iter().zip(report.rules.iter_mut()) {
            if !r.matches(facts) {
                continue;
            }
            hit = true;
            p.count += 1;
            if p.examples.len() < EXAMPLES {
                p.examples.push(facts.name.clone());
            }
            if p.sample_dest.is_none() {
                p.sample_dest = Some(r.render_dest_in(facts, Some(anchor)));
            }
            if exclusive {
                break;
            }
        }
        if !hit {
            report.unmatched += 1;
        }
    }
    report
}

/// [`collect_files`] + [`match_files`].
pub fn preview(
    rules: &RuleSet,
    watches: &[WatchRoot],
    filter: &IgnoreFilter,
    exclusive: bool,
) -> PreviewReport {
    match_files(rules, &collect_files(watches, filter), exclusive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nofilter() -> IgnoreFilter {
        IgnoreFilter::new(&[]).unwrap()
    }
    use crate::config::OrganizeMode;

    #[test]
    fn counts_independently_or_first_match() {
        let tmp = tempfile::tempdir().unwrap();
        let desk = tmp.path().join("바탕 화면");
        std::fs::create_dir_all(desk.join("하위")).unwrap();
        for n in ["스크린샷 1.png", "사진.png", "보고서.pdf", "메모.txt"] {
            std::fs::write(desk.join(n), "x").unwrap();
        }
        std::fs::write(desk.join("하위/깊은.pdf"), "x").unwrap();
        let rules = RuleSet::from_toml(
            r#"
[[rule]]
name = "스크린샷"
match = { ext = ["png"], name_regex = "^스크린샷" }
dest = "캡처"

[[rule]]
name = "꺼짐"
match = { ext = ["txt"] }
dest = "x"
enabled = false

[[rule]]
name = "이미지"
match = { ext = ["png"] }
dest = "{root}/이미지"

[[rule]]
name = "문서"
match = { ext = ["pdf"] }
dest = "문서"
"#,
        )
        .unwrap();
        let w = WatchRoot {
            path: desk.clone(),
            ..Default::default()
        };
        let all = preview(&rules, std::slice::from_ref(&w), &nofilter(), false);
        assert_eq!(all.scanned, 4, "not recursive: subfolder skipped");
        let counts: Vec<(usize, usize)> = all.rules.iter().map(|r| (r.index, r.count)).collect();
        assert_eq!(counts, [(0, 1), (2, 2), (3, 1)]);
        let first = preview(&rules, std::slice::from_ref(&w), &nofilter(), true);
        assert_eq!(first.rules[1].count, 1, "the screenshot went to rule 0");
        assert_eq!(first.unmatched, 1);
        // in_place: relative destinations under the watch folder.
        assert_eq!(first.rules[2].sample_dest, Some(desk.join("문서")));
        // collect: under organize_root, {root} too.
        let w2 = WatchRoot {
            recursive: true,
            mode: OrganizeMode::Collect,
            organize_root: Some(tmp.path().join("정리")),
            ..w
        };
        let c = preview(&rules, &[w2], &nofilter(), true);
        assert_eq!(c.scanned, 5);
        assert_eq!(c.rules[2].count, 2);
        assert_eq!(
            c.rules[1].sample_dest,
            Some(tmp.path().join("정리").join("이미지"))
        );
    }
}
