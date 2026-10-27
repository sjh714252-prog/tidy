//! (M14) Hangul word processor documents (SPEC §14.9 3).
//!
//! - `.hwpx` (OWPML): a zip whose `Contents/section<N>.xml` hold the body;
//!   text sits in `<hp:t>` inside `<hp:p>` paragraphs (table cells too).
//! - `.hwp` (HWP 5.0): an OLE compound file. `FileHeader` says whether the
//!   body is compressed / password-protected / a distribution document;
//!   `BodyText/Section<N>` is a raw-deflate stream of records, and the text
//!   is in `HWPTAG_PARA_TEXT` records as UTF-16LE with inline control codes.
//!   `PrvText` (preview text) is the fallback when the body cannot be read.

use std::io::Read;
use std::path::Path;

use super::office::{ooxml_text, open_zip, read_part};
use super::{ExtractError, Extractor};

// ── HWPX ────────────────────────────────────────────────────────────────

pub struct HwpxExtractor;

impl Extractor for HwpxExtractor {
    fn name(&self) -> &'static str {
        "hwpx"
    }

    fn supports(&self, ext: &str) -> bool {
        ext == "hwpx"
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let mut zip = open_zip(path)?;
        let mut sections: Vec<(u32, String)> = zip
            .file_names()
            .filter_map(|n| {
                let num = n
                    .strip_prefix("Contents/section")?
                    .strip_suffix(".xml")?
                    .parse()
                    .ok()?;
                Some((num, n.to_string()))
            })
            .collect();
        sections.sort();
        let mut out = String::new();
        for (_, name) in &sections {
            if let Some(xml) = read_part(&mut zip, name, path)? {
                out.push_str(&ooxml_text(&xml, &["p", "lineBreak"], &["t"], path)?);
                out.push('\n');
            }
        }
        if out.trim().is_empty() {
            if let Some(prv) = read_part(&mut zip, "Preview/PrvText.txt", path)? {
                return Ok(prv);
            }
            if sections.is_empty() {
                return Err(ExtractError::format(path, "no Contents/section*.xml"));
            }
        }
        Ok(out)
    }
}

// ── HWP 5.0 ─────────────────────────────────────────────────────────────

pub struct HwpExtractor;

const SIGNATURE: &[u8] = b"HWP Document File";
const FLAG_COMPRESSED: u32 = 1;
const FLAG_PASSWORD: u32 = 1 << 1;
const FLAG_DISTRIBUTION: u32 = 1 << 2;
const HWPTAG_BEGIN: u16 = 0x10;
const HWPTAG_PARA_TEXT: u16 = HWPTAG_BEGIN + 51;

impl Extractor for HwpExtractor {
    fn name(&self) -> &'static str {
        "hwp"
    }

    fn supports(&self, ext: &str) -> bool {
        ext == "hwp"
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let mut doc = cfb::open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
                ExtractError::Io {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
            _ => ExtractError::format(path, format!("not an HWP 5.0 file: {e}")),
        })?;
        let header = read_stream(&mut doc, "/FileHeader")
            .ok_or_else(|| ExtractError::format(path, "no FileHeader stream"))?;
        if header.len() < 40 || !header.starts_with(SIGNATURE) {
            return Err(ExtractError::format(path, "bad FileHeader signature"));
        }
        let flags = u32::from_le_bytes([header[36], header[37], header[38], header[39]]);
        if flags & (FLAG_PASSWORD | FLAG_DISTRIBUTION) != 0 {
            return Err(ExtractError::Encrypted(path.to_path_buf()));
        }
        let body = body_text(&mut doc, flags & FLAG_COMPRESSED != 0);
        match body {
            Ok(t) if !t.trim().is_empty() => Ok(t),
            other => match read_stream(&mut doc, "/PrvText") {
                Some(prv) if prv.len() >= 2 => Ok(utf16le(&prv)),
                _ => other.map_err(|msg| ExtractError::format(path, msg)),
            },
        }
    }
}

