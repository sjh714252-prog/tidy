//! PDF via `pdf-extract` (pure Rust). Scanned PDFs yield no text; OCR is
//! a 2nd-phase item (SPEC §2.2).

use std::path::Path;

use super::{ExtractError, Extractor};

pub struct PdfExtractor;

impl Extractor for PdfExtractor {
    fn name(&self) -> &'static str {
        "pdf"
    }

    fn supports(&self, ext: &str) -> bool {
        ext == "pdf"
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let bytes = super::read_bytes(path)?;
        pdf_extract::extract_text_from_mem(&bytes).map_err(|e| ExtractError::format(path, e))
    }
}
