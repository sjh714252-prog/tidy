//! Learner (SPEC §5.2): when the user sends files with the same pattern to
//! the same folder `learn_threshold` times (default 3), propose a rule
//! ("이 규칙을 자동화할까요?"). Accepting writes the rule to the top of
//! `rules.toml` (more specific than the hand-written rules below it);
//! dismissing remembers the pattern so it is not proposed again.
//!
//! Patterns: same extension AND (same leading name part, e.g. `KT과금내역서`
//! from `KT과금내역서_2026-09.xlsx`, OR same source folder).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use super::{FileFacts, RuleError, RuleSet};
use crate::store::{Decision, Store, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum LearnError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Rules(#[from] RuleError),
    #[error("cannot read rules file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no such suggestion: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Pattern {
    /// File names starting with this text.
    NamePrefix(String),
    /// Files appearing directly in this folder.
    FromDir(PathBuf),
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleSuggestion {
    /// Stable id (also the dismissal key).
    pub key: String,
    pub ext: String,
    pub pattern: Pattern,
    pub dest: PathBuf,
    /// How many matching decisions chose `dest`.
    pub count: u32,
    /// Example file names behind the suggestion.
    pub examples: Vec<String>,
    pub rule_name: String,
    /// The `[[rule]]` block that accepting adds.
    pub rule_toml: String,
}

/// Leading name part used as a pattern: up to the first digit or
/// separator-like character (`_ - . ( [`), trimmed. `None` if shorter than
/// two characters.
pub fn name_prefix(file_name: &str) -> Option<String> {
    let stem = Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cut: String = stem
        .chars()
        .take_while(|c| !(c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '(' | '[' | '{')))
        .collect();
    let p = cut.trim().to_string();
    (p.chars().count() >= 2).then_some(p)
}

/// Current suggestions, strongest first.
pub fn suggestions(
    store: &Store,
    rules: &RuleSet,
    threshold: u32,
) -> Result<Vec<RuleSuggestion>, LearnError> {
    let threshold = threshold.max(1);
    // (kind, ext, pattern, dest) -> decisions
    let mut groups: BTreeMap<(u8, String, String, PathBuf), Vec<Decision>> = BTreeMap::new();
    for d in store.move_decisions()? {
        let Some(dest) = d.chosen_dest.clone() else {
            continue;
        };
        let ext = d.ext.clone().unwrap_or_default();
        if let Some(prefix) = name_prefix(&d.file_name) {
            groups
                .entry((0, ext.clone(), prefix, dest.clone()))
                .or_default()
                .push(d.clone());
        }
        groups
            .entry((1, ext, d.src_dir.display().to_string(), dest))
            .or_default()
            .push(d);
    }

    let mut out: Vec<RuleSuggestion> = Vec::new();
    for ((kind, ext, pat, dest), ds) in groups {
        let count = ds.len() as u32;
        if count < threshold {
            continue;
        }
        let pattern = if kind == 0 {
            Pattern::NamePrefix(pat)
        } else {
            Pattern::FromDir(PathBuf::from(pat))
        };
        // A folder rule adds nothing if a name rule already covers the
        // same files and destination.
        if matches!(pattern, Pattern::FromDir(_))
            && out
                .iter()
                .any(|s| s.ext == ext && s.dest == dest && s.count >= count)
        {
            continue;
        }
        let key = format!(
            "{}|{ext}|{}|{}",
            if kind == 0 { "prefix" } else { "dir" },
            match &pattern {
                Pattern::NamePrefix(p) => p.clone(),
                Pattern::FromDir(d) => d.display().to_string(),
            },
            dest.display()
        );
        if store.is_learn_dismissed(&key)? || covered(rules, &ds[ds.len() - 1], &dest) {
            continue;
        }
        let rule_name = rule_name(&pattern, &ext, &dest);
        let rule_toml = rule_toml(&rule_name, &ext, &pattern, &dest, count);
        out.push(RuleSuggestion {
            key,
            ext,
            pattern,
            dest,
            count,
            examples: ds
                .iter()
                .rev()
                .take(3)
                .map(|d| d.file_name.clone())
                .collect(),
            rule_name,
            rule_toml,
        });
    }
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    Ok(out)
}

/// Already handled without asking by an existing rule?
fn covered(rules: &RuleSet, sample: &Decision, dest: &Path) -> bool {
    let path = sample.src_dir.join(&sample.file_name);
    let facts = FileFacts {
        name: sample.file_name.clone(),
        stem: Path::new(&sample.file_name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        ext: sample.ext.clone().unwrap_or_default(),
        size: 0,
        modified: SystemTime::now(),
        path,
    };
    rules
        .classify(&facts)
        .is_some_and(|c| !c.confirm && c.dest_dir == dest)
}

fn rule_name(p: &Pattern, ext: &str, dest: &Path) -> String {
    let to = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dest.display().to_string());
    match p {
        Pattern::NamePrefix(prefix) => format!("학습: {prefix} → {to}"),
        Pattern::FromDir(dir) => format!(
            "학습: {} .{ext} → {to}",
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
    }
}

/// TOML strings via the `toml` serializer (correct escaping for Korean,
/// quotes and Windows backslashes).
fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn rule_toml(name: &str, ext: &str, p: &Pattern, dest: &Path, count: u32) -> String {
    let mut m = Vec::new();
    if !ext.is_empty() {
        m.push(format!("ext = [{}]", toml_str(ext)));
    }
    match p {
        Pattern::NamePrefix(prefix) => m.push(format!(
            "name_regex = {}",
            toml_str(&format!("^{}", regex::escape(prefix)))
        )),
        Pattern::FromDir(dir) => m.push(format!(
            "from_dir = {}",
            toml_str(&dir.display().to_string())
        )),
    }
    format!(
        "# 학습으로 추가됨 ({}): 같은 결정 {count}회\n[[rule]]\nname = {}\nmatch = {{ {} }}\ndest = {}\n",
        chrono::Local::now().format("%Y-%m-%d"),
        toml_str(name),
        m.join(", "),
        toml_str(&dest.display().to_string()),
    )
}

/// Insert `rule_toml` before the first `[[rule]]` of the rules text (after
/// the leading comments), or append when there is none.
fn insert_rule(text: &str, rule_toml: &str) -> String {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("[[rule]]") {
            // Keep a comment block directly above the first rule with it.
            let head = &text[..offset];
            let mut cut = head.len();
            for l in head.split_inclusive('\n').rev() {
                let t = l.trim();
                if t.starts_with('#') && !t.is_empty() {
                    cut -= l.len();
                } else {
                    break;
                }
            }
            return format!("{}{}\n{}", &text[..cut], rule_toml, &text[cut..]);
        }
        offset += line.len();
    }
    let sep = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    format!("{text}{sep}{rule_toml}")
}

/// Accept suggestion `key`: validate and write the new rules file, and
/// return the reloaded rules.
pub fn accept(
    store: &Store,
    rules_path: &Path,
    threshold: u32,
    key: &str,
) -> Result<RuleSet, LearnError> {
    let text = match std::fs::read_to_string(rules_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(LearnError::Read {
                path: rules_path.to_path_buf(),
                source,
            })
        }
    };
    let current = RuleSet::from_toml(&text)?;
    let s = suggestions(store, &current, threshold)?
        .into_iter()
        .find(|s| s.key == key)
        .ok_or_else(|| LearnError::NotFound(key.to_string()))?;
    let new_text = insert_rule(&text, &s.rule_toml);
    Ok(RuleSet::save_text(rules_path, &new_text)?)
}

