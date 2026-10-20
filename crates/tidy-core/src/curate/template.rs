//! Work-context folder layouts (SPEC §14.3): `{entity}/{yyyymmdd}_{issue}/{doc_kind}`
//! for files with a customer, `기타/{topic}` without one. Same `{var}`
//! syntax as rule destinations; always relative, resolved against the watch
//! folder's anchor (in_place: the folder itself, collect: organize_root).

use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};

const VARS: &[&str] = &[
    "entity", "issue", "doc_kind", "topic", "yyyy", "mm", "dd", "yyyymmdd", "root",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Lit(String),
    Var(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTemplate {
    text: String,
    segs: Vec<Seg>,
}

/// Values a template is filled with.
#[derive(Debug, Clone, Default)]
pub struct TemplateVals<'a> {
    pub entity: Option<&'a str>,
    pub issue: &'a str,
    pub doc_kind: &'a str,
    /// "큰 주제/작은 주제".
    pub topic: Option<&'a str>,
    pub date: Option<NaiveDate>,
}

impl FolderTemplate {
    pub fn parse(text: &str) -> Result<Self, String> {
        let t = text.trim().trim_end_matches(['/', '\\']);
        if t.is_empty() {
            return Err("폴더 틀이 비어 있습니다".into());
        }
        if t.starts_with('/')
            || t.starts_with('\\')
            || t.starts_with('~')
            || t.chars().nth(1) == Some(':')
        {
            return Err(format!("폴더 틀은 상대 경로여야 합니다: {t}"));
        }
        if t.split(['/', '\\']).any(|c| c == "..") {
            return Err(format!("폴더 틀에 '..'은 쓸 수 없습니다: {t}"));
        }
        let mut segs = Vec::new();
        let mut rest = t;
        while let Some(open) = rest.find('{') {
            if open > 0 {
                segs.push(Seg::Lit(rest[..open].to_string()));
            }
            let close = rest[open..]
                .find('}')
                .ok_or_else(|| format!("닫는 }}가 없습니다: {t}"))?
                + open;
            let var = &rest[open + 1..close];
            if !VARS.contains(&var) {
                return Err(format!(
                    "알 수 없는 변수 {{{var}}} (쓸 수 있는 것: {})",
                    VARS.iter()
                        .map(|v| format!("{{{v}}}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
            segs.push(Seg::Var(var.to_string()));
            rest = &rest[close + 1..];
        }
        if !rest.is_empty() {
            segs.push(Seg::Lit(rest.to_string()));
        }
        Ok(Self {
            text: t.to_string(),
            segs,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn uses(&self, var: &str) -> bool {
        self.segs
            .iter()
            .any(|s| matches!(s, Seg::Var(v) if v == var))
    }

    /// The folder for `vals` under `anchor`.
    pub fn render(&self, vals: &TemplateVals<'_>, anchor: &Path) -> PathBuf {
        let date = vals.date;
        let mut s = String::new();
        for seg in &self.segs {
            match seg {
                Seg::Lit(l) => s.push_str(l),
                Seg::Var(v) => {
                    let val = match v.as_str() {
                        "entity" => clean(vals.entity.unwrap_or("기타")),
                        "issue" => clean(or(vals.issue, "기타 이슈")),
                        "doc_kind" => clean(or(vals.doc_kind, "기타 자료")),
                        // Up to two levels; each level cleaned on its own.
                        "topic" => {
                            let parts: Vec<String> = vals
                                .topic
                                .unwrap_or("")
                                .split(['/', '\\', '>'])
                                .map(clean)
                                .filter(|p| !p.is_empty())
                                .take(2)
                                .collect();
                            if parts.is_empty() {
                                "미분류".into()
                            } else {
                                parts.join("/")
                            }
                        }
                        "yyyy" => date.map(|d| format!("{:04}", d.year())).unwrap_or_default(),
                        "mm" => date
                            .map(|d| format!("{:02}", d.month()))
                            .unwrap_or_default(),
                        "dd" => date.map(|d| format!("{:02}", d.day())).unwrap_or_default(),
                        "yyyymmdd" => date
                            .map(|d| d.format("%Y%m%d").to_string())
                            .unwrap_or_else(|| "날짜없음".into()),
                        _ => String::new(), // {root}: the anchor itself
                    };
                    s.push_str(&val);
                }
            }
        }
        let mut out = anchor.to_path_buf();
        for c in s
            .split(['/', '\\'])
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            out.push(c.trim_matches(['.', ' ']));
        }
        out
    }
}

fn or<'a>(v: &'a str, d: &'a str) -> &'a str {
    if v.trim().is_empty() {
        d
    } else {
        v
    }
}

/// One folder name from an AI value: no separators or characters Windows
/// refuses, no leading/trailing dots or spaces, at most 60 characters.
pub fn clean(v: &str) -> String {
    let s: String = v
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches(['.', ' ']).to_string();
    s.chars().take(60).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, dd)
    }

    #[test]
    fn default_layouts() {
        let t = FolderTemplate::parse("{entity}/{yyyymmdd}_{issue}/{doc_kind}/").unwrap();
        let a = Path::new("/바탕 화면");
        let p = t.render(
            &TemplateVals {
                entity: Some("가람정보"),
                issue: "과금이슈",
                doc_kind: "로그",
                topic: None,
                date: d(2026, 10, 20),
            },
            a,
        );
        assert_eq!(p, a.join("가람정보").join("20261020_과금이슈").join("로그"));
        let o = FolderTemplate::parse("기타/{topic}").unwrap();
        let p = o.render(
            &TemplateVals {
                topic: Some("정책 / 문자 정책 / 세부"),
                ..Default::default()
            },
            a,
        );
        assert_eq!(p, a.join("기타").join("정책").join("문자 정책"));
    }

    #[test]
    fn values_cannot_escape_or_break_paths() {
        let t = FolderTemplate::parse("{root}/{entity}/{issue}").unwrap();
        let a = Path::new("/r");
        let p = t.render(
            &TemplateVals {
                entity: Some("../../etc"),
                issue: "a:b*c?",
                ..Default::default()
            },
            a,
        );
        assert!(p.starts_with(a));
        assert_eq!(p, a.join("etc").join("a b c"));
        for bad in ["/abs", "C:/x", "~/x", "../up", "{nope}", "{entity"] {
            assert!(FolderTemplate::parse(bad).is_err(), "{bad}");
        }
    }
}
