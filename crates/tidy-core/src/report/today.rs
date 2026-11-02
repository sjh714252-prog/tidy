//! (M13, SPEC §14.8) "오늘 한 일" on the home tab: today's moves from
//! `move_log`, with a folder moved as a whole (M12 `move_dir`) shown as one
//! item "(폴더 · 파일 N)". Read-only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::MoveLogEntry;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DoneItem {
    /// Unix seconds of the newest move in the item.
    pub at: i64,
    /// File name, or the moved folder's name.
    pub name: String,
    /// Where it was: the file, or the folder.
    pub src: PathBuf,
    /// Folder it went into (a folder item: the folder that now holds it).
    pub dest_dir: PathBuf,
    pub is_dir: bool,
    pub files: usize,
    pub batch_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TodaySummary {
    /// Files moved (and not undone).
    pub moved: usize,
    /// Files whose move was undone.
    pub undone: usize,
    /// Newest first, at most `limit`.
    pub items: Vec<DoneItem>,
}

/// Highest pair of folders (src side, dst side) under which the file kept
/// the same relative path — the folder that was moved as a whole.
fn moved_folder(src: &Path, dst: &Path) -> Option<(PathBuf, PathBuf)> {
    let (mut s, mut d) = (src.parent()?, dst.parent()?);
    let mut top = None;
    while let (Some(sn), Some(dn)) = (s.file_name(), d.file_name()) {
        if sn != dn {
            break;
        }
        top = Some((s.to_path_buf(), d.to_path_buf()));
        match (s.parent(), d.parent()) {
            (Some(a), Some(b)) => (s, d) = (a, b),
            _ => break,
        }
    }
    top
}

/// Summarize `entries` (from [`crate::store::Store::moves_since`]).
pub fn summarize(entries: &[MoveLogEntry], since: i64, limit: usize) -> TodaySummary {
    let mut out = TodaySummary::default();
    let active: Vec<&MoveLogEntry> = entries
        .iter()
        .filter(|e| !e.in_progress && e.moved_at >= since && e.undone_at.is_none())
        .collect();
    out.moved = active.len();
    out.undone = entries
        .iter()
        .filter(|e| e.undone_at.is_some_and(|u| u >= since))
        .count();
    // A folder counts as one item only when 2+ files of one batch share it.
    let mut groups: HashMap<(String, PathBuf, PathBuf), Vec<&MoveLogEntry>> = HashMap::new();
    for e in &active {
        if let Some((s, d)) = moved_folder(&e.src_path, &e.dst_path) {
            groups
                .entry((e.batch_id.clone(), s, d))
                .or_default()
                .push(e);
        }
    }
    let mut items: Vec<DoneItem> = Vec::new();
    let mut in_folder: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for ((batch, s, d), list) in &groups {
        if list.len() < 2 {
            continue;
        }
        in_folder.extend(list.iter().map(|e| e.id));
        items.push(DoneItem {
            at: list.iter().map(|e| e.moved_at).max().unwrap_or(0),
            name: s
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            src: s.clone(),
            dest_dir: d
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| d.clone()),
            is_dir: true,
            files: list.len(),
            batch_id: batch.clone(),
        });
    }
    for e in active.iter().filter(|e| !in_folder.contains(&e.id)) {
        items.push(DoneItem {
            at: e.moved_at,
            name: e
                .dst_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            src: e.src_path.clone(),
            dest_dir: e
                .dst_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            is_dir: false,
            files: 1,
            batch_id: e.batch_id.clone(),
        });
    }
    items.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.name.cmp(&b.name)));
    items.truncate(limit);
    out.items = items;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DecidedBy, MoveOp};

    fn e(id: i64, batch: &str, src: &str, dst: &str, at: i64, undone: Option<i64>) -> MoveLogEntry {
        MoveLogEntry {
            id,
            batch_id: batch.into(),
            src_path: PathBuf::from(src),
            dst_path: PathBuf::from(dst),
            hash: None,
            size: None,
            decided_by: DecidedBy::Rule,
            rule_name: None,
            ai_confidence: None,
            moved_at: at,
            undone_at: undone,
            in_progress: false,
            op: MoveOp::Move,
            dst_hash: None,
        }
    }

    #[test]
    fn folder_moves_are_one_item() {
        let list = vec![
            e(
                1,
                "b1",
                "/d/바탕 화면/가온누리원 로그/tools/a.go",
                "/정리/가온누리원/20261023_로그분석/개발/가온누리원 로그/tools/a.go",
                100,
                None,
            ),
            e(
                2,
                "b1",
                "/d/바탕 화면/가온누리원 로그/b.md",
                "/정리/가온누리원/20261023_로그분석/개발/가온누리원 로그/b.md",
                101,
                None,
            ),
            e(
                3,
                "b2",
                "/d/바탕 화면/보고서.docx",
                "/정리/문서/2026-10/보고서.docx",
                105,
                None,
            ),
            e(
                4,
                "b3",
                "/d/바탕 화면/x.png",
                "/정리/사진/x.png",
                90,
                Some(110),
            ),
            e(5, "b4", "/d/2026/a.pdf", "/정리/2026/a.pdf", 80, None),
        ];
        let t = summarize(&list, 50, 10);
        assert_eq!(t.moved, 4);
        assert_eq!(t.undone, 1);
        assert_eq!(t.items.len(), 3);
        assert_eq!(t.items[0].name, "보고서.docx");
        assert_eq!(t.items[0].dest_dir, PathBuf::from("/정리/문서/2026-10"));
        let f = &t.items[1];
        assert!(f.is_dir);
        assert_eq!(f.name, "가온누리원 로그");
        assert_eq!(f.files, 2);
        assert_eq!(
            f.dest_dir,
            PathBuf::from("/정리/가온누리원/20261023_로그분석/개발")
        );
        // A single file whose parent happens to match is still a file.
        assert!(!t.items[2].is_dir);
        assert_eq!(t.items[2].name, "a.pdf");
    }

    #[test]
    fn before_since_is_left_out() {
        let list = vec![e(1, "b", "/a/x.txt", "/b/x.txt", 10, None)];
        let t = summarize(&list, 50, 10);
        assert_eq!(t, TodaySummary::default());
    }
}
