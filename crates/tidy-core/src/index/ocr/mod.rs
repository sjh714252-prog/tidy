//! (M15) Text recognition of images and scanned PDFs (SPEC §14.11).
//!
//! The indexer only marks targets (`files.ocr = pending`, or fills them
//! from `ocr_cache` by content hash); [`run_pass`] recognizes them in the
//! background and stores the text as the file's body, so word search,
//! semantic search and the personal-information check all see it.
//!
//! Engines are local and offline: Windows' built-in OCR on Windows,
//! tesseract (+ poppler `pdftoppm` for PDFs) elsewhere. Without an engine
//! nothing changes and the files simply keep waiting (CLAUDE.md rule 4).
//! Nothing here writes to user folders.

mod tesseract;
#[cfg(windows)]
mod winrt;

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::config::{Config, OcrCfg, OcrEngineKind};
use crate::store::{OcrCounts, Store, StoreError, TextStatus};

pub use tesseract::TesseractEngine;

/// Image types the engines read.
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp"];
/// Smaller images are icons and the like.
pub const MIN_IMAGE_BYTES: u64 = 4096;
/// A PDF with fewer non-blank characters than this is treated as scanned.
pub const SCANNED_PDF_CHARS: usize = 30;
/// Files recognized per pass of the background loop.
pub const OCR_PASS: usize = 16;

#[derive(Debug, Error)]
pub enum OcrError {
    /// No usable engine (not installed, wrong OS): files keep waiting.
    #[error("{0}")]
    Unavailable(String),
    /// This file could not be read.
    #[error("{0}")]
    File(String),
}

pub trait OcrEngine: Send + Sync {
    /// `windows` | `tesseract` (stored in `ocr_cache.engine`).
    fn name(&self) -> &'static str;
    /// For people: "Windows 내장 OCR · 한국어".
    fn describe(&self) -> String;
    /// A hint shown next to the status (e.g. a missing language pack).
    fn note(&self) -> Option<String> {
        None
    }
    fn reads_pdf(&self) -> bool;
    fn image(&self, path: &Path) -> Result<String, OcrError>;
    fn pdf(&self, path: &Path, max_pages: u32) -> Result<String, OcrError>;
}

/// Should the indexer mark this file for recognition?
pub fn is_target(
    ext: &str,
    size: u64,
    status: TextStatus,
    content: Option<&str>,
    max_image_bytes: u64,
) -> bool {
    if IMAGE_EXTS.contains(&ext) {
        return status == TextStatus::Unsupported
            && (MIN_IMAGE_BYTES..=max_image_bytes).contains(&size);
    }
    ext == "pdf"
        && match status {
            TextStatus::Failed => true,
            TextStatus::Ok => {
                content
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .count()
                    < SCANNED_PDF_CHARS
            }
            _ => false,
        }
}

/// The configured engine, or why there is none.
pub fn build_engine(cfg: &OcrCfg) -> Result<Box<dyn OcrEngine>, OcrError> {
    match cfg.engine {
        OcrEngineKind::Tesseract => Ok(Box::new(TesseractEngine::find(cfg)?)),
        OcrEngineKind::Windows => windows_engine(cfg),
        OcrEngineKind::Auto => {
            if cfg!(windows) {
                match windows_engine(cfg) {
                    Ok(e) => Ok(e),
                    // An installed tesseract still beats nothing.
                    Err(we) => TesseractEngine::find(cfg)
                        .map(|t| Box::new(t) as Box<dyn OcrEngine>)
                        .map_err(|_| we),
                }
            } else {
                Ok(Box::new(TesseractEngine::find(cfg)?))
            }
        }
    }
}

#[cfg(windows)]
fn windows_engine(cfg: &OcrCfg) -> Result<Box<dyn OcrEngine>, OcrError> {
    Ok(Box::new(winrt::WindowsEngine::new(cfg)?))
}

#[cfg(not(windows))]
fn windows_engine(_cfg: &OcrCfg) -> Result<Box<dyn OcrEngine>, OcrError> {
    Err(OcrError::Unavailable(
        "Windows 내장 OCR은 Windows에서만 쓸 수 있습니다".into(),
    ))
}

/// Recognize one file (dispatch by type) and tidy the text.
pub fn recognize(engine: &dyn OcrEngine, path: &Path, cfg: &OcrCfg) -> Result<String, OcrError> {
    let ext = crate::paths::ext_lower(path);
    let raw = if ext == "pdf" {
        engine.pdf(path, cfg.pdf_max_pages.max(1))?
    } else {
        engine.image(path)?
    };
    Ok(normalize(&raw))
}