fn read_stream<F: Read + std::io::Seek>(
    doc: &mut cfb::CompoundFile<F>,
    name: &str,
) -> Option<Vec<u8>> {
    let mut s = doc.open_stream(name).ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Text of every `BodyText/Section<N>` in order.
fn body_text<F: Read + std::io::Seek>(
    doc: &mut cfb::CompoundFile<F>,
    compressed: bool,
) -> Result<String, String> {
    let mut sections: Vec<(u32, String)> = doc
        .read_storage("/BodyText")
        .map_err(|e| format!("no BodyText: {e}"))?
        .filter_map(|e| {
            let n = e.name().strip_prefix("Section")?.parse().ok()?;
            Some((n, e.path().to_string_lossy().into_owned()))
        })
        .collect();
    sections.sort();
    if sections.is_empty() {
        return Err("no BodyText/Section streams".into());
    }
    let mut out = String::new();
    for (_, name) in sections {
        let raw = read_stream(doc, &name).ok_or_else(|| format!("cannot read {name}"))?;
        let data = if compressed {
            let mut d = Vec::new();
            flate2::read::DeflateDecoder::new(&raw[..])
                .read_to_end(&mut d)
                .map_err(|e| format!("{name}: deflate: {e}"))?;
            d
        } else {
            raw
        };
        records_text(&data, &mut out).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(out)
}

/// Walk the record stream and append the text of each PARA_TEXT record.
fn records_text(data: &[u8], out: &mut String) -> Result<(), &'static str> {
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let h = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        i += 4;
        let tag = (h & 0x3FF) as u16;
        let mut size = (h >> 20) as usize;
        if size == 0xFFF {
            if i + 4 > data.len() {
                return Err("truncated record size");
            }
            size = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            i += 4;
        }
        if i + size > data.len() {
            return Err("truncated record");
        }
        if tag == HWPTAG_PARA_TEXT {
            para_text(&data[i..i + size], out);
            out.push('\n');
        }
        i += size;
    }
    Ok(())
}

/// UTF-16LE paragraph text without control codes. Extended and inline
/// controls take 8 WCHARs (the code, 6 of payload, the code again); plain
/// "char" controls take one.
fn para_text(bytes: &[u8], out: &mut String) {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let mut text: Vec<u16> = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        match c {
            // inline: field end, reserved, title mark, tab, reserved
            4..=9 | 19 | 20 => {
                if c == 9 {
                    text.push(u16::from(b' '));
                }
                i += 8;
            }
            // extended: section/column def, field start, table/drawing,
            // comment, header/footer, footnote, auto number, page control,
            // bookmark, overlapping characters
            1..=3 | 11 | 12 | 14..=18 | 21..=23 => i += 8,
            // line break; 13 = end of paragraph (the record ends there)
            10 => {
                text.push(u16::from(b'\n'));
                i += 1;
            }
            // 24 hyphen, 30 bundle blank, 31 fixed-width blank
            24 => {
                text.push(u16::from(b'-'));
                i += 1;
            }
            30 | 31 => {
                text.push(u16::from(b' '));
                i += 1;
            }
            0..=31 => i += 1,
            _ => {
                text.push(c);
                i += 1;
            }
        }
    }
    out.push_str(&String::from_utf16_lossy(&text));
}

fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Build a minimal HWP 5.0 / HWPX file (fixtures and tests). Not a full
/// writer: just the streams the extractors read.
#[doc(hidden)]
pub mod testgen {
    use std::io::{Cursor, Write};

    use super::*;

