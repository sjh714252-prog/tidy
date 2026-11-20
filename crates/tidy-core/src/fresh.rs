//! (0.19.0, SPEC §14.29) Files the user just made to write in — "새 텍스트
//! 문서.txt" from Explorer's 새로 만들기, an empty file, a file created and
//! then edited — are never moved on their own while watching. An empty
//! file is left alone; once it has content it is *asked about* in 확인
//! 대기 instead of being moved. Downloads (Windows: the "Mark of the Web"
//! stream), copies (modified before they were created) and unpacked files
//! are not affected.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// The file was created at least this long before its last change: it was
/// made, then written to (an editor), not dropped in finished.
pub const EDITED_AFTER: Duration = Duration::from_secs(30);
/// Only files created this recently count as "just made".
pub const RECENT: Duration = Duration::from_secs(7 * 24 * 3600);

/// Explorer / Office / Hancom "new file" names (Korean and English Windows,
/// macOS / Linux "Untitled"), with or without " (2)".
pub fn is_blank_template(name: &str) -> bool {
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    };
    let stem = strip_counter(stem.trim());
    let lower = stem.to_lowercase();
    if let Some(rest) = stem.strip_prefix("새 ") {
        return [
            "문서",
            "워크시트",
            "프레젠테이션",
            "이미지",
            "통합 문서",
            "텍스트",
        ]
        .iter()
        .any(|w| rest.contains(w));
    }
    if let Some(rest) = lower.strip_prefix("new ") {
        return ["document", "worksheet", "presentation", "image", "text"]
            .iter()
            .any(|w| rest.contains(w));
    }
    matches!(
        lower.as_str(),
        "untitled" | "제목 없음" | "제목없음" | "이름 없음" | "noname" | "noname1"
    ) || lower.starts_with("untitled-")
        || lower.starts_with("untitled ")
}

/// "새 텍스트 문서 (2)" → "새 텍스트 문서".
fn strip_counter(stem: &str) -> &str {
    if let Some(open) = stem.rfind(" (") {
        let tail = &stem[open + 2..];
        if let Some(num) = tail.strip_suffix(')') {
            if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                return &stem[..open];
            }
        }
    }
    stem
}

/// Created recently and written to well after it was created — the user is
/// working on it. Only on Windows, where the creation time survives an
/// editor's save (file tunnelling) and downloads carry a Zone.Identifier
/// stream (a slow browser download also looks "created, then written").
pub fn authored_recently(path: &Path, meta: &std::fs::Metadata, now: SystemTime) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let (Ok(created), Ok(modified)) = (meta.created(), meta.modified()) else {
        return false;
    };
    let edited = modified
        .duration_since(created)
        .is_ok_and(|d| d >= EDITED_AFTER);
    let recent = now.duration_since(created).is_ok_and(|d| d <= RECENT);
    edited && recent && !downloaded(path)
}

/// The file carries Windows' "Mark of the Web" (browser / mail download).
#[cfg(windows)]
pub fn downloaded(path: &Path) -> bool {
    let mut s = path.as_os_str().to_os_string();
    s.push(":Zone.Identifier");
    std::fs::metadata(std::path::PathBuf::from(s)).is_ok()
}

#[cfg(not(windows))]
pub fn downloaded(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_names() {
        for n in [
            "새 텍스트 문서.txt",
            "새 텍스트 문서 (2).txt",
            "새 Microsoft Word 문서.docx",
            "새 Microsoft Excel 워크시트.xlsx",
            "새 Microsoft PowerPoint 프레젠테이션.pptx",
            "새 비트맵 이미지.bmp",
            "새 서식 있는 텍스트 문서.rtf",
            "새 한컴오피스 한글 문서.hwp",
            "New Text Document.txt",
            "New Microsoft Word Document (3).docx",
            "Untitled.txt",
            "Untitled-1.psd",
            "제목 없음.txt",
        ] {
            assert!(is_blank_template(n), "{n}");
        }
        for n in [
            "회의록.txt",
            "새 고객사 제안서.pdf",
            "새로운 견적.xlsx",
            "News letter.pdf",
            "전송자격인증제 시행에 따른 발신번호 조치 방안.docx",
        ] {
            assert!(!is_blank_template(n), "{n}");
        }
    }

    #[test]
    fn counter_only_digits() {
        assert_eq!(strip_counter("새 텍스트 문서 (12)"), "새 텍스트 문서");
        assert_eq!(strip_counter("보고서 (최종)"), "보고서 (최종)");
    }
}
