//! Search query syntax (SPEC §5.6): free words plus filters.
//!
//! ```text
//! 과금 내역 ext:xlsx,pdf in:~/Documents after:2026-09 before:2026-10-15 size:>10mb type:doc
//! "정확한 구절" in:"C:/Users/홍 길동/바탕 화면"
//! ```
//! Words are ANDed. A quoted string is one phrase. Filters can appear
//! anywhere. Unknown `key:value` pairs are treated as plain words so a
//! file named `a:b.txt` can still be found.

use std::path::PathBuf;

use chrono::{Local, NaiveDate, TimeZone};

use crate::config::expand_home;
use crate::paths::normalize_lexical;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Free-text terms (phrases keep their inner spaces).
    pub terms: Vec<String>,
    /// Lower-case extensions without dots; empty = any.
    pub exts: Vec<String>,
    /// Only files under this folder.
    pub in_dir: Option<PathBuf>,
    /// Unix seconds, inclusive lower bound on mtime.
    pub after: Option<i64>,
    /// Unix seconds, exclusive upper bound on mtime.
    pub before: Option<i64>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
}

/// `type:` groups. Kept small on purpose; `ext:` covers the rest.
pub fn type_exts(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "image" | "img" | "사진" | "이미지" => &[
            "png", "jpg", "jpeg", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff",
        ],
        "doc" | "document" | "문서" => &[
            "pdf", "doc", "docx", "hwp", "hwpx", "ppt", "pptx", "xls", "xlsx", "txt", "md", "rtf",
            "odt",
        ],
        "video" | "영상" => &["mp4", "mkv", "mov", "avi", "wmv", "webm"],
        "audio" | "음악" => &["mp3", "wav", "flac", "m4a", "aac", "ogg"],
        "archive" | "압축" => &["zip", "7z", "rar", "tar", "gz", "alz", "egg"],
        "code" | "코드" => &[
            "rs", "py", "js", "ts", "tsx", "jsx", "java", "kt", "c", "cpp", "h", "cs", "go", "rb",
            "php", "swift", "sh", "sql", "toml", "yaml", "yml", "json",
        ],
        _ => return None,
    })
}

impl Query {
    pub fn parse(input: &str) -> Self {
        let mut q = Query::default();
        for tok in tokenize(input) {
            match tok {
                Token::Phrase(p) => {
                    if !p.trim().is_empty() {
                        q.terms.push(p.trim().to_string());
                    }
                }
                Token::Word(w) => {
                    if let Some((key, value)) = w.split_once(':') {
                        if q.apply_filter(key, value) {
                            continue;
                        }
                    }
                    q.terms.push(w);
                }
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }

    /// True if the filter was recognised and applied.
    fn apply_filter(&mut self, key: &str, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        match key.to_ascii_lowercase().as_str() {
            "ext" | "확장자" => {
                self.exts.extend(
                    value
                        .split(',')
                        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                        .filter(|e| !e.is_empty()),
                );
                true
            }
            "type" | "종류" => match type_exts(&value.to_lowercase()) {
                Some(list) => {
                    self.exts.extend(list.iter().map(|s| s.to_string()));
                    true
                }
                None => false,
            },
            "in" | "폴더" => {
                let p = normalize_lexical(&expand_home(std::path::Path::new(value)));
                self.in_dir = Some(p);
                true
            }
            "after" | "since" | "이후" => match parse_date(value) {
                Some(t) => {
                    self.after = Some(t);
                    true
                }
                None => false,
            },
            "before" | "until" | "이전" => match parse_date(value) {
                Some(t) => {
                    self.before = Some(t);
                    true
                }
                None => false,
            },
            "size" | "크기" => {
                let (op, rest) = match value.as_bytes()[0] {
                    b'>' => ('>', &value[1..]),
                    b'<' => ('<', &value[1..]),
                    _ => ('>', value),
                };
                match parse_size(rest) {
                    Some(n) if op == '>' => {
                        self.min_size = Some(n);
                        true
                    }
                    Some(n) => {
                        self.max_size = Some(n);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }
}

enum Token {
    Word(String),
    Phrase(String),
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    let mut cur = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut phrase = String::new();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    phrase.push(c);
                }
                if cur.ends_with(':') {
                    // `in:"C:/Users/홍 길동/문서"` — quoted filter value.
                    cur.push_str(&phrase);
                    out.push(Token::Word(std::mem::take(&mut cur)));
                } else {
                    if !cur.is_empty() {
                        out.push(Token::Word(std::mem::take(&mut cur)));
                    }
                    out.push(Token::Phrase(phrase));
                }
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(Token::Word(std::mem::take(&mut cur)));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(Token::Word(cur));
    }
    out
}

/// `2026`, `2026-09`, `2026-09-15` (local time, start of the period).
fn parse_date(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split(['-', '.', '/']).collect();
    let (y, m, d) = match parts.as_slice() {
        [y] => (y.parse().ok()?, 1, 1),
        [y, m] => (y.parse().ok()?, m.parse().ok()?, 1),
        [y, m, d] => (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?),
        _ => return None,
    };
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    let local = Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()?;
    Some(local.timestamp())
}

/// `10mb`, `1.5GB`, `500k`, `123` → bytes (binary units).
pub(crate) fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    let idx = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(idx);
    let num: f64 = num.parse().ok()?;
    let mult: u64 = match unit.trim().trim_end_matches("ib").trim_end_matches('b') {
        "" => 1,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        "t" => 1 << 40,
        _ => return None,
    };
    Some((num * mult as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_phrases_and_filters() {
        let q = Query::parse(r#"과금 "KT 내역서" ext:xlsx,.PDF size:>10mb type:image"#);
        assert_eq!(q.terms, vec!["과금", "KT 내역서"]);
        assert!(q.exts.contains(&"xlsx".to_string()));
        assert!(q.exts.contains(&"pdf".to_string()));
        assert!(q.exts.contains(&"png".to_string()));
        assert_eq!(q.min_size, Some(10 << 20));
        assert_eq!(q.max_size, None);
    }

    #[test]
    fn dates_and_folders() {
        let q = Query::parse("보고서 after:2026-09 before:2026-10-15 in:~/Documents/업무");
        assert_eq!(q.terms, vec!["보고서"]);
        assert_eq!(q.after, parse_date("2026-09-01"));
        assert_eq!(q.before, parse_date("2026-10-15"));
        assert!(q.after < q.before);
        let home = dirs::home_dir().unwrap();
        assert_eq!(q.in_dir, Some(home.join("Documents").join("업무")));
        let q = Query::parse(r#"메모 in:"/home/홍 길동/바탕 화면" 추가"#);
        assert_eq!(q.terms, vec!["메모", "추가"]);
        assert_eq!(q.in_dir, Some(PathBuf::from("/home/홍 길동/바탕 화면")));
    }

    #[test]
    fn unknown_or_invalid_filters_stay_words() {
        let q = Query::parse("a:b after:yesterday size:big type:thing ext:");
        assert_eq!(
            q.terms,
            vec!["a:b", "after:yesterday", "size:big", "type:thing", "ext:"]
        );
        assert!(Query::parse("   ").is_empty());
    }
}
