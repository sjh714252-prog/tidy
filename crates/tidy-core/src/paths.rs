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

/// Best-effort guess whether `p` is on a network share (SPEC §5.9: the
/// user marks roots `kind = "network"`; this only feeds warnings and the
/// default of the app's "add folder"). UNC paths, Windows remote drives,
/// and Linux/macOS mounts of SMB/NFS/AFP/WebDAV/SSHFS count as network.
pub fn looks_like_network(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if is_unc(&s) {
        return true;
    }
    platform_is_network(p)
}

fn is_unc(s: &str) -> bool {
    let unc = |rest: &str| !rest.is_empty() && !rest.starts_with(['\\', '/', '?', '.']);
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return !rest.is_empty();
    }
    if let Some(rest) = s.strip_prefix(r"\\").or_else(|| s.strip_prefix("//")) {
        return unc(rest);
    }
    false
}

/// File system types of network mounts (Linux `mountinfo`, macOS `statfs`).
#[cfg(any(target_os = "linux", target_os = "macos"))]
const NETWORK_FS: &[&str] = &[
    "cifs",
    "smb3",
    "smbfs",
    "nfs",
    "nfs4",
    "afpfs",
    "webdav",
    "fuse.sshfs",
    "sshfs",
    "9p",
    "davfs",
    "fuse.davfs2",
    "ncpfs",
    "fuse.rclone",
];

#[cfg(target_os = "linux")]
fn platform_is_network(p: &Path) -> bool {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let target = dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    mount_fs_type(&info, &target).is_some_and(|t| NETWORK_FS.contains(&t.as_str()))
}

/// File system type of the longest mount point containing `target`, from
/// `/proc/self/mountinfo` text (fields: `... mount_point ... - fstype src`).
#[cfg(any(target_os = "linux", test))]
fn mount_fs_type(mountinfo: &str, target: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let (Some(mount_point), Some(fstype)) = (pre.split(' ').nth(4), post.split(' ').next())
        else {
            continue;
        };
        let mount_point = mount_point.replace("\\040", " ");
        let fstype = fstype.to_string();
        let mp = Path::new(&mount_point);
        if target.starts_with(mp) {
            let len = mp.components().count();
            if best.as_ref().is_none_or(|(l, _)| len >= *l) {
                best = Some((len, fstype));
            }
        }
    }
    best.map(|(_, t)| t)
}

#[cfg(target_os = "macos")]
fn platform_is_network(p: &Path) -> bool {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    let target = dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let Ok(c) = CString::new(target.as_os_str().as_bytes()) else {
        return false;
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path, `st` a writable statfs.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    // SAFETY: f_fstypename is a NUL-terminated C string filled by statfs.
    let name = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
    NETWORK_FS.contains(&name.to_string_lossy().as_ref())
}

#[cfg(windows)]
fn platform_is_network(p: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Prefix;
    let Some(Component::Prefix(prefix)) = p.components().next() else {
        return false;
    };
    let letter = match prefix.kind() {
        Prefix::Disk(l) | Prefix::VerbatimDisk(l) => l,
        Prefix::UNC(..) | Prefix::VerbatimUNC(..) => return true,
        _ => return false,
    };
    let root: Vec<u16> = std::ffi::OsStr::new(&format!("{}:\\", letter as char))
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: `root` is a NUL-terminated UTF-16 string.
    let kind = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) };
    // DRIVE_REMOTE (WindowsProgramming; not worth another feature flag).
    const DRIVE_REMOTE: u32 = 4;
    kind == DRIVE_REMOTE
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_is_network(_: &Path) -> bool {
    false
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
    fn unc_paths_look_like_network() {
        assert!(looks_like_network(Path::new(r"\\nas\공유 폴더\업무")));
        assert!(looks_like_network(Path::new("//nas/공유")));
        assert!(looks_like_network(Path::new(r"\\?\UNC\nas\공유")));
        assert!(!looks_like_network(Path::new(r"\\?\C:\Users")));
    }

    #[test]
    fn mountinfo_longest_prefix_wins() {
        let info = "22 1 8:1 / / rw - ext4 /dev/sda1 rw\n\
                    40 22 0:50 / /mnt/nas\\040공유 rw - cifs //nas/share rw\n\
                    41 40 0:51 / /mnt/nas\\040공유/로컬 rw - ext4 /dev/sdb1 rw\n";
        assert_eq!(
            mount_fs_type(info, Path::new("/mnt/nas 공유/업무")).as_deref(),
            Some("cifs")
        );
        assert_eq!(
            mount_fs_type(info, Path::new("/mnt/nas 공유/로컬/x")).as_deref(),
            Some("ext4")
        );
        assert_eq!(
            mount_fs_type(info, Path::new("/home/me")).as_deref(),
            Some("ext4")
        );
    }

    #[test]
    fn ext() {
        assert_eq!(ext_lower(Path::new("a/보고서.HWP")), "hwp");
        assert_eq!(ext_lower(Path::new("a/README")), "");
    }
}
