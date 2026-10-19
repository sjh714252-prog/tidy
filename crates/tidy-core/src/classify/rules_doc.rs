//! `rules.toml` ↔ rule cards (SPEC §14.1, M8 visual rule editor).
//!
//! A card is one `[[rule]]` table. [`parse_cards`] reads them (with each
//! table's index as `origin`); [`apply_cards`] writes cards back into the
//! same text with `toml_edit`: tables are re-ordered as the cards are, and
//! a kept table only gets the keys a card manages changed — comments, key
//! order and keys the card does not know (`ai_auto`, `source_app`, future
//! keys) survive. The file's leading comment block stays at the top.

use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use super::rules::{RuleError, RuleSet};

/// One rule as the card editor shows it. Empty strings / `None` mean "not
/// set" (the key is absent from the file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleCard {
    /// Index of the `[[rule]]` table this card came from (`None` = new).
    pub origin: Option<usize>,
    pub name: String,
    pub enabled: bool,
    pub confirm: bool,
    pub ext: Vec<String>,
    pub name_contains: String,
    pub name_regex: String,
    /// Bytes ("1048576") or text ("10MB").
    pub min_size: String,
    pub max_size: String,
    pub older_than_days: Option<u32>,
    pub newer_than_days: Option<u32>,
    pub from_dir: String,
    pub dest: String,
    /// Keys of the rule the card does not edit (shown read-only, kept).
    pub other_keys: Vec<String>,
}

impl Default for RuleCard {
    fn default() -> Self {
        Self {
            origin: None,
            name: String::new(),
            enabled: true,
            confirm: false,
            ext: Vec::new(),
            name_contains: String::new(),
            name_regex: String::new(),
            min_size: String::new(),
            max_size: String::new(),
            older_than_days: None,
            newer_than_days: None,
            from_dir: String::new(),
            dest: String::new(),
            other_keys: Vec::new(),
        }
    }
}

const CARD_KEYS: &[&str] = &["name", "match", "dest", "confirm", "enabled"];
const MATCH_KEYS: &[&str] = &[
    "ext",
    "name_regex",
    "name_contains",
    "min_size",
    "max_size",
    "older_than_days",
    "newer_than_days",
    "from_dir",
];

fn parse_doc(text: &str) -> Result<DocumentMut, RuleError> {
    // Validate with the real loader first: same error messages as `tidy`.
    RuleSet::from_toml(text)?;
    text.parse::<DocumentMut>()
        .map_err(|e| RuleError::Template {
            rule: String::new(),
            msg: e.to_string(),
        })
}

fn tables(doc: &DocumentMut) -> Result<Vec<Table>, RuleError> {
    match doc.get("rule") {
        None => Ok(Vec::new()),
        Some(Item::ArrayOfTables(a)) => Ok(a.iter().cloned().collect()),
        Some(_) => Err(RuleError::Template {
            rule: String::new(),
            msg: "카드로 편집하려면 규칙을 [[rule]] 표로 써야 합니다".into(),
        }),
    }
}

fn str_of(v: Option<&Item>) -> String {
    v.and_then(Item::as_str).unwrap_or_default().to_string()
}

fn size_of(v: Option<&Item>) -> String {
    match v {
        Some(i) if i.as_integer().is_some() => i.as_integer().unwrap_or(0).to_string(),
        Some(i) => i.as_str().unwrap_or_default().to_string(),
        None => String::new(),
    }
}

fn days_of(v: Option<&Item>) -> Option<u32> {
    v.and_then(Item::as_integer)
        .and_then(|n| u32::try_from(n).ok())
}

