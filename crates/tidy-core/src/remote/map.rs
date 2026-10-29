//! (M18) NAS paths ↔ the paths this PC sees for them (`[[nas.map]]`), so a
//! NAS search result can be opened here.

use std::path::{Path, PathBuf};

use crate::config::PathMapping;

/// Components of a NAS (Unix) path, without empty ones.
fn parts(p: &str) -> Vec<&str> {
    p.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect()
}

/// The PC path for NAS path `remote`, by the longest matching `remote`
/// prefix; `None` without a match.
pub fn to_local(maps: &[PathMapping], remote: &str) -> Option<PathBuf> {
    let target = parts(remote);
    let (m, n) = maps
        .iter()
        .filter(|m| !m.remote.trim().is_empty() && !m.local.trim().is_empty())
        .filter_map(|m| {
            let pre = parts(m.remote.trim());
            target.starts_with(&pre).then_some((m, pre.len()))
        })
        .max_by_key(|(_, n)| *n)?;
    let mut out = PathBuf::from(m.local.trim());
    for c in &target[n..] {
        out.push(c);
    }
    Some(out)
}

/// The NAS path for a PC path (the reverse, for "폴더 고르기" defaults).
pub fn to_remote(maps: &[PathMapping], local: &Path) -> Option<String> {
    let (m, rest) = maps
        .iter()
        .filter(|m| !m.remote.trim().is_empty() && !m.local.trim().is_empty())
        .filter_map(|m| {
            local
                .strip_prefix(Path::new(m.local.trim()))
                .ok()
                .map(|r| (m, r.to_path_buf()))
        })
        .max_by_key(|(m, _)| m.local.len())?;
    let mut out = m.remote.trim().trim_end_matches('/').to_string();
    for c in rest.components() {
        out.push('/');
        out.push_str(&c.as_os_str().to_string_lossy());
    }
    Some(out)
}

/// Host name of a NAS address (`http://192.168.0.10:7841` → `192.168.0.10`).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().split("://").last()?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// Default PC path of a NAS folder: the first folder under the data root
/// is taken as the shared folder's name (the setup guide mounts each share
/// at `/data/<share>`). Windows: `\\host\share`, macOS: `/Volumes/share`.
/// `None` elsewhere or when the folder is the data root itself.
pub fn guess(url: &str, data_root: &str, remote: &str) -> Option<PathMapping> {
    let root = parts(data_root);
    let target = parts(remote);
    if !target.starts_with(&root) || target.len() <= root.len() {
        return None;
    }
    let share = target[root.len()];
    let mut prefix = String::new();
    for c in &target[..=root.len()] {
        prefix.push('/');
        prefix.push_str(c);
    }
    let local = if cfg!(windows) {
        format!(r"\\{}\{}", host_of(url)?, share)
    } else if cfg!(target_os = "macos") {
        format!("/Volumes/{share}")
    } else {
        return None;
    };
    Some(PathMapping {
        remote: prefix,
        local,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(remote: &str, local: &str) -> PathMapping {
        PathMapping {
            remote: remote.into(),
            local: local.into(),
        }
    }

    #[test]
    fn longest_prefix_wins() {
        let maps = [m("/data", "/mnt/nas"), m("/data/업무", "/mnt/work")];
        assert_eq!(
            to_local(&maps, "/data/업무/고객/a.pdf"),
            Some(PathBuf::from("/mnt/work").join("고객").join("a.pdf"))
        );
        assert_eq!(
            to_local(&maps, "/data/사진/b.jpg"),
            Some(PathBuf::from("/mnt/nas").join("사진").join("b.jpg"))
        );
        // Component-wise: "/data/업무2" is not under "/data/업무".
        assert_eq!(
            to_local(&maps, "/data/업무2/c"),
            Some(PathBuf::from("/mnt/nas").join("업무2").join("c"))
        );
        assert_eq!(to_local(&maps, "/other/x"), None);
        assert_eq!(to_local(&[], "/data/x"), None);
    }

    #[test]
    fn reverse_mapping() {
        let maps = [m("/data/업무", "/mnt/work")];
        assert_eq!(
            to_remote(&maps, &PathBuf::from("/mnt/work").join("고객")),
            Some("/data/업무/고객".into())
        );
        assert_eq!(to_remote(&maps, Path::new("/elsewhere")), None);
    }

    #[test]
    fn host_from_address() {
        assert_eq!(
            host_of("http://192.168.0.10:7841").as_deref(),
            Some("192.168.0.10")
        );
        assert_eq!(
            host_of("https://nas.local/tidy").as_deref(),
            Some("nas.local")
        );
        assert_eq!(host_of("nas:7841").as_deref(), Some("nas"));
        assert_eq!(host_of("http://[fe80::1]:7841").as_deref(), Some("fe80::1"));
    }

    #[test]
    fn guess_share_path() {
        let g = guess("http://nas:7841", "/data", "/data/업무/받은 파일");
        if cfg!(windows) {
            let g = g.unwrap();
            assert_eq!(g.remote, "/data/업무");
            assert_eq!(g.local, r"\\nas\업무");
        } else if cfg!(target_os = "macos") {
            assert_eq!(g.unwrap().local, "/Volumes/업무");
        } else {
            assert!(g.is_none());
        }
        assert!(guess("http://nas", "/data", "/data").is_none());
        assert!(guess("http://nas", "/data", "/elsewhere/x").is_none());
    }
}