    /// One paragraph = one PARA_TEXT record; `\t` becomes an inline tab
    /// control. A paragraph holding `\u{b}` gets a table control (extended
    /// 11) there, followed by a `tbl ` CTRL_HEADER; put the cell texts in
    /// the next paragraphs, as HWP stores them.
    pub fn hwp_bytes(paragraphs: &[&str], flags: u32, prv_text: Option<&str>) -> Vec<u8> {
        let mut rec = Vec::new();
        for (n, p) in paragraphs.iter().enumerate() {
            // A PARA_HEADER-ish record first (tag 66), ignored by the reader.
            push_record(&mut rec, HWPTAG_BEGIN + 50, &[0u8; 22]);
            let mut units: Vec<u16> = Vec::new();
            if n == 0 {
                // Section/column definition control (extended, 8 WCHARs).
                units.extend([2u16, 0x6463, 0x6573, 0, 0, 0, 0, 2]);
            }
            for ch in p.chars() {
                match ch {
                    '\t' => units.extend([9u16, 0, 0, 0, 0, 0, 0, 9]),
                    '\u{b}' => units.extend([11u16, 0x626c, 0x7420, 0, 0, 0, 0, 11]),
                    c => {
                        let mut b = [0u16; 2];
                        units.extend_from_slice(c.encode_utf16(&mut b));
                    }
                }
            }
            units.push(13);
            let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
            push_record(&mut rec, HWPTAG_PARA_TEXT, &bytes);
            if p.contains('\u{b}') {
                // CTRL_HEADER (tag 71) with the "tbl " control id.
                push_record(&mut rec, HWPTAG_BEGIN + 55, b" lbt\0\0\0\0");
            }
        }
        let body = if flags & FLAG_COMPRESSED != 0 {
            let mut e =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&rec).unwrap();
            e.finish().unwrap()
        } else {
            rec
        };
        let mut header = vec![0u8; 256];
        header[..SIGNATURE.len()].copy_from_slice(SIGNATURE);
        header[32..36].copy_from_slice(&0x0500_0000u32.to_le_bytes());
        header[36..40].copy_from_slice(&flags.to_le_bytes());

