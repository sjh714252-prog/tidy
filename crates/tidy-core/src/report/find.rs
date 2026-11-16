//! (0.18.0, SPEC §14.24) "어디로 갔지?": find files Tidy moved by name and
//! say where each one is **now**. `move_log` keeps where a file went at the
//! time; a later move of the file, or of a folder above it (M12
//! `move_dir`, 정리 위치 점검·새로 짜기), changes that. Here every row is
//! followed through the moves still in effect, so 찾기 opens the file where
//! it actually is. Read-only.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::store::{MoveLogEntry, MoveOp, Store, StoreError};

/// One file (or folder moved whole) Tidy moved, with where it is now.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FoundMove {
    pub move_id: i64,
    pub batch_id: String,
    /// File (or folder) name where it is now.
    pub name: String,
    /// Where it is now (after later moves; the original place if undone).
    pub now_path: PathBuf,
    /// Where it was before this move.
    pub src_path: PathBuf,
    /// Where this move put it.
    pub dst_path: PathBuf,
    pub moved_at: i64,
    /// rule | ai | user | restructure
    pub decided_by: String,
    /// Rule name, or the plan label ("정리 제안 #44").
    pub rule_name: Option<String>,
    /// This move was undone (the file went back to `src_path`).
    pub undone: bool,
    /// Moved again later (now_path differs from where this move put it).
    pub moved_again: bool,
    /// Encrypted into a vault zip (M9).
    pub encrypted: bool,
    pub exists: bool,
    pub is_dir: bool,
}

/// Moves still in effect, by source path (each list in id order).
pub struct Chain {
    by_src: HashMap<PathBuf, Vec<(i64, PathBuf)>>,
}

impl Chain {
    pub fn load(store: &Store) -> Result<Self, StoreError> {
        Ok(Self::from_rows(store.active_move_paths()?))
    }

    pub fn from_rows(rows: Vec<(i64, PathBuf, PathBuf)>) -> Self {
        let mut by_src: HashMap<PathBuf, Vec<(i64, PathBuf)>> = HashMap::new();
        for (id, src, dst) in rows {
            by_src.entry(src).or_default().push((id, dst));
        }
        Self { by_src }
    }

    /// Where `path` (as it was right after move `after_id`) is now: follow
    /// later moves of the path itself or of a folder above it.
    pub fn resolve(&self, path: &Path, after_id: i64) -> PathBuf {
        let mut cur = path.to_path_buf();
        let mut after = after_id;
        // A file moved back and forth cannot loop: ids only grow.
        for _ in 0..32 {
            let mut best: Option<(i64, PathBuf, PathBuf)> = None;
            for anc in cur.ancestors() {
                if anc.as_os_str().is_empty() {
                    break;
                }
                if let Some(list) = self.by_src.get(anc) {
                    if let Some((id, dst)) = list.iter().find(|(id, _)| *id > after) {
                        if best.as_ref().is_none_or(|(b, _, _)| id < b) {
                            best = Some((*id, anc.to_path_buf(), dst.clone()));
                        }
                    }
                }
            }
            let Some((id, anc, dst)) = best else { break };
            cur = match cur.strip_prefix(&anc) {
                Ok(rest) if !rest.as_os_str().is_empty() => dst.join(rest),
                _ => dst,
            };
            after = id;
        }
        cur
    }

    /// Where the file of a `move_log` row is now (`None` for trash/rmdir).
    pub fn now_path(&self, e: &MoveLogEntry) -> Option<PathBuf> {
        match e.op {
            MoveOp::Trash | MoveOp::RemoveDir => None,
            _ if e.in_progress => Some(e.dst_path.clone()),
            _ if e.undone_at.is_some() => Some(self.resolve(&e.src_path, e.id)),
            _ => Some(self.resolve(&e.dst_path, e.id)),
        }
    }
}

