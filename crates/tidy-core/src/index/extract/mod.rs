//! Text extractors (SPEC §5.5): `trait Extractor { supports(ext); extract(path) }`.
//! Pure Rust only; a failure never aborts indexing (metadata is kept).
//!
//! Adding one: implement [`Extractor`], register it in
//! [`Extractors::default_set`], and add a sample + expected text under
//! `tests/fixtures/` (CLAUDE.md).

mod html;
mod hwp;
mod office;
mod pdf;
mod text;

use std::path::{Path, PathBuf};

use thiserror::Error;

pub use html::HtmlExtractor;
pub use hwp::{testgen as hwp_testgen, HwpExtractor, HwpxExtractor};
pub use office::{DocxExtractor, PptxExtractor, XlsxExtractor};
pub use pdf::PdfExtractor;
pub use text::PlainTextExtractor;

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} looks like binary data")]
    Binary(PathBuf),
    #[error("{path}: {msg}")]
    Format { path: PathBuf, msg: String },
    /// (M14) Password-protected or distribution-only document: metadata only.
    #[error("{0}: 본문 추출 불가(암호)")]
    Encrypted(PathBuf),
    #[error("{path}: extractor panicked ({msg})")]
    Panic { path: PathBuf, msg: String },
}

impl ExtractError {
    fn format(path: &Path, msg: impl ToString) -> Self {
        ExtractError::Format {
            path: path.to_path_buf(),
            msg: msg.to_string(),
        }
    }
}

pub trait Extractor: Send + Sync {
    fn name(&self) -> &'static str;
    /// `ext` is lower-case without the dot (`""` for none).
    fn supports(&self, ext: &str) -> bool;
    fn extract(&self, path: &Path) -> Result<String, ExtractError>;
}

/// The registered extractors, asked in order.
pub struct Extractors {
    list: Vec<Box<dyn Extractor>>,
}

impl Default for Extractors {
    fn default() -> Self {
        Self::default_set()
    }
}

impl Extractors {
    pub fn default_set() -> Self {
        Self {
            list: vec![
                Box::new(PlainTextExtractor),
                Box::new(HtmlExtractor),
                Box::new(PdfExtractor),
                Box::new(DocxExtractor),
                Box::new(PptxExtractor),
                Box::new(XlsxExtractor),
                Box::new(HwpExtractor),
                Box::new(HwpxExtractor),
            ],
        }
    }

    pub fn empty() -> Self {
        Self { list: Vec::new() }
    }

    pub fn register(&mut self, e: Box<dyn Extractor>) {
        self.list.push(e);
    }

    pub fn for_ext(&self, ext: &str) -> Option<&dyn Extractor> {
        self.list
            .iter()
            .find(|e| e.supports(ext))
            .map(|e| e.as_ref())
    }

    pub fn supports(&self, ext: &str) -> bool {
        self.for_ext(ext).is_some()
    }

    /// `None` = no extractor for this type. Panics inside an extractor
    /// (third-party parsers on hostile input) are turned into errors.
    pub fn extract(&self, path: &Path) -> Option<Result<String, ExtractError>> {
        let ext = crate::paths::ext_lower(path);
        let e = self.for_ext(&ext)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| e.extract(path)));
        Some(match result {
            Ok(r) => r.map(|t| clean_text(&t)),
            Err(p) => Err(ExtractError::Panic {
                path: path.to_path_buf(),
                msg: panic_message(&p),
            }),
        })
    }
}

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

/// Normalise whitespace: trim lines, drop control characters, collapse
/// runs of blank lines. Keeps single newlines (snippets read better).
pub fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let line: String = line
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect();
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_run += 1;
            if blank_run == 1 && !out.is_empty() {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

/// Read a whole file into memory (extractors share this error mapping).
pub(crate) fn read_bytes(path: &Path) -> Result<Vec<u8>, ExtractError> {
    std::fs::read(path).map_err(|source| ExtractError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_collapses_blank_lines_and_spaces() {
        let t = clean_text("  a   b \n\n\n\n c\t d\u{0}\n");
        assert_eq!(t, "a b\n\nc d");
    }

    #[test]
    fn registry_picks_by_extension() {
        let x = Extractors::default_set();
        assert_eq!(x.for_ext("md").unwrap().name(), "text");
        assert_eq!(x.for_ext("docx").unwrap().name(), "docx");
        assert!(x.for_ext("exe").is_none());
        assert!(x.extract(Path::new("x.exe")).is_none());
    }
}