pub fn dismiss(store: &Store, key: &str) -> Result<(), LearnError> {
    Ok(store.dismiss_learn(key)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DecisionAction, NewDecision, PendingSource};

    fn decide(s: &Store, name: &str, src: &Path, dest: &Path) {
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase());
        s.insert_decision(&NewDecision {
            file_name: name,
            ext: ext.as_deref(),
            src_dir: src,
            suggested_dest: None,
            chosen_dest: Some(dest),
            action: DecisionAction::Move,
            source: PendingSource::Ai,
        })
        .unwrap();
    }

    #[test]
    fn prefixes() {
        assert_eq!(
            name_prefix("KT과금내역서_2026-09.xlsx").as_deref(),
            Some("KT과금내역서")
        );
        assert_eq!(
            name_prefix("화면 캡처 2026-10-15 101010.png").as_deref(),
            Some("화면 캡처")
        );
        assert_eq!(name_prefix("보고서(최종).hwp").as_deref(), Some("보고서"));
        assert_eq!(name_prefix("2026-09.pdf"), None);
        assert_eq!(name_prefix("a1.txt"), None);
    }

    #[test]
    fn threshold_then_accept_then_rule_applies() {
        let tmp = tempfile::tempdir().unwrap();
        let desk = tmp.path().join("바탕 화면");
        let dest = tmp.path().join("업무 문서").join("KT 과금");
        let s = Store::open_in_memory().unwrap();
        let rules_path = tmp.path().join("rules.toml");
        std::fs::write(
            &rules_path,
            "# 내 규칙\n\n# 스크린샷\n[[rule]]\nname = \"엑셀\"\nmatch = { ext = [\"xlsx\"] }\ndest = '/다른 곳'\nconfirm = true\n",
        )
        .unwrap();
        let rules = RuleSet::load(&rules_path).unwrap();

        decide(&s, "KT과금내역서_2026-07.xlsx", &desk, &dest);
        decide(&s, "KT과금내역서_2026-08.xlsx", &desk, &dest);
        assert!(suggestions(&s, &rules, 3).unwrap().is_empty());
        decide(&s, "KT과금내역서_2026-09.xlsx", &desk, &dest);

        let sug = suggestions(&s, &rules, 3).unwrap();
        // The name rule wins; the equivalent folder rule is not repeated.
        assert_eq!(sug.len(), 1, "{sug:#?}");
        assert_eq!(sug[0].pattern, Pattern::NamePrefix("KT과금내역서".into()));
        assert_eq!(sug[0].count, 3);
        assert_eq!(sug[0].examples[0], "KT과금내역서_2026-09.xlsx");

        let new_rules = accept(&s, &rules_path, 3, &sug[0].key).unwrap();
        let text = std::fs::read_to_string(&rules_path).unwrap();
        // Inserted before the first rule (and its comment), header kept.
        assert!(text.starts_with("# 내 규칙\n\n# 학습으로 추가됨"), "{text}");
        assert!(text.contains("# 스크린샷\n[[rule]]\nname = \"엑셀\""));
        let f = desk.join("KT과금내역서_2026-10.xlsx");
        let facts = FileFacts {
            path: f,
            name: "KT과금내역서_2026-10.xlsx".into(),
            stem: "KT과금내역서_2026-10".into(),
            ext: "xlsx".into(),
            size: 1,
            modified: SystemTime::now(),
        };
        let c = new_rules.classify(&facts).unwrap();
        assert_eq!(c.dest_dir, dest);
        assert!(!c.confirm);
        // Now covered: no longer suggested.
        assert!(suggestions(&s, &new_rules, 3).unwrap().is_empty());
    }

    #[test]
    fn dismissed_and_folder_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        let dl = tmp.path().join("다운로드");
        let dest = tmp.path().join("영수증");
        let s = Store::open_in_memory().unwrap();
        for n in ["a1.pdf", "b2.pdf", "c3.pdf"] {
            decide(&s, n, &dl, &dest);
        }
        let sug = suggestions(&s, &RuleSet::default(), 3).unwrap();
        assert_eq!(sug.len(), 1);
        assert_eq!(sug[0].pattern, Pattern::FromDir(dl.clone()));
        assert!(sug[0].rule_toml.contains("from_dir = "));
        // The generated block parses as a valid rule.
        assert_eq!(
            RuleSet::from_toml(&sug[0].rule_toml).unwrap().rules().len(),
            1
        );
        dismiss(&s, &sug[0].key).unwrap();
        assert!(suggestions(&s, &RuleSet::default(), 3).unwrap().is_empty());
    }

    #[test]
    fn insert_into_empty_or_ruleless_files() {
        assert_eq!(insert_rule("", "R\n"), "R\n");
        assert_eq!(insert_rule("# c\n", "R\n"), "# c\n\nR\n");
    }
}
