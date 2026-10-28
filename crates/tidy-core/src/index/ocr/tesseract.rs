//! tesseract command-line engine (+ poppler `pdftoppm` for scanned PDFs).
//! Pages go from `pdftoppm` to `tesseract` through pipes: no temp files.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{OcrEngine, OcrError};
use crate::config::OcrCfg;

/// Per process (one image, or one PDF page).
const TIMEOUT: Duration = Duration::from_secs(60);

pub struct TesseractEngine {
    exe: PathBuf,
    /// `kor+eng`; empty = tesseract's default.
    langs: String,
    pdftoppm: Option<PathBuf>,
}

/// BCP-47 short code → tesseract traineddata name.
fn tess_lang(code: &str) -> &str {
    match code {
        "ko" => "kor",
        "en" => "eng",
        "ja" => "jpn",
        "zh" | "zh-hans" => "chi_sim",
        "zh-hant" => "chi_tra",
        "de" => "deu",
        "fr" => "fra",
        other => other,
    }
}

impl TesseractEngine {
    pub fn find(cfg: &OcrCfg) -> Result<Self, OcrError> {
        let mut tries: Vec<PathBuf> = Vec::new();
        if let Some(p) = &cfg.tesseract_path {
            tries.push(p.clone());
        }
        tries.push(PathBuf::from("tesseract"));
        if cfg!(windows) {
            tries.push(PathBuf::from(
                r"C:\Program Files\Tesseract-OCR\tesseract.exe",
            ));
        }
        for exe in tries {
            let Ok(out) = run(command(&exe).arg("--list-langs"), None, TIMEOUT) else {
                continue;
            };
            let installed: Vec<String> = String::from_utf8_lossy(&out)
                .lines()
                .skip(1)
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            let langs: Vec<&str> = cfg
                .languages
                .iter()
                .map(|l| tess_lang(l))
                .filter(|l| installed.iter().any(|i| i == l))
                .collect();
            let pdftoppm = find_pdftoppm(&exe);
            return Ok(Self {
                exe,
                langs: langs.join("+"),
                pdftoppm,
            });
        }
        Err(OcrError::Unavailable(
            "tesseract가 설치되어 있지 않습니다 (Linux: apt install tesseract-ocr tesseract-ocr-kor poppler-utils, macOS: brew install tesseract tesseract-lang poppler)"
                .into(),
        ))
    }

    fn tess(&self, input: &str) -> Command {
        let mut c = command(&self.exe);
        c.arg(input).arg("stdout");
        if !self.langs.is_empty() {
            c.arg("-l").arg(&self.langs);
        }
        c
    }
}

fn find_pdftoppm(tesseract: &Path) -> Option<PathBuf> {
    let mut tries = vec![PathBuf::from("pdftoppm")];
    // Next to tesseract (some Windows bundles ship both).
    if let Some(dir) = tesseract.parent().filter(|d| !d.as_os_str().is_empty()) {
        tries.push(dir.join("pdftoppm"));
    }
    tries
        .into_iter()
        .find(|p| run(command(p).arg("-v"), None, TIMEOUT).is_ok())
}

impl OcrEngine for TesseractEngine {
    fn name(&self) -> &'static str {
        "tesseract"
    }

    fn describe(&self) -> String {
        if self.langs.is_empty() {
            "tesseract".into()
        } else {
            format!("tesseract · {}", self.langs)
        }
    }

    fn note(&self) -> Option<String> {
        let mut notes = Vec::new();
        if !self.langs.split('+').any(|l| l == "kor") {
            notes.push("한국어 학습 데이터(kor) 없음 — tesseract-ocr-kor 설치");
        }
        if self.pdftoppm.is_none() {
            notes.push("pdftoppm 없음 — 스캔 PDF는 poppler 설치 후 읽음");
        }
        (!notes.is_empty()).then(|| notes.join(" · "))
    }

    fn reads_pdf(&self) -> bool {
        self.pdftoppm.is_some()
    }

    fn image(&self, path: &Path) -> Result<String, OcrError> {
        let input = path
            .to_str()
            .ok_or_else(|| OcrError::File(format!("{}: 경로를 넘길 수 없음", path.display())))?;
        let out = run(&mut self.tess(input), None, TIMEOUT).map_err(OcrError::File)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn pdf(&self, path: &Path, max_pages: u32) -> Result<String, OcrError> {
        let Some(pdftoppm) = &self.pdftoppm else {
            return Err(OcrError::Unavailable("pdftoppm 없음".into()));
        };
        let mut pages = Vec::new();
        for page in 1..=max_pages {
            let n = page.to_string();
            let png = run(
                command(pdftoppm)
                    .args(["-r", "200", "-png", "-f", &n, "-l", &n, "-singlefile"])
                    .arg(path),
                None,
                TIMEOUT,
            );
            let png = match png {
                Ok(p) if !p.is_empty() => p,
                // Past the last page (or unreadable from page 1 on).
                Ok(_) | Err(_) if page > 1 => break,
                Ok(_) => return Err(OcrError::File("PDF를 그림으로 바꾸지 못함".into())),
                Err(e) => return Err(OcrError::File(e)),
            };
            let text = run(&mut self.tess("stdin"), Some(png), TIMEOUT).map_err(OcrError::File)?;
            pages.push(String::from_utf8_lossy(&text).into_owned());
        }
        Ok(pages.join("\n\n"))
    }
}

fn command(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Run to completion (stdout), killing the process after `timeout`.
fn run(cmd: &mut Command, input: Option<Vec<u8>>, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut child = cmd
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let writer = input.and_then(|data| {
        child.stdin.take().map(|mut stdin| {
            std::thread::spawn(move || {
                let _ = stdin.write_all(&data);
            })
        })
    });
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let mut stderr = child.stderr.take().expect("piped");
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{}초 안에 끝나지 않음", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    let out = reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    if status.success() {
        Ok(out)
    } else {
        let msg = String::from_utf8_lossy(&err);
        Err(msg
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("실패")
            .trim()
            .to_string())
    }
}
