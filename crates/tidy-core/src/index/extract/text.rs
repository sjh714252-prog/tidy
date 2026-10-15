//! Plain text and source code. Handles the encodings Korean Windows
//! produces: UTF-8 (with/without BOM), UTF-16 with BOM, and EUC-KR/CP949.

use std::path::Path;

use super::{read_bytes, ExtractError, Extractor};

pub struct PlainTextExtractor;

pub const TEXT_EXTS: &[&str] = &[
    // documents / data
    "txt",
    "text",
    "md",
    "markdown",
    "csv",
    "tsv",
    "json",
    "jsonl",
    "ndjson",
    "xml",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "properties",
    "log",
    "tex",
    "srt",
    "vtt",
    "rst",
    "org",
    "nfo",
    "env",
    "gitignore",
    "editorconfig",
    // source code
    "rs",
    "py",
    "pyi",
    "js",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "jsx",
    "java",
    "kt",
    "kts",
    "c",
    "cc",
    "cpp",
    "cxx",
    "h",
    "hh",
    "hpp",
    "cs",
    "go",
    "rb",
    "php",
    "swift",
    "m",
    "mm",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    "sql",
    "css",
    "scss",
    "sass",
    "less",
    "lua",
    "pl",
    "pm",
    "r",
    "dart",
    "scala",
    "groovy",
    "vue",
    "svelte",
    "gradle",
    "cmake",
    "mk",
    "make",
    "proto",
    "graphql",
    "gql",
    "vb",
    "vbs",
    "asm",
    "s",
    "f90",
    "jl",
    "ex",
    "exs",
    "erl",
    "hs",
    "clj",
    "elm",
    "nim",
    "zig",
    "v",
    "sol",
];

impl Extractor for PlainTextExtractor {
    fn name(&self) -> &'static str {
        "text"
    }

    fn supports(&self, ext: &str) -> bool {
        TEXT_EXTS.contains(&ext)
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let bytes = read_bytes(path)?;
        decode(&bytes).ok_or_else(|| ExtractError::Binary(path.to_path_buf()))
    }
}

/// Decode text bytes; `None` if the data looks binary.
pub fn decode(bytes: &[u8]) -> Option<String> {
    // BOMs first.
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Some(String::from_utf8_lossy(rest).into_owned());
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let (text, _, _) = encoding_rs::UTF_16LE.decode(bytes); // BOM-sniffing picks LE/BE
        return Some(text.into_owned());
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (!looks_binary(bytes)).then(|| s.to_string());
    }
    if looks_binary(bytes) {
        return None;
    }
    // Korean Windows default (CP949 is a superset of EUC-KR; the WHATWG
    // "euc-kr" decoder in encoding_rs is windows-949).
    let (text, _, had_errors) = encoding_rs::EUC_KR.decode(bytes);
    if had_errors {
        // Not CP949 either; keep what is readable rather than nothing.
        return Some(String::from_utf8_lossy(bytes).into_owned());
    }
    Some(text.into_owned())
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf8_bom_utf16_and_cp949() {
        assert_eq!(decode("한글 메모".as_bytes()).unwrap(), "한글 메모");
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("표".as_bytes());
        assert_eq!(decode(&bom).unwrap(), "표");

        let mut le = vec![0xFF, 0xFE];
        le.extend("가나다".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode(&le).unwrap(), "가나다");

        let (kr, _, _) = encoding_rs::EUC_KR.encode("과금 내역서 2026");
        assert_eq!(decode(&kr).unwrap(), "과금 내역서 2026");

        assert!(decode(&[b'a', 0, b'b']).is_none());
    }
}
