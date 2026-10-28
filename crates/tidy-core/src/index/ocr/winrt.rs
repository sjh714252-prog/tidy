//! Windows' built-in OCR (`Windows.Media.Ocr`): no install, offline.
//! Images are decoded with `BitmapDecoder`; scanned PDFs are rendered
//! page by page with `Windows.Data.Pdf` at about 192 dpi.

use std::cell::Cell;
use std::path::Path;

use windows::core::HSTRING;
use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
    ColorManagementMode, ExifOrientationMode, SoftwareBitmap,
};
use windows::Media::Ocr::OcrEngine as WinOcr;
use windows::Storage::Streams::{IRandomAccessStream, InMemoryRandomAccessStream};
use windows::Storage::{FileAccessMode, StorageFile};

use super::{OcrEngine, OcrError};
use crate::config::OcrCfg;

thread_local! {
    static RO_INIT: Cell<bool> = const { Cell::new(false) };
}

/// WinRT needs the calling thread in an apartment; background threads
/// join the multithreaded one (an STA thread keeps its own).
fn ensure_apartment() {
    RO_INIT.with(|done| {
        if !done.get() {
            unsafe {
                let _ = windows::Win32::System::WinRT::RoInitialize(
                    windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
                );
            }
            done.set(true);
        }
    });
}

pub struct WindowsEngine {
    /// Language tag the engine is created with; `None` = user profile.
    lang: Option<String>,
    display: String,
    /// Korean was asked for but has no OCR language pack.
    missing_korean: bool,
}

fn wanted_supported(code: &str) -> bool {
    Language::CreateLanguage(&HSTRING::from(code))
        .and_then(|l| WinOcr::IsLanguageSupported(&l))
        .unwrap_or(false)
}

impl WindowsEngine {
    pub fn new(cfg: &OcrCfg) -> Result<Self, OcrError> {
        ensure_apartment();
        let lang = cfg.languages.iter().find(|l| wanted_supported(l)).cloned();
        let engine = Self::create(lang.as_deref()).map_err(|e| {
            OcrError::Unavailable(format!(
                "Windows OCR 언어팩이 없습니다 — 설정 › 시간 및 언어 › 언어에서 한국어 언어 옵션의 '광학 문자 인식'을 설치하세요 ({e})"
            ))
        })?;
        let display = engine
            .RecognizerLanguage()
            .and_then(|l| l.NativeName())
            .map(|n| n.to_string())
            .unwrap_or_default();
        Ok(Self {
            missing_korean: cfg.languages.iter().any(|l| l == "ko") && !wanted_supported("ko"),
            lang,
            display,
        })
    }

    fn create(lang: Option<&str>) -> windows::core::Result<WinOcr> {
        match lang {
            Some(code) => {
                WinOcr::TryCreateFromLanguage(&Language::CreateLanguage(&HSTRING::from(code))?)
            }
            None => WinOcr::TryCreateFromUserProfileLanguages(),
        }
    }

    fn recognize_stream(&self, stream: &IRandomAccessStream) -> windows::core::Result<String> {
        let decoder = BitmapDecoder::CreateAsync(stream)?.join()?;
        let bitmap = to_bitmap(&decoder)?;
        let engine = Self::create(self.lang.as_deref())?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let mut lines = Vec::new();
        for line in result.Lines()? {
            lines.push(line.Text()?.to_string());
        }
        Ok(lines.join("\n"))
    }
}

/// Bgra8 bitmap, scaled down to fit `OcrEngine::MaxImageDimension`.
fn to_bitmap(decoder: &BitmapDecoder) -> windows::core::Result<SoftwareBitmap> {
    let (w, h) = (
        decoder.OrientedPixelWidth()?,
        decoder.OrientedPixelHeight()?,
    );
    let max = WinOcr::MaxImageDimension()?.max(1);
    let transform = BitmapTransform::new()?;
    if w > max || h > max {
        let scale = max as f64 / w.max(h) as f64;
        transform.SetScaledWidth(((w as f64 * scale) as u32).max(1))?;
        transform.SetScaledHeight(((h as f64 * scale) as u32).max(1))?;
        transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    }
    decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()
}

/// `StorageFile` wants an absolute path with backslashes only: a `/` in
/// it (config paths, `join("a/b")`) fails as "path too long".
fn winrt_path(path: &Path) -> std::path::PathBuf {
    let abs = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    std::path::PathBuf::from(abs.to_string_lossy().replace('/', "\\"))
}

fn open(path: &Path) -> windows::core::Result<StorageFile> {
    StorageFile::GetFileFromPathAsync(&HSTRING::from(winrt_path(path).as_os_str()))?.join()
}

fn file_err(path: &Path, e: windows::core::Error) -> OcrError {
    OcrError::File(format!("{}: {}", path.display(), e.message()))
}

impl OcrEngine for WindowsEngine {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn describe(&self) -> String {
        if self.display.is_empty() {
            "Windows 내장 OCR".into()
        } else {
            format!("Windows 내장 OCR · {}", self.display)
        }
    }

    fn note(&self) -> Option<String> {
        self.missing_korean.then(|| {
            "한국어 인식 언어팩 없음 — 설정 › 시간 및 언어 › 언어 › 한국어 › 언어 옵션 › 광학 문자 인식 설치".into()
        })
    }

    fn reads_pdf(&self) -> bool {
        true
    }

    fn image(&self, path: &Path) -> Result<String, OcrError> {
        ensure_apartment();
        let run = || -> windows::core::Result<String> {
            let file = open(path)?;
            let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
            self.recognize_stream(&stream)
        };
        run().map_err(|e| file_err(path, e))
    }

    fn pdf(&self, path: &Path, max_pages: u32) -> Result<String, OcrError> {
        ensure_apartment();
        let run = || -> windows::core::Result<String> {
            let file = open(path)?;
            let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
            let max_dim = WinOcr::MaxImageDimension()?.max(1) as f32;
            let mut pages = Vec::new();
            for i in 0..doc.PageCount()?.min(max_pages) {
                let page = doc.GetPage(i)?;
                let size = page.Size()?;
                // DIPs are 1/96 inch: 2× ≈ 192 dpi, capped for the engine.
                let scale = 2.0f32
                    .min(max_dim / size.Width.max(1.0))
                    .min(max_dim / size.Height.max(1.0));
                let opts = PdfPageRenderOptions::new()?;
                opts.SetDestinationWidth((size.Width * scale).max(1.0) as u32)?;
                opts.SetDestinationHeight((size.Height * scale).max(1.0) as u32)?;
                let stream = InMemoryRandomAccessStream::new()?;
                page.RenderWithOptionsToStreamAsync(&stream, &opts)?
                    .join()?;
                stream.Seek(0)?;
                let stream: IRandomAccessStream = stream.into();
                pages.push(self.recognize_stream(&stream)?);
            }
            Ok(pages.join("\n\n"))
        };
        run().map_err(|e| file_err(path, e))
    }
}
