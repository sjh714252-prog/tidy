//! Path helpers shared by several modules. Paths stay `PathBuf` end to end
//! (CLAUDE.md: no string concatenation for paths).

use std::path::{Component, Path, PathBuf};

/// Remove `.` and `..` components lexically, without touching the
/// filesystem (so it also works for folders that do not exist yet).
///
/// `..` pops the previous normal component; at the root of an absolute
/// path it is dropped (`/a/../../b` → `/b`, like the OS would resolve it),
/// and at the start of a relative path it is kept (`../x` stays `../x`).
/// Symlinks are not resolved: a rule's `dest = "../정리 결과"` is meant
/// lexically and this is what `move_log` and the CLI should show.
pub fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out: Vec<Component<'_>> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(c),
            },
            other => out.push(other),
        }
    }
    let mut buf = PathBuf::new();
    for c in out {
        buf.push(c.as_os_str());
    }
    if buf.as_os_str().is_empty() {
        buf.push(".");
    }
    buf
}

/// Lower-case extension without the dot (`""` if none).
pub fn ext_lower(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_dot_dot_in_the_middle() {
        let p = Path::new("/home/me/받은 파일/../정리 결과/2026/10");
        assert_eq!(
            normalize_lexical(p),
            Path::new("/home/me/정리 결과/2026/10")
        );
        assert_eq!(
            normalize_lexical(Path::new("/a/./b/./c")),
            Path::new("/a/b/c")
        );
    }

    #[test]
    fn root_and_relative_edge_cases() {
        assert_eq!(normalize_lexical(Path::new("/a/../../b")), Path::new("/b"));
        assert_eq!(normalize_lexical(Path::new("../x/../y")), Path::new("../y"));
        assert_eq!(normalize_lexical(Path::new("a/..")), Path::new("."));
        assert_eq!(normalize_lexical(Path::new("")), Path::new("."));
    }

    #[cfg(windows)]
    #[test]
    fn windows_prefixes_are_kept() {
        assert_eq!(
            normalize_lexical(Path::new(r"C:\Users\홍 길동\받은 파일\..\정리 결과")),
            PathBuf::from(r"C:\Users\홍 길동\정리 결과")
        );
        assert_eq!(
            normalize_lexical(Path::new(r"\\nas\공유\a\..\..\b")),
            PathBuf::from(r"\\nas\공유\b")
        );
    }

    #[test]
    fn ext() {
        assert_eq!(ext_lower(Path::new("a/보고서.HWP")), "hwp");
        assert_eq!(ext_lower(Path::new("a/README")), "");
    }
}
