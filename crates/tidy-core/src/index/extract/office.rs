//! Office formats: DOCX / PPTX (OOXML zip + XML) and XLSX & friends via
//! `calamine`. Only visible text is taken; no styling, no images.

use std::io::Read;
use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader as _};
use quick_xml::events::{BytesRef, Event};
use quick_xml::Reader;

use super::{ExtractError, Extractor};

// ── DOCX ────────────────────────────────────────────────────────────────

pub struct DocxExtractor;

impl Extractor for DocxExtractor {
    fn name(&self) -> &'static str {
        "docx"
    }

    fn supports(&self, ext: &str) -> bool {
        matches!(ext, "docx" | "docm" | "dotx")
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let mut zip = open_zip(path)?;
        let mut out = String::new();
        for part in [
            "word/document.xml",
            "word/footnotes.xml",
            "word/endnotes.xml",
        ] {
            if let Some(xml) = read_part(&mut zip, part, path)? {
                out.push_str(&ooxml_text(&xml, &["p", "tab", "br", "cr"], &["t"], path)?);
                out.push('\n');
            }
        }
        Ok(out)
    }
}

// ── PPTX ────────────────────────────────────────────────────────────────

pub struct PptxExtractor;

impl Extractor for PptxExtractor {
    fn name(&self) -> &'static str {
        "pptx"
    }

    fn supports(&self, ext: &str) -> bool {
        matches!(ext, "pptx" | "pptm" | "potx")
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let mut zip = open_zip(path)?;
        // Slides in numeric order (zip order is arbitrary), then their notes.
        let mut slides: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| {
                let num = n
                    .strip_prefix("ppt/slides/slide")?
                    .strip_suffix(".xml")?
                    .parse()
                    .ok()?;
                Some((num, n.to_string()))
            })
            .collect();
        slides.sort();
        let mut out = String::new();
        for (num, name) in &slides {
            if let Some(xml) = read_part(&mut zip, name, path)? {
                out.push_str(&format!("[slide {num}]\n"));
                out.push_str(&ooxml_text(&xml, &["p", "br"], &["t"], path)?);
                out.push('\n');
            }
            let notes = format!("ppt/notesSlides/notesSlide{num}.xml");
            if let Some(xml) = read_part(&mut zip, &notes, path)? {
                let t = ooxml_text(&xml, &["p", "br"], &["t"], path)?;
                if !t.trim().is_empty() {
                    out.push_str("[notes]\n");
                    out.push_str(&t);
                    out.push('\n');
                }
            }
        }
        Ok(out)
    }
}

// ── XLSX / XLS / ODS ────────────────────────────────────────────────────

pub struct XlsxExtractor;

impl Extractor for XlsxExtractor {
    fn name(&self) -> &'static str {
        "xlsx"
    }

    fn supports(&self, ext: &str) -> bool {
        matches!(ext, "xlsx" | "xlsm" | "xlsb" | "xls" | "ods")
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let mut wb = open_workbook_auto(path).map_err(|e| ExtractError::format(path, e))?;
        let mut out = String::new();
        for name in wb.sheet_names() {
            let Ok(range) = wb.worksheet_range(&name) else {
                continue;
            };
            out.push_str(&format!("[sheet {name}]\n"));
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .filter(|c| !matches!(c, Data::Empty))
                    .map(cell_text)
                    .collect();
                if !cells.is_empty() {
                    out.push_str(&cells.join("\t"));
                    out.push('\n');
                }
            }
        }
        Ok(out)
    }
}

fn cell_text(c: &Data) -> String {
    match c {
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => d
            .as_datetime()
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| d.as_f64().to_string()),
        Data::Error(e) => format!("{e:?}"),
        Data::Empty => String::new(),
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

type Zip = zip::ZipArchive<std::fs::File>;

fn open_zip(path: &Path) -> Result<Zip, ExtractError> {
    let f = std::fs::File::open(path).map_err(|source| ExtractError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    zip::ZipArchive::new(f).map_err(|e| ExtractError::format(path, e))
}

fn read_part(zip: &mut Zip, name: &str, path: &Path) -> Result<Option<String>, ExtractError> {
    let mut entry = match zip.by_name(name) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(ExtractError::format(path, e)),
    };
    let mut s = String::new();
    entry
        .read_to_string(&mut s)
        .map_err(|e| ExtractError::format(path, format!("{name}: {e}")))?;
    Ok(Some(s))
}

/// Collect text of `text_tags` elements (by local name); `break_tags`
/// produce a newline (or a tab for `tab`).
fn ooxml_text(
    xml: &str,
    break_tags: &[&str],
    text_tags: &[&str],
    path: &Path,
) -> Result<String, ExtractError> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = 0u32;
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| ExtractError::format(path, e))?;
        match ev {
            Event::Eof => break,
            Event::Start(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if text_tags.contains(&local.as_str()) {
                    in_text += 1;
                }
            }
            Event::End(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if text_tags.contains(&local.as_str()) {
                    in_text = in_text.saturating_sub(1);
                } else if break_tags.contains(&local.as_str()) {
                    out.push('\n');
                }
            }
            Event::Empty(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                match local.as_str() {
                    "tab" => out.push('\t'),
                    l if break_tags.contains(&l) => out.push('\n'),
                    _ => {}
                }
            }
            Event::Text(t) if in_text > 0 => {
                out.push_str(&t.decode().map_err(|e| ExtractError::format(path, e))?);
            }
            Event::CData(t) if in_text > 0 => {
                out.push_str(&String::from_utf8_lossy(&t));
            }
            Event::GeneralRef(r) if in_text > 0 => {
                if let Some(c) = resolve_ref(&r) {
                    out.push(c);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn resolve_ref(r: &BytesRef<'_>) -> Option<char> {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return Some(c);
    }
    match std::str::from_utf8(r).ok()? {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ooxml_paragraphs_and_entities() {
        let xml = r#"<w:document xmlns:w="x"><w:body>
<w:p><w:r><w:t>안녕</w:t></w:r><w:r><w:t xml:space="preserve"> 세계 &amp; 친구</w:t></w:r></w:p>
<w:p><w:r><w:tab/><w:t>둘째 &#x4e;</w:t></w:r></w:p>
<w:p><w:pPr><w:rPr><w:t>not visible? still t</w:t></w:rPr></w:pPr></w:p>
</w:body></w:document>"#;
        let t = ooxml_text(xml, &["p", "tab", "br"], &["t"], Path::new("x")).unwrap();
        assert_eq!(
            super::super::clean_text(&t),
            "안녕 세계 & 친구\n둘째 N\nnot visible? still t"
        );
    }
}