        let mut doc = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        doc.create_stream("/FileHeader")
            .unwrap()
            .write_all(&header)
            .unwrap();
        doc.create_storage("/BodyText").unwrap();
        doc.create_stream("/BodyText/Section0")
            .unwrap()
            .write_all(&body)
            .unwrap();
        if let Some(p) = prv_text {
            let b: Vec<u8> = p.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            doc.create_stream("/PrvText")
                .unwrap()
                .write_all(&b)
                .unwrap();
        }
        doc.flush().unwrap();
        doc.into_inner().into_inner()
    }

    fn push_record(out: &mut Vec<u8>, tag: u16, data: &[u8]) {
        let size = data.len();
        if size >= 0xFFF {
            out.extend((u32::from(tag) | (0xFFF << 20)).to_le_bytes());
            out.extend((size as u32).to_le_bytes());
        } else {
            out.extend((u32::from(tag) | ((size as u32) << 20)).to_le_bytes());
        }
        out.extend_from_slice(data);
    }

    /// `paragraphs` plus a table whose cells hold `cells`.
    pub fn hwpx_bytes(paragraphs: &[&str], cells: &[&str]) -> Vec<u8> {
        use zip::write::SimpleFileOptions;
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default();
        z.start_file("mimetype", stored).unwrap();
        z.write_all(b"application/hwp+zip").unwrap();
        z.start_file("version.xml", deflated).unwrap();
        z.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><hv:HCFVersion xmlns:hv="http://www.hancom.co.kr/hwpml/2011/version" major="5" minor="1"/>"#).unwrap();
        let mut sec = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">"#,
        );
        for p in paragraphs {
            sec.push_str(r#"<hp:p id="0"><hp:run charPrIDRef="0"><hp:t>"#);
            sec.push_str(&xml_escape(p));
            sec.push_str("</hp:t></hp:run></hp:p>");
        }
        if !cells.is_empty() {
            sec.push_str(r#"<hp:p id="0"><hp:run charPrIDRef="0"><hp:tbl rowCnt="1"><hp:tr>"#);
            for c in cells {
                sec.push_str(r#"<hp:tc><hp:subList><hp:p id="0"><hp:run><hp:t>"#);
                sec.push_str(&xml_escape(c));
                sec.push_str("</hp:t></hp:run></hp:p></hp:subList></hp:tc>");
            }
            sec.push_str("</hp:tr></hp:tbl></hp:run></hp:p>");
        }
        sec.push_str("</hs:sec>");
        z.start_file("Contents/section0.xml", deflated).unwrap();
        z.write_all(sec.as_bytes()).unwrap();
        z.start_file("Preview/PrvText.txt", deflated).unwrap();
        z.write_all(paragraphs.first().copied().unwrap_or("").as_bytes())
            .unwrap();
        z.finish().unwrap().into_inner()
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub const COMPRESSED: u32 = FLAG_COMPRESSED;
    pub const PASSWORD: u32 = FLAG_PASSWORD;
    pub const DISTRIBUTION: u32 = FLAG_DISTRIBUTION;
}

#[cfg(test)]
mod tests {
    use super::testgen::*;
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn hwp_compressed_and_plain_bodies() {
        let tmp = tempfile::tempdir().unwrap();
        for flags in [COMPRESSED, 0] {
            let p = write(
                tmp.path(),
                &format!("문서{flags}.hwp"),
                &hwp_bytes(&["첫 문단 Hello 123", "이름\t값"], flags, None),
            );
            let t = super::super::clean_text(&HwpExtractor.extract(&p).unwrap());
            assert_eq!(t, "첫 문단 Hello 123\n이름 값", "flags {flags}");
        }
    }

    #[test]
    fn long_record_uses_extended_size() {
        let tmp = tempfile::tempdir().unwrap();
        let long = "가".repeat(3000); // 6000 bytes > 0xFFF
        let p = write(
            tmp.path(),
            "긴.hwp",
            &hwp_bytes(&[&long, "끝"], COMPRESSED, None),
        );
        let t = HwpExtractor.extract(&p).unwrap();
        assert!(t.contains(&long) && t.contains("끝"));
    }

    #[test]
    fn password_and_distribution_documents_are_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        for f in [PASSWORD, DISTRIBUTION] {
            let p = write(
                tmp.path(),
                "암호.hwp",
                &hwp_bytes(&["비밀"], COMPRESSED | f, Some("미리보기")),
            );
            let e = HwpExtractor.extract(&p).unwrap_err();
            assert!(matches!(e, ExtractError::Encrypted(_)), "{e}");
            assert!(e.to_string().contains("본문 추출 불가(암호)"));
        }
    }

    #[test]
    fn broken_body_falls_back_to_prvtext() {
        let tmp = tempfile::tempdir().unwrap();
        // Claims compression but the body is not deflate data.
        let mut bytes = hwp_bytes(&["본문"], 0, Some("미리보기 텍스트 PrvText"));
        let p = write(tmp.path(), "a.hwp", &bytes);
        // Re-flag as compressed by patching the header inside the CFB.
        let mut doc = cfb::open(&p).unwrap();
        let mut h = read_stream(&mut doc, "/FileHeader").unwrap();
        h[36] |= 1;
        drop(doc);
        let mut doc = cfb::CompoundFile::open(std::io::Cursor::new(&mut bytes)).unwrap();
        {
            use std::io::Write;
            let mut s = doc.create_stream("/FileHeader").unwrap();
            s.write_all(&h).unwrap();
        }
        doc.flush().unwrap();
        drop(doc);
        std::fs::write(&p, &bytes).unwrap();
        let t = HwpExtractor.extract(&p).unwrap();
        assert_eq!(t, "미리보기 텍스트 PrvText");
    }

    #[test]
    fn not_a_hwp_file_is_a_format_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "가짜.hwp", b"plain text, not OLE");
        assert!(matches!(
            HwpExtractor.extract(&p),
            Err(ExtractError::Format { .. })
        ));
    }

    #[test]
    fn hwpx_paragraphs_and_table_cells() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(
            tmp.path(),
            "a.hwpx",
            &hwpx_bytes(&["제목 A&B", "둘째 2026"], &["셀1", "셀 둘"]),
        );
        let t = super::super::clean_text(&HwpxExtractor.extract(&p).unwrap());
        assert_eq!(t, "제목 A&B\n둘째 2026\n셀1\n셀 둘");
    }
}
