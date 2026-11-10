//! (0.13.0, SPEC §14.19) "정리 위치 다듬기": tidy up a tree Tidy already
//! built with the older, always-detailed layout. A kind or sub-topic folder
//! (`가람정보/20261020_과금/로그/`, `기타/개발/소스 예제/`) that holds fewer
//! than `min_items` files goes away: its files move one level up, into the
//! issue / big-topic folder — the layout new files get since 0.13.0.
//!
//! Only folders that are plainly Tidy's own layout are touched:
//! - exactly three levels below the organize root (customer/issue/kind or
//!   기타/topic/sub-topic), with no sub-folders;
//! - every file in it was put there by Tidy (an active `move_log` entry)
//!   and none came as part of a whole folder of the same name (a folder
//!   item keeps its inside as it was — never flattened).
//!
//! Nothing moves here: the result is an ordinary plan (preview, tick,
//! apply as one undoable batch; emptied folders are removed).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::restructure::{Plan, PlanSource, PlannedMove};
use crate::mover::TMP_MARKER;
use crate::store::Store;

/// Most folders folded in one plan.
const MAX_FOLDERS: usize = 2_000;

fn sub_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.starts_with('.') && !n.contains(TMP_MARKER)
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// The files of a small layout folder that can move up, or `None` when
/// the folder must stay as it is.
fn foldable(dir: &Path, min_items: usize, store: &Store) -> Option<Vec<(PathBuf, u64)>> {
    let rd = std::fs::read_dir(dir).ok()?;
    let name = dir.file_name()?.to_os_string();
    let mut files = Vec::new();
    for e in rd.filter_map(Result::ok) {
        let t = e.file_type().ok()?;
        if !t.is_file() {
            return None; // a sub-folder (or link): not a plain kind folder
        }
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with('.') || n.contains(TMP_MARKER) || n.eq_ignore_ascii_case("desktop.ini") {
            return None;
        }
        // Tidy put it here, on its own (not inside a moved folder of the
        // same name).
        let m = store.active_move_to(&p).ok()??;
        if m.src_path.parent().and_then(Path::file_name) == Some(name.as_os_str()) {
            return None;
        }
        files.push((p, e.metadata().map(|m| m.len()).unwrap_or(0)));
        if files.len() >= min_items {
            return None;
        }
    }
    (!files.is_empty()).then_some(files)
}

/// A plan folding the small layout folders under `root` (an organize
/// root, or a watch folder organized in place). `None` when there is
/// nothing to fold. Nothing moves.
pub fn plan_compact(root: &Path, min_items: usize, store: &Store) -> Option<Plan> {
    if min_items <= 1 || !root.is_dir() {
        return None;
    }
    let mut moves: Vec<PlannedMove> = Vec::new();
    let mut folded: Vec<String> = Vec::new();
    'outer: for top in sub_dirs(root) {
        for mid in sub_dirs(&top) {
            for leaf in sub_dirs(&mid) {
                if folded.len() >= MAX_FOLDERS {
                    break 'outer;
                }
                let Some(files) = foldable(&leaf, min_items, store) else {
                    continue;
                };
                let rel = leaf
                    .strip_prefix(root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                for (p, bytes) in files {
                    let Some(n) = p.file_name() else { continue };
                    let to = mid.join(n);
                    let mut warnings = Vec::new();
                    if to.exists() {
                        warnings
                            .push("위 폴더에 같은 이름이 있어 '(1)'처럼 번호가 붙습니다".into());
                    }
                    moves.push(PlannedMove {
                        id: moves.len() as u32 + 1,
                        from: p,
                        to,
                        is_dir: false,
                        files: 1,
                        bytes,
                        reason: format!(
                            "파일이 {min_items}개보다 적은 폴더 '{rel}' → 한 단계 위로"
                        ),
                        warnings,
                        context: None,
                        encrypt: false,
                    });
                }
                folded.push(rel);
            }
        }
    }
    if moves.is_empty() {
        return None;
    }
    let shown: Vec<&str> = folded.iter().take(5).map(String::as_str).collect();
    Some(Plan {
        id: 0,
        root: root.to_path_buf(),
        source: PlanSource::Compact,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        summary: format!(
            "정리 위치 다듬기: 파일이 {min_items}개보다 적은 종류·작은 주제 폴더 {}곳({}{})의 파일 {}개를 한 단계 위(이슈·주제 폴더)로 올리고, 빈 폴더는 지웁니다. Tidy가 넣은 파일만 다루며 통째로 옮긴 폴더 안은 건드리지 않습니다.",
            folded.len(),
            shown.join(", "),
            if folded.len() > shown.len() { " …" } else { "" },
            moves.len(),
        ),
        folders: Vec::new(),
        moves,
        rejected: Vec::new(),
        applied_batch: None,
        anchor: None,
        sources: Vec::new(),
    })
}

/// Distinct folders results go to (organize roots and in-place watch
/// folders) — where [`plan_compact`] looks.
pub fn result_roots(cfg: &crate::config::Config) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for w in &cfg.watches {
        let a = w.anchor().to_path_buf();
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}
