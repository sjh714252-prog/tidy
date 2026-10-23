//! (M12, SPEC §14.7) "고객사별로 찾기": where each customer's material is
//! (its folder under every watch folder's anchor) and how much is there.
//! Read-only.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use super::{clean, ContextDict};
use crate::config::Config;
use crate::store::Store;

/// Files counted per customer at most.
pub const CUSTOMER_SCAN_CAP: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomerStat {
    pub name: String,
    /// Existing customer folders (one per watch folder anchor).
    pub dirs: Vec<PathBuf>,
    /// Folders directly in them (issue / date folders).
    pub issues: usize,
    pub files: usize,
    /// Newest file's mtime (Unix seconds).
    pub last: Option<i64>,
    pub capped: bool,
}

/// Distinct anchors (where organized results go) of the watch folders.
fn anchors(cfg: &Config) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for w in &cfg.watches {
        let a = w.anchor().to_path_buf();
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// A customer's folders that exist (search filter "고객사").
pub fn customer_dirs(cfg: &Config, name: &str) -> Vec<PathBuf> {
    let n = clean(name);
    if n.is_empty() {
        return Vec::new();
    }
    anchors(cfg)
        .into_iter()
        .map(|a| a.join(&n))
        .filter(|d| d.is_dir())
        .collect()
}

/// Customer names: the dictionary, then any the cache knows.
pub fn customer_names(dict: &ContextDict, store: Option<&Store>) -> Vec<String> {
    let mut out: Vec<String> = dict.entities.iter().map(|e| e.name.clone()).collect();
    if let Some(s) = store {
        for (e, _) in s.curation_groups().unwrap_or_default() {
            if !out.iter().any(|x| clean(x) == clean(&e)) {
                out.push(e);
            }
        }
    }
    out
}

/// Every customer with a folder, newest work first.
pub fn customer_overview(
    cfg: &Config,
    dict: &ContextDict,
    store: Option<&Store>,
) -> Vec<CustomerStat> {
    let mut out = Vec::new();
    for name in customer_names(dict, store) {
        let dirs = customer_dirs(cfg, &name);
        if dirs.is_empty() {
            continue;
        }
        let mut st = CustomerStat {
            name,
            dirs: dirs.clone(),
            issues: 0,
            files: 0,
            last: None,
            capped: false,
        };
        for d in &dirs {
            st.issues += std::fs::read_dir(d)
                .map(|rd| {
                    rd.filter_map(Result::ok)
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .count()
                })
                .unwrap_or(0);
            for e in walkdir::WalkDir::new(d)
                .min_depth(1)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !e.file_type().is_file() {
                    continue;
                }
                if st.files >= CUSTOMER_SCAN_CAP {
                    st.capped = true;
                    break;
                }
                st.files += 1;
                if let Some(t) = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                {
                    st.last = Some(st.last.map_or(t, |l| l.max(t)));
                }
            }
        }
        out.push(st);
    }
    out.sort_by(|a, b| b.last.cmp(&a.last).then(a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OrganizeMode, WatchRoot};

    #[test]
    fn customers_under_every_anchor() {
        let tmp = tempfile::tempdir().unwrap();
        let desk = tmp.path().join("바탕 화면");
        let org = tmp.path().join("정리");
        let a = org
            .join("가온누리원")
            .join("20261023_로그분석")
            .join("개발");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("x.go"), "x").unwrap();
        std::fs::create_dir_all(desk.join("가온누리원").join("20260901_장애")).unwrap();
        std::fs::write(
            desk.join("가온누리원").join("20260901_장애").join("y.log"),
            "y",
        )
        .unwrap();
        let cfg = Config {
            watches: vec![
                WatchRoot {
                    path: desk.clone(),
                    ..Default::default()
                },
                WatchRoot {
                    path: tmp.path().join("다운로드"),
                    mode: OrganizeMode::Collect,
                    organize_root: Some(org.clone()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut dict = ContextDict::starter();
        dict.add_entity("가온누리원", &[]).unwrap();
        dict.add_entity("가람정보", &[]).unwrap();
        let v = customer_overview(&cfg, &dict, None);
        assert_eq!(v.len(), 1, "no folder, no entry");
        assert_eq!(v[0].name, "가온누리원");
        assert_eq!((v[0].dirs.len(), v[0].issues, v[0].files), (2, 2, 2));
        assert!(v[0].last.is_some());
        assert_eq!(customer_dirs(&cfg, "가온누리원").len(), 2);
        assert!(customer_dirs(&cfg, "가람정보").is_empty());
    }
}