/// Split the query into words (spaces; quotes are just stripped).
fn words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|w| w.trim_matches('"').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Files Tidy moved whose path (before or after) has every word of `query`,
/// newest first, one entry per place a file is now. Empty query = the most
/// recently moved files ("최근 정리한 파일").
pub fn find_moved(store: &Store, query: &str, limit: usize) -> Result<Vec<FoundMove>, StoreError> {
    let ws = words(query);
    let refs: Vec<&str> = ws.iter().map(String::as_str).collect();
    // A file moved several times shows up once: read more rows than shown.
    let rows = store.moves_matching(&refs, limit.saturating_mul(4).max(40))?;
    let chain = Chain::load(store)?;
    let lower: Vec<String> = ws.iter().map(|w| w.to_lowercase()).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in rows {
        let Some(now) = chain.now_path(&e) else {
            continue;
        };
        if !seen.insert(now.clone()) {
            continue;
        }
        let meta = std::fs::metadata(&now).ok();
        let moved_again = e.undone_at.is_none() && now != e.dst_path;
        out.push(FoundMove {
            move_id: e.id,
            batch_id: e.batch_id.clone(),
            name: name_of(&now),
            now_path: now,
            src_path: e.src_path.clone(),
            dst_path: e.dst_path.clone(),
            moved_at: e.moved_at,
            decided_by: e.decided_by.as_str().to_string(),
            rule_name: e.rule_name.clone(),
            undone: e.undone_at.is_some(),
            moved_again,
            encrypted: e.op == MoveOp::Encrypt,
            exists: meta.is_some(),
            is_dir: meta.map(|m| m.is_dir()).unwrap_or(false),
        });
        if out.len() >= limit {
            break;
        }
    }
    // Name matches first (a word in a folder name matches every file in it).
    if !lower.is_empty() {
        out.sort_by_key(|f| {
            let n = f.name.to_lowercase();
            !lower.iter().all(|w| n.contains(w.as_str()))
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DecidedBy, NewMove};

    fn mv(store: &Store, batch: &str, src: &Path, dst: &Path) -> i64 {
        store
            .insert_move(&NewMove {
                batch_id: batch.into(),
                src_path: src.to_path_buf(),
                dst_path: dst.to_path_buf(),
                hash: None,
                size: None,
                decided_by: DecidedBy::Rule,
                rule_name: Some("문서".into()),
                ai_confidence: None,
                op: MoveOp::Move,
            })
            .unwrap()
    }

    #[test]
    fn follows_later_moves_of_the_file_and_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let desk = dir.path().join("바탕 화면");
        let org = dir.path().join("정리");
        // 1) 견적서 → 정리/문서, 2) 정리/문서 folder → 정리/가람정보/문서,
        // 3) 메모 → 정리/메모.txt then 4) moved again → 정리/기타/메모.txt
        mv(
            &store,
            "a",
            &desk.join("견적서.pdf"),
            &org.join("문서").join("견적서.pdf"),
        );
        mv(
            &store,
            "b",
            &org.join("문서"),
            &org.join("가람정보").join("문서"),
        );
        mv(&store, "c", &desk.join("메모.txt"), &org.join("메모.txt"));
        mv(
            &store,
            "d",
            &org.join("메모.txt"),
            &org.join("기타").join("메모.txt"),
        );

        let found = find_moved(&store, "견적서", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].now_path,
            org.join("가람정보").join("문서").join("견적서.pdf")
        );
        assert!(found[0].moved_again);

        let memo = find_moved(&store, "메모", 10).unwrap();
        // Both rows of 메모 end at the same place: shown once (newest row).
        assert_eq!(memo.len(), 1);
        assert_eq!(memo[0].now_path, org.join("기타").join("메모.txt"));
        assert_eq!(memo[0].src_path, org.join("메모.txt"));

        // Empty query = most recent moves, newest first.
        let recent = find_moved(&store, "", 10).unwrap();
        assert_eq!(recent[0].name, "메모.txt");
    }

    #[test]
    fn undone_move_points_back_and_like_wildcards_are_literal() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let src = dir.path().join("Desktop").join("report_100%.xlsx");
        let dst = dir.path().join("정리").join("report_100%.xlsx");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"x").unwrap();
        let id = mv(&store, "a", &src, &dst);
        store.mark_undone(id).unwrap();
        let f = find_moved(&store, "REPORT_100%", 10).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].undone);
        assert_eq!(f[0].now_path, src);
        assert!(f[0].exists);
        assert!(find_moved(&store, "report%x", 10).unwrap().is_empty());
        assert!(find_moved(&store, "report_1000", 10).unwrap().is_empty());
    }

    #[test]
    fn name_matches_come_before_folder_matches() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("t.db")).unwrap();
        let d = dir.path();
        mv(
            &store,
            "a",
            &d.join("가람 견적.pdf"),
            &d.join("o").join("가람 견적.pdf"),
        );
        mv(
            &store,
            "b",
            &d.join("x.log"),
            &d.join("o").join("가람").join("x.log"),
        );
        let f = find_moved(&store, "가람", 10).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].name, "가람 견적.pdf");
    }
}