/// Cards for every `[[rule]]` in `text` (disabled ones included).
pub fn parse_cards(text: &str) -> Result<Vec<RuleCard>, RuleError> {
    let doc = parse_doc(text)?;
    let mut out = Vec::new();
    for (i, t) in tables(&doc)?.iter().enumerate() {
        let m = t.get("match").and_then(Item::as_table_like);
        let mget = |k: &str| m.and_then(|m| m.get(k));
        let mut other: Vec<String> = t
            .iter()
            .map(|(k, _)| k.to_string())
            .filter(|k| !CARD_KEYS.contains(&k.as_str()))
            .collect();
        if let Some(m) = m {
            other.extend(
                m.iter()
                    .map(|(k, _)| k.to_string())
                    .filter(|k| !MATCH_KEYS.contains(&k.as_str()))
                    .map(|k| format!("match.{k}")),
            );
        }
        out.push(RuleCard {
            origin: Some(i),
            name: str_of(t.get("name")),
            enabled: t.get("enabled").and_then(Item::as_bool).unwrap_or(true),
            confirm: t.get("confirm").and_then(Item::as_bool).unwrap_or(false),
            ext: mget("ext")
                .and_then(Item::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            name_contains: str_of(mget("name_contains")),
            name_regex: str_of(mget("name_regex")),
            min_size: size_of(mget("min_size")),
            max_size: size_of(mget("max_size")),
            older_than_days: days_of(mget("older_than_days")),
            newer_than_days: days_of(mget("newer_than_days")),
            from_dir: str_of(mget("from_dir")),
            dest: str_of(t.get("dest")),
            other_keys: other,
        });
    }
    Ok(out)
}

/// Set `key` to `v` unless it already holds an equal value (keeps the
/// original formatting of unchanged values); `None` removes it.
fn set(t: &mut dyn toml_edit::TableLike, key: &str, v: Option<Value>) {
    match v {
        None => {
            t.remove(key);
        }
        Some(v) => {
            let same = t
                .get(key)
                .and_then(Item::as_value)
                .is_some_and(|old| values_equal(old, &v));
            if !same {
                let decorated = match t.get(key).and_then(Item::as_value) {
                    // Keep the comment/space around a changed value.
                    Some(old) => {
                        let mut v = v;
                        *v.decor_mut() = old.decor().clone();
                        v
                    }
                    None => v,
                };
                t.insert(key, Item::Value(decorated));
            }
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Integer(x), Value::Integer(y)) => x.value() == y.value(),
        (Value::Boolean(x), Value::Boolean(y)) => x.value() == y.value(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b))
        }
        _ => false,
    }
}

fn s(v: &str) -> Option<Value> {
    let v = v.trim();
    (!v.is_empty()).then(|| Value::from(v))
}

fn size_value(v: &str) -> Option<Value> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    Some(match v.parse::<i64>() {
        Ok(n) => Value::from(n),
        Err(_) => Value::from(v),
    })
}

/// Bool keys: written when not the default, kept (updated) when present.
fn set_bool(t: &mut Table, key: &str, v: bool, default: bool) {
    if t.contains_key(key) || v != default {
        set(t, key, Some(Value::from(v)));
    }
}