/// Clean whitespace and join Hangul that came out one syllable at a time
/// ("견 적 서" → "견적서"), which trigram search could not match.
pub fn normalize(text: &str) -> String {
    let cleaned = crate::index::extract::clean_text(text);
    cleaned
        .lines()
        .map(join_spaced_hangul)
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_hangul_syllable(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

fn join_spaced_hangul(line: &str) -> String {
    let tokens: Vec<&str> = line.split(' ').filter(|t| !t.is_empty()).collect();
    let single = |t: &str| {
        let mut cs = t.chars();
        matches!((cs.next(), cs.next()), (Some(c), None) if is_hangul_syllable(c))
    };
    let n_single = tokens.iter().filter(|t| single(t)).count();
    if tokens.len() < 3 || n_single * 10 < tokens.len() * 6 {
        return tokens.join(" ");
    }
    let mut out = String::with_capacity(line.len());
    for (i, t) in tokens.iter().enumerate() {
        if i > 0 && !(single(t) && single(tokens[i - 1])) {
            out.push(' ');
        }
        out.push_str(t);
    }
    out
}

/// Settings card / `tidy ocr --status`.
#[derive(Debug, Clone, Serialize)]
pub struct OcrStatus {
    pub enabled: bool,
    /// "Windows 내장 OCR · 한국어"; `None` = no engine.
    pub engine: Option<String>,
    /// Why there is no engine, or a hint about it.
    pub note: Option<String>,
    pub counts: OcrCounts,
}

pub fn status(cfg: &Config, store: &Store) -> Result<OcrStatus, StoreError> {
    let counts = store.ocr_counts()?;
    let (engine, note) = match build_engine(&cfg.ocr) {
        Ok(e) => (Some(e.describe()), e.note()),
        Err(e) => (None, Some(e.to_string())),
    };
    Ok(OcrStatus {
        enabled: cfg.ocr.enabled,
        engine,
        note,
        counts,
    })
}

/// Result of one background pass.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OcrReport {
    /// Files whose text was recognized (or taken from the cache).
    pub recognized: usize,
    /// Files with no text in them.
    pub empty: usize,
    pub failed: usize,
    /// Still waiting after the pass.
    pub remaining: u64,
}

/// Recognize up to `limit` waiting files. `progress` gets each path.
pub fn run_pass(
    cfg: &Config,
    store: &Store,
    engine: &dyn OcrEngine,
    limit: usize,
    mut progress: impl FnMut(&Path),
) -> Result<OcrReport, StoreError> {
    let mut report = OcrReport::default();
    let detector = cfg
        .security
        .pii
        .enabled
        .then(|| crate::pii::Detector::lenient(&cfg.security.pii));
    let max_chars = cfg.index.max_text_chars;
    // Without PDF support PDFs keep waiting (an engine that reads them
    // may come later).
    for c in store.ocr_candidates(limit, engine.reads_pdf())? {
        progress(&c.path);
        let cached = match &c.hash {
            Some(h) => store.ocr_cache_get(h)?,
            None => None,
        };
        let text = match cached {
            Some(t) => Ok(t),
            None => {
                let _slot = c.network.then(|| crate::netio::network_io().acquire());
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    recognize(engine, &c.path, &cfg.ocr)
                }))
                .unwrap_or_else(|_| Err(OcrError::File("OCR engine panicked".into())));
                if let (Ok(t), Some(h)) = (&r, &c.hash) {
                    store.ocr_cache_put(h, t, engine.name())?;
                }
                r
            }
        };
        match text {
            Ok(t) => {
                let t = truncate_chars(t, max_chars);
                if t.trim().is_empty() {
                    report.empty += 1;
                    store.set_ocr_text(c.id, "", None)?;
                } else {
                    report.recognized += 1;
                    let pii = detector.as_ref().map(|d| d.judge(&c.name, Some(&t)));
                    store.set_ocr_text(c.id, &t, pii.as_ref())?;
                }
            }
            Err(e) => {
                tracing::debug!(path = %c.path.display(), "ocr failed: {e}");
                report.failed += 1;
                store.set_ocr_failed(c.id)?;
            }
        }
    }
    report.remaining = store.ocr_candidates(1, engine.reads_pdf())?.len() as u64;
    if report.remaining > 0 {
        report.remaining = store.ocr_counts()?.pending;
    }
    Ok(report)
}

fn truncate_chars(mut s: String, max: usize) -> String {
    if let Some((idx, _)) = s.char_indices().nth(max) {
        s.truncate(idx);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_spaced_hangul_only_when_most_tokens_are_single() {
        assert_eq!(normalize("견 적 서"), "견적서");
        assert_eq!(normalize("가 람 정 보 견적 2026"), "가람정보 견적 2026");
        assert_eq!(normalize("가람정보 견적서 송부"), "가람정보 견적서 송부");
        assert_eq!(normalize("a b c"), "a b c");
        assert_eq!(normalize("나 는 Tidy 를 쓴 다"), "나는 Tidy 를쓴다");
    }

    #[test]
    fn targets() {
        let max = 20 << 20;
        assert!(is_target("png", 10_000, TextStatus::Unsupported, None, max));
        assert!(!is_target("png", 1_000, TextStatus::Unsupported, None, max));
        assert!(!is_target(
            "png",
            30 << 20,
            TextStatus::Unsupported,
            None,
            max
        ));
        assert!(!is_target("png", 10_000, TextStatus::None, None, max));
        assert!(is_target("pdf", 10, TextStatus::Failed, None, max));
        assert!(is_target("pdf", 10, TextStatus::Ok, Some(" \n 3 "), max));
        assert!(!is_target(
            "pdf",
            10,
            TextStatus::Ok,
            Some("이 PDF에는 본문 글자가 충분히 들어 있어서 스캔본이 아닙니다. 정말로요."),
            max
        ));
        assert!(!is_target("docx", 10_000, TextStatus::Failed, None, max));
    }
}
