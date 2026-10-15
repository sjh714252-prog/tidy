//! HTML → text: drops `<script>`/`<style>`/comments, turns tags into
//! whitespace (block tags into newlines) and decodes common entities.
//! Good enough for indexing; not a browser.

use std::path::Path;

use super::text::decode;
use super::{read_bytes, ExtractError, Extractor};

pub struct HtmlExtractor;

impl Extractor for HtmlExtractor {
    fn name(&self) -> &'static str {
        "html"
    }

    fn supports(&self, ext: &str) -> bool {
        matches!(ext, "html" | "htm" | "xhtml" | "mhtml")
    }

    fn extract(&self, path: &Path) -> Result<String, ExtractError> {
        let bytes = read_bytes(path)?;
        let html = decode(&bytes).ok_or_else(|| ExtractError::Binary(path.to_path_buf()))?;
        Ok(html_to_text(&html))
    }
}

const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "tr",
    "td",
    "th",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "pre",
    "blockquote",
    "hr",
    "title",
    "dd",
    "dt",
    "dl",
    "figure",
    "figcaption",
    "aside",
    "main",
    "form",
    "option",
];

pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        push_text(&mut out, &rest[..lt]);
        let after = &rest[lt + 1..];
        // Comments.
        if let Some(r) = after.strip_prefix("!--") {
            rest = r.find("-->").map(|i| &r[i + 3..]).unwrap_or("");
            continue;
        }
        let Some(gt) = after.find('>') else {
            break;
        };
        let tag = &after[..gt];
        rest = &after[gt + 1..];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if matches!(name.as_str(), "script" | "style" | "noscript" | "template")
            && !tag.starts_with('/')
        {
            // Skip to the matching close tag.
            let close = format!("</{name}");
            let lower = rest.to_ascii_lowercase();
            match lower.find(&close) {
                Some(i) => {
                    let r = &rest[i..];
                    rest = r.find('>').map(|j| &r[j + 1..]).unwrap_or("");
                }
                None => rest = "",
            }
            continue;
        }
        if BLOCK_TAGS.contains(&name.as_str()) {
            let end = out.trim_end_matches([' ', '\t']).len();
            out.truncate(end);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        } else if !out.ends_with(char::is_whitespace) {
            out.push(' ');
        }
    }
    push_text(&mut out, rest);
    out
}

/// Text between tags: HTML whitespace (incl. newlines) collapses, so the
/// only line breaks in the output come from block tags.
fn push_text(out: &mut String, s: &str) {
    let s = s.replace(['\n', '\r'], " ");
    let mut rest = s.as_str();
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        match after.find(';').filter(|i| *i <= 10) {
            Some(semi) => {
                let ent = &after[..semi];
                match decode_entity(ent) {
                    Some(c) => out.push_str(&c),
                    None => {
                        out.push('&');
                        out.push_str(ent);
                        out.push(';');
                    }
                }
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
}

fn decode_entity(ent: &str) -> Option<String> {
    let c = match ent {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#39" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "middot" => '·',
        "hellip" => '…',
        _ => {
            let code = match (
                ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")),
                ent.strip_prefix('#'),
            ) {
                (Some(hex), _) => u32::from_str_radix(hex, 16).ok()?,
                (None, Some(dec)) => dec.parse().ok()?,
                (None, None) => return None,
            };
            char::from_u32(code)?
        }
    };
    Some(c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_scripts_and_entities() {
        let html = r#"<html><head><title>회의록</title><style>p{color:red}</style>
<script>var x = "<b>no</b>";</script></head>
<body><h1>10월 &quot;정기&quot; 회의</h1><p>참석자: 홍&nbsp;길동 &amp; 김영희</p>
<!-- 주석 --><ul><li>안건 &#49;</li><li>결정 &#x4e;</li></ul></body></html>"#;
        let t = super::super::clean_text(&html_to_text(html));
        assert_eq!(
            t,
            "회의록\n10월 \"정기\" 회의\n참석자: 홍 길동 & 김영희\n안건 1\n결정 N"
        );
    }
}