fn fill(t: &mut Table, c: &RuleCard) {
    set(t, "name", Some(Value::from(c.name.trim())));
    {
        if !t.contains_key("match") {
            let mut it = InlineTable::new();
            it.set_dotted(false);
            t.insert("match", Item::Value(Value::InlineTable(it)));
        }
        let m = t
            .get_mut("match")
            .and_then(Item::as_table_like_mut)
            .expect("match is a table");
        let ext: Vec<String> = c
            .ext
            .iter()
            .map(|e| e.trim().trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        set(
            m,
            "ext",
            (!ext.is_empty()).then(|| {
                let mut a: Array = ext.iter().map(String::as_str).collect();
                a.fmt();
                Value::Array(a)
            }),
        );
        set(m, "name_contains", s(&c.name_contains));
        set(m, "name_regex", s(&c.name_regex));
        set(m, "min_size", size_value(&c.min_size));
        set(m, "max_size", size_value(&c.max_size));
        set(
            m,
            "older_than_days",
            c.older_than_days.map(|d| Value::from(i64::from(d))),
        );
        set(
            m,
            "newer_than_days",
            c.newer_than_days.map(|d| Value::from(i64::from(d))),
        );
        set(m, "from_dir", s(&c.from_dir));
        if m.is_empty() {
            t.remove("match");
        } else if let Some(it) = t.get_mut("match").and_then(Item::as_inline_table_mut) {
            it.fmt();
        }
    }
    set(t, "dest", Some(Value::from(c.dest.trim())));
    set_bool(t, "confirm", c.confirm, false);
    set_bool(t, "enabled", c.enabled, true);
}

/// Split a table prefix into the file header (up to the last blank line)
/// and the rule's own comment.
fn split_header(prefix: &str) -> (String, String) {
    match prefix.rfind("\n\n") {
        Some(i) => (prefix[..i + 2].to_string(), prefix[i + 2..].to_string()),
        None => (String::new(), prefix.to_string()),
    }
}

/// Write `cards` (in order) into `text`, see the module docs. The result
/// is validated like any rules file.
pub fn apply_cards(text: &str, cards: &[RuleCard]) -> Result<String, RuleError> {
    let mut doc = if text.trim().is_empty() {
        DocumentMut::new()
    } else {
        parse_doc(text)?
    };
    let orig = tables(&doc)?;
    let first_pos = orig.iter().filter_map(Table::position).min().unwrap_or(0);
    let (header, first_own) = match orig.first() {
        Some(t) => t
            .decor()
            .prefix()
            .and_then(|p| p.as_str())
            .map(split_header)
            .unwrap_or_default(),
        // A file with only comments keeps them as the document's trailing
        // text: they become the header in front of the first rule.
        None => {
            let t = doc.trailing().as_str().unwrap_or_default().to_string();
            if !cards.is_empty() {
                doc.set_trailing("");
            }
            let mut h = t;
            if !h.is_empty() && !h.ends_with("\n\n") {
                h.push('\n');
            }
            (h, String::new())
        }
    };
    let mut out = toml_edit::ArrayOfTables::new();
    for (k, c) in cards.iter().enumerate() {
        let mut t = match c.origin.and_then(|i| orig.get(i)) {
            Some(t) => t.clone(),
            None => {
                let mut t = Table::new();
                t.decor_mut().set_prefix("\n");
                t
            }
        };
        if c.origin == Some(0) {
            t.decor_mut().set_prefix(if k == 0 {
                format!("{header}{first_own}")
            } else {
                format!("\n{first_own}")
            });
        } else if k == 0 {
            let own = t
                .decor()
                .prefix()
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .trim_start_matches('\n')
                .to_string();
            t.decor_mut().set_prefix(format!("{header}{own}"));
        }
        fill(&mut t, c);
        t.set_position(first_pos + k);
        out.push(t);
    }
    if out.is_empty() {
        doc.remove("rule");
        if !header.is_empty() && doc.as_table().is_empty() {
            // Keep the explanation comments of an emptied file.
            let mut text = header;
            if !text.ends_with('\n') {
                text.push('\n');
            }
            return Ok(text);
        }
    } else {
        doc.insert("rule", Item::ArrayOfTables(out));
    }
    let text = doc.to_string();
    RuleSet::from_toml(&text)?;
    Ok(text)
}

/// Append `extra` cards to the rules in `text`, skipping any whose name
/// is already there (re-running the wizard never duplicates or touches
/// existing rules).
pub fn append_cards(text: &str, extra: &[RuleCard]) -> Result<String, RuleError> {
    let mut cards = if text.trim().is_empty() {
        Vec::new()
    } else {
        parse_cards(text)?
    };
    for c in extra {
        if !cards.iter().any(|e| e.name.trim() == c.name.trim()) {
            cards.push(RuleCard {
                origin: None,
                ..c.clone()
            });
        }
    }
    apply_cards(text, &cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"# 내 규칙
# 위에서부터 검사

# 캡처는 따로
[[rule]]
name = "스크린샷"
match = { ext = ["png", "jpg"], name_regex = "^스크린샷" }  # 화면 캡처
dest = "캡처/{yyyy}-{mm}"
ai_auto = true

[[rule]]
name = "설치파일"
match = { ext = ["exe"] }
dest = "설치파일"
confirm = true
"#;

    #[test]
    fn parse_reads_cards_and_unknown_keys() {
        let c = parse_cards(FILE).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].origin, Some(0));
        assert_eq!(c[0].ext, ["png", "jpg"]);
        assert_eq!(c[0].name_regex, "^스크린샷");
        assert_eq!(c[0].other_keys, ["ai_auto"]);
        assert!(c[1].confirm && c[1].enabled);
    }

    #[test]
    fn unchanged_cards_round_trip_byte_for_byte() {
        let c = parse_cards(FILE).unwrap();
        assert_eq!(apply_cards(FILE, &c).unwrap(), FILE);
    }

    #[test]
    fn reorder_edit_add_remove_keep_comments_and_unknown_keys() {
        let mut c = parse_cards(FILE).unwrap();
        c.swap(0, 1);
        c[1].ext.push("jpeg".into());
        c[1].name_contains = "캡처".into();
        c.push(RuleCard {
            name: "압축".into(),
            ext: vec!["zip".into()],
            dest: "{root}/압축 파일".into(),
            ..Default::default()
        });
        let out = apply_cards(FILE, &c).unwrap();
        // Header stays at the top, the moved rule keeps its own comment
        // and its unknown key.
        assert!(
            out.starts_with("# 내 규칙\n# 위에서부터 검사\n\n[[rule]]\nname = \"설치파일\""),
            "{out}"
        );
        assert!(
            out.contains("# 캡처는 따로\n[[rule]]\nname = \"스크린샷\""),
            "{out}"
        );
        assert!(out.contains("ai_auto = true"));
        assert!(out.contains("# 화면 캡처"), "{out}");
        assert!(out.contains(r#"ext = ["png", "jpg", "jpeg"]"#), "{out}");
        assert!(out.contains(r#"name_contains = "캡처""#), "{out}");
        assert!(out.contains("dest = \"{root}/압축 파일\""), "{out}");
        let back = parse_cards(&out).unwrap();
        assert_eq!(
            back.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["설치파일", "스크린샷", "압축"]
        );
        // Remove one: the others stay intact.
        let only = vec![back[2].clone()];
        let out2 = apply_cards(&out, &only).unwrap();
        assert!(out2.starts_with("# 내 규칙"), "{out2}");
        assert_eq!(parse_cards(&out2).unwrap().len(), 1);
    }

    #[test]
    fn bools_and_sizes() {
        let mut c = parse_cards(FILE).unwrap();
        c[1].confirm = false;
        c[0].enabled = false;
        c[0].min_size = "10MB".into();
        c[0].max_size = "2048".into();
        c[0].newer_than_days = Some(7);
        let out = apply_cards(FILE, &c).unwrap();
        assert!(out.contains("confirm = false"), "{out}");
        assert!(out.contains("enabled = false"), "{out}");
        assert!(
            out.contains(r#"min_size = "10MB""#) && out.contains("max_size = 2048"),
            "{out}"
        );
        let back = parse_cards(&out).unwrap();
        assert_eq!(back[0].newer_than_days, Some(7));
        assert_eq!(back[0].max_size, "2048");
    }

    #[test]
    fn comment_only_file_keeps_its_header_on_top() {
        let head = "# 설명\n# 둘째 줄\n\n";
        let out = apply_cards(
            head,
            &[RuleCard {
                name: "문서".into(),
                ext: vec!["pdf".into()],
                dest: "문서".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        assert!(out.starts_with("# 설명\n# 둘째 줄\n\n[[rule]]"), "{out}");
        assert_eq!(out.matches("# 설명").count(), 1);
        let more = append_cards("# 한 줄\n", &parse_cards(&out).unwrap()).unwrap();
        assert!(more.starts_with("# 한 줄\n\n[[rule]]"), "{more}");
    }

    #[test]
    fn empty_and_invalid() {
        let out = apply_cards(
            "",
            &[RuleCard {
                name: "문서".into(),
                ext: vec!["PDF".into(), ".hwp".into()],
                dest: "문서/{yyyy}-{mm}".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        assert!(out.contains(r#"ext = ["pdf", "hwp"]"#), "{out}");
        // A card that makes the file invalid is refused.
        let bad = RuleCard {
            name: "x".into(),
            dest: "{year}".into(),
            ..Default::default()
        };
        assert!(apply_cards("", &[bad]).is_err());
        assert!(parse_cards("[[rule]]\nname='x'\n").is_err());
        // Appending skips names that exist and keeps the rest as is.
        let more = append_cards(
            FILE,
            &[
                RuleCard {
                    name: "설치파일".into(),
                    dest: "다른 곳".into(),
                    ..Default::default()
                },
                RuleCard {
                    name: "압축".into(),
                    ext: vec!["zip".into()],
                    dest: "압축 파일".into(),
                    ..Default::default()
                },
            ],
        )
        .unwrap();
        assert!(more.starts_with(FILE), "{more}");
        assert_eq!(parse_cards(&more).unwrap().len(), 3);
        // Removing every rule keeps the explanation comments.
        assert_eq!(
            apply_cards(FILE, &[]).unwrap(),
            "# 내 규칙\n# 위에서부터 검사\n\n"
        );
    }
}
