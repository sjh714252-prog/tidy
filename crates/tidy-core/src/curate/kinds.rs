//! (0.16.0, SPEC §14.22) The kind of material a file is, offline: a fixed
//! table by extension, then the dictionary's kinds for extensions the
//! table does not know (the dictionary's are hints for the AI and too
//! narrow for every file — the starter maps every PDF to `안내문서`). Used where a customer is known but no AI judged the file —
//! it still goes into that customer's folder, under its kind
//! (`가람정보/문서/…`), so a customer's data is never left elsewhere.

use std::path::Path;

use super::ContextDict;

const TABLE: &[(&str, &[&str])] = &[
    (
        "문서",
        &["pdf", "hwp", "hwpx", "doc", "docx", "odt", "rtf", "pages"],
    ),
    ("발표", &["ppt", "pptx", "key", "odp"]),
    (
        "데이터",
        &[
            "xlsx", "xls", "csv", "tsv", "json", "xml", "yaml", "yml", "sql", "db", "sqlite",
        ],
    ),
    ("로그", &["log", "out"]),
    ("텍스트", &["txt", "md", "ini", "cfg", "conf"]),
    (
        "이미지",
        &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "svg", "tif", "tiff",
        ],
    ),
    ("동영상", &["mp4", "mov", "avi", "mkv", "wmv", "webm"]),
    ("음성", &["mp3", "wav", "m4a", "aac", "flac", "ogg"]),
    (
        "압축파일",
        &[
            "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "egg", "alz",
        ],
    ),
    (
        "설치파일",
        &["exe", "msi", "dmg", "pkg", "deb", "rpm", "apk", "jar"],
    ),
    (
        "코드",
        &[
            "go", "rs", "java", "kt", "py", "js", "ts", "tsx", "jsx", "c", "h", "cpp", "hpp", "cs",
            "sh", "bat", "ps1", "rb", "php", "swift", "html", "css", "vue",
        ],
    ),
    ("메일", &["eml", "msg"]),
];

/// The kind folder name for `path` ("자료" when nothing fits).
pub fn kind_of(dict: Option<&ContextDict>, path: &Path) -> String {
    let ext = crate::paths::ext_lower(path);
    if ext.is_empty() {
        return "자료".into();
    }
    if let Some((k, _)) = TABLE.iter().find(|(_, exts)| exts.contains(&ext.as_str())) {
        return (*k).to_string();
    }
    dict.and_then(|d| d.doc_kind_for_ext(&ext))
        .map(str::to_string)
        .unwrap_or_else(|| "자료".into())
}

/// Whether `name` is a kind folder's name (the table's, the dictionary's,
/// or `자료`), ignoring spacing and case.
pub fn is_kind_name(dict: Option<&ContextDict>, name: &str) -> bool {
    let k = super::existing::norm(name);
    if k.is_empty() {
        return false;
    }
    k == super::existing::norm("자료")
        || TABLE.iter().any(|(n, _)| super::existing::norm(n) == k)
        || dict.is_some_and(|d| {
            d.doc_kinds
                .iter()
                .any(|x| super::existing::norm(&x.name) == k)
        })
}

/// `{customer}/{kind}` under `anchor`, reusing a customer / kind folder
/// that is already there under a slightly different spelling. The bool
/// says whether the customer folder exists (a new customer folder is
/// asked about, never made silently — SPEC §14.3).
pub fn customer_kind_dir(anchor: &Path, customer: &str, kind: &str) -> (std::path::PathBuf, bool) {
    let cust = super::existing::reuse_existing(&anchor.join(super::clean(customer)), anchor, 0);
    let exists = cust.is_dir();
    let dest = super::existing::reuse_existing(&cust.join(super::clean(kind)), anchor, 0);
    (dest, exists)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_first_then_dictionary() {
        let mut d = ContextDict::starter();
        d.add_doc_kind("설계", &["drawio".into()]).unwrap();
        assert_eq!(kind_of(Some(&d), Path::new("a.log")), "로그");
        assert_eq!(kind_of(Some(&d), Path::new("a.PDF")), "문서");
        assert_eq!(kind_of(Some(&d), Path::new("a.drawio")), "설계");
        assert_eq!(kind_of(None, Path::new("a.pdf")), "문서");
        assert_eq!(kind_of(None, Path::new("main.go")), "코드");
        assert_eq!(kind_of(None, Path::new("noext")), "자료");
        assert_eq!(kind_of(None, Path::new("x.weird")), "자료");
    }
}
