//! The context dictionary `context.toml` (SPEC §14.3): customers /
//! projects with aliases, issue types and kinds of material. The AI gets
//! it with every work-context request and answers in its names; corrections
//! the user makes are offered back as additions (alias, new customer, new
//! issue type) and written only when confirmed.
//!
//! Writing this file is configuration, like `rules.toml` — no user file
//! is touched here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("이름이 비어 있습니다")]
    EmptyName,
    #[error("사전에 없는 고객사입니다: {0}")]
    UnknownEntity(String),
}

/// A customer or project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Entity {
    pub name: String,
    /// Other spellings: English name, abbreviations ("BADA", "바다").
    pub aliases: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IssueType {
    pub name: String,
}

/// A kind of material, with extensions that usually mean it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DocKind {
    pub name: String,
    pub ext: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ContextDict {
    #[serde(rename = "entity")]
    pub entities: Vec<Entity>,
    #[serde(rename = "issue_type")]
    pub issue_types: Vec<IssueType>,
    #[serde(rename = "doc_kind")]
    pub doc_kinds: Vec<DocKind>,
}

/// An addition the user is asked to confirm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextSuggestion {
    AddEntity { name: String },
    AddAlias { entity: String, alias: String },
    AddIssueType { name: String },
}

impl ContextSuggestion {
    pub fn label(&self) -> String {
        match self {
            ContextSuggestion::AddEntity { name } => format!("고객사 '{name}' 추가"),
            ContextSuggestion::AddAlias { entity, alias } => {
                format!("'{entity}'의 별칭으로 '{alias}' 추가")
            }
            ContextSuggestion::AddIssueType { name } => format!("이슈 유형 '{name}' 추가"),
        }
    }
}

const HEADER: &str = "\
# Tidy 업무 맥락 사전 (SPEC §14.3). 설정 → 업무 맥락에서 고칠 수 있습니다.
# [[entity]]     고객사·프로젝트: name, aliases(영문·약칭), notes
# [[issue_type]] 이슈 유형: name
# [[doc_kind]]   자료 종류: name, ext(이 확장자면 보통 이 종류)
";

fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl ContextDict {
    /// Issue types and kinds of material to start with; no customers.
    pub fn starter() -> Self {
        let kinds: &[(&str, &[&str])] = &[
            ("로그", &["log", "txt", "out"]),
            ("안내문서", &["hwp", "hwpx", "doc", "docx", "pdf"]),
            ("계약", &[]),
            ("회의록", &[]),
            ("캡처", &["png", "jpg", "jpeg", "gif", "webp"]),
            ("데이터", &["xlsx", "xls", "csv", "json"]),
            ("발표", &["ppt", "pptx"]),
        ];
        Self {
            entities: Vec::new(),
            issue_types: ["과금", "장애", "전환", "점검", "제안"]
                .iter()
                .map(|n| IssueType { name: (*n).into() })
                .collect(),
            doc_kinds: kinds
                .iter()
                .map(|(n, e)| DocKind {
                    name: (*n).into(),
                    ext: e.iter().map(|x| (*x).into()).collect(),
                })
                .collect(),
        }
    }

    /// Read `path`; a missing file is the starter dictionary.
    pub fn load(path: &Path) -> Result<Self, ContextError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::starter()),
            Err(source) => {
                return Err(ContextError::Read {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        Self::from_toml(&text).map_err(|source| ContextError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut d: ContextDict = toml::from_str(text)?;
        d.normalize();
        Ok(d)
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string(self).unwrap_or_default();
        format!("{HEADER}\n{body}")
    }

    pub fn save(&self, path: &Path) -> Result<(), ContextError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| ContextError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, self.to_toml()).map_err(|source| ContextError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Trim names, drop empty and duplicate entries.
    pub fn normalize(&mut self) {
        let mut ents: Vec<Entity> = Vec::new();
        for mut e in std::mem::take(&mut self.entities) {
            e.name = e.name.trim().to_string();
            if e.name.is_empty() {
                continue;
            }
            let mut aliases: Vec<String> = Vec::new();
            for a in e.aliases.iter().map(|a| a.trim().to_string()) {
                if !a.is_empty() && !same(&a, &e.name) && !aliases.iter().any(|x| same(x, &a)) {
                    aliases.push(a);
                }
            }
            e.aliases = aliases;
            match ents.iter_mut().find(|x| same(&x.name, &e.name)) {
                Some(x) => {
                    for a in e.aliases {
                        if !x.aliases.iter().any(|y| same(y, &a)) {
                            x.aliases.push(a);
                        }
                    }
                }
                None => ents.push(e),
            }
        }
        self.entities = ents;
        let mut issues: Vec<IssueType> = Vec::new();
        for i in std::mem::take(&mut self.issue_types) {
            let n = i.name.trim().to_string();
            if !n.is_empty() && !issues.iter().any(|x| same(&x.name, &n)) {
                issues.push(IssueType { name: n });
            }
        }
        self.issue_types = issues;
        let mut kinds: Vec<DocKind> = Vec::new();
        for k in std::mem::take(&mut self.doc_kinds) {
            let n = k.name.trim().to_string();
            if !n.is_empty() && !kinds.iter().any(|x| same(&x.name, &n)) {
                kinds.push(DocKind {
                    name: n,
                    ext: k
                        .ext
                        .iter()
                        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                        .filter(|e| !e.is_empty())
                        .collect(),
                });
            }
        }
        self.doc_kinds = kinds;
    }

    /// The dictionary name for `name` (itself or an alias), if known.
    pub fn canonical_entity(&self, name: &str) -> Option<&str> {
        let n = name.trim();
        if n.is_empty() {
            return None;
        }
        self.entities
            .iter()
            .find(|e| same(&e.name, n) || e.aliases.iter().any(|a| same(a, n)))
            .map(|e| e.name.as_str())
    }

    pub fn has_issue_type(&self, name: &str) -> bool {
        self.issue_types.iter().any(|i| same(&i.name, name))
    }

    pub fn doc_kind_for_ext(&self, ext: &str) -> Option<&str> {
        self.doc_kinds
            .iter()
            .find(|k| k.ext.iter().any(|e| e == ext))
            .map(|k| k.name.as_str())
    }

    pub fn add_entity(&mut self, name: &str, aliases: &[String]) -> Result<bool, ContextError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ContextError::EmptyName);
        }
        let before = self.clone();
        self.entities.push(Entity {
            name: name.into(),
            aliases: aliases.to_vec(),
            notes: String::new(),
        });
        self.normalize();
        Ok(*self != before)
    }

    pub fn add_alias(&mut self, entity: &str, alias: &str) -> Result<bool, ContextError> {
        let alias = alias.trim();
        if alias.is_empty() {
            return Err(ContextError::EmptyName);
        }
        let Some(canon) = self.canonical_entity(entity).map(str::to_string) else {
            return Err(ContextError::UnknownEntity(entity.into()));
        };
        let e = self
            .entities
            .iter_mut()
            .find(|e| e.name == canon)
            .expect("canonical entity exists");
        if same(&e.name, alias) || e.aliases.iter().any(|a| same(a, alias)) {
            return Ok(false);
        }
        e.aliases.push(alias.into());
        Ok(true)
    }

    pub fn add_issue_type(&mut self, name: &str) -> Result<bool, ContextError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ContextError::EmptyName);
        }
        if self.has_issue_type(name) {
            return Ok(false);
        }
        self.issue_types.push(IssueType { name: name.into() });
        Ok(true)
    }

    pub fn add_doc_kind(&mut self, name: &str, ext: &[String]) -> Result<bool, ContextError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ContextError::EmptyName);
        }
        let before = self.clone();
        self.doc_kinds.push(DocKind {
            name: name.into(),
            ext: ext.to_vec(),
        });
        self.normalize();
        Ok(*self != before)
    }

    /// Apply a confirmed suggestion. `Ok(false)` when it was already there.
    pub fn apply(&mut self, s: &ContextSuggestion) -> Result<bool, ContextError> {
        match s {
            ContextSuggestion::AddEntity { name } => self.add_entity(name, &[]),
            ContextSuggestion::AddAlias { entity, alias } => {
                if self.canonical_entity(entity).is_none() {
                    self.add_entity(entity, &[])?;
                }
                self.add_alias(entity, alias)
            }
            ContextSuggestion::AddIssueType { name } => self.add_issue_type(name),
        }
    }

    /// What a correction teaches: the user put a file under `entity` /
    /// `issue_type` where the AI said `ai_entity` / `ai_issue`.
    pub fn suggestions_for(
        &self,
        entity: Option<&str>,
        ai_entity: Option<&str>,
        issue_type: Option<&str>,
    ) -> Vec<ContextSuggestion> {
        let mut out = Vec::new();
        if let Some(e) = entity.map(str::trim).filter(|e| !e.is_empty()) {
            if self.canonical_entity(e).is_none() {
                out.push(ContextSuggestion::AddEntity { name: e.into() });
            }
            if let Some(ai) = ai_entity.map(str::trim).filter(|a| !a.is_empty()) {
                let ai_known = self.canonical_entity(ai);
                let target = self.canonical_entity(e).unwrap_or(e);
                // The AI's spelling of a customer it did not know.
                if ai_known.is_none() && !same(ai, target) {
                    out.push(ContextSuggestion::AddAlias {
                        entity: target.into(),
                        alias: ai.into(),
                    });
                }
            }
        }
        if let Some(i) = issue_type.map(str::trim).filter(|i| !i.is_empty()) {
            if !self.has_issue_type(i) {
                out.push(ContextSuggestion::AddIssueType { name: i.into() });
            }
        }
        out
    }

    /// The dictionary as the AI sees it.
    pub fn prompt_text(&self) -> String {
        let ents = if self.entities.is_empty() {
            "(아직 없음 — 파일에서 분명한 고객사·프로젝트 이름을 쓴다)".to_string()
        } else {
            self.entities
                .iter()
                .map(|e| {
                    if e.aliases.is_empty() {
                        e.name.clone()
                    } else {
                        format!("{}(별칭: {})", e.name, e.aliases.join(", "))
                    }
                })
                .collect::<Vec<_>>()
                .join(" / ")
        };
        let issues = self
            .issue_types
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let kinds = self
            .doc_kinds
            .iter()
            .map(|k| {
                if k.ext.is_empty() {
                    k.name.clone()
                } else {
                    format!("{}({})", k.name, k.ext.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("고객사·프로젝트: {ents}\n이슈 유형: {issues}\n자료 종류: {kinds}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_and_round_trip() {
        let mut d = ContextDict::starter();
        d.add_entity("바다정보", &["BADA".into(), "바다".into(), "bada".into()])
            .unwrap();
        assert_eq!(d.canonical_entity("bada"), Some("바다정보"));
        assert_eq!(d.canonical_entity(" 바다 "), Some("바다정보"));
        assert_eq!(
            d.entities[0].aliases,
            ["BADA", "바다"],
            "case-insensitive dedupe"
        );
        assert!(d.add_alias("BADA", "바다 정보").unwrap());
        assert!(!d.add_alias("바다정보", "바다").unwrap());
        assert!(d.add_alias("없는 회사", "x").is_err());
        let back = ContextDict::from_toml(&d.to_toml()).unwrap();
        assert_eq!(back, d);
        assert!(d.to_toml().starts_with("# Tidy 업무 맥락 사전"));
        assert_eq!(d.doc_kind_for_ext("log"), Some("로그"));
    }

    #[test]
    fn missing_file_is_the_starter_and_save_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("설정").join("context.toml");
        let d = ContextDict::load(&p).unwrap();
        assert!(d.entities.is_empty() && d.has_issue_type("과금"));
        d.save(&p).unwrap();
        assert_eq!(ContextDict::load(&p).unwrap(), d);
    }

    #[test]
    fn corrections_become_suggestions() {
        let mut d = ContextDict::starter();
        d.add_entity("하늘통신", &[]).unwrap();
        // AI guessed an unknown spelling, user picked a known customer.
        assert_eq!(
            d.suggestions_for(Some("하늘통신"), Some("SKY텔레콤"), Some("과금")),
            vec![ContextSuggestion::AddAlias {
                entity: "하늘통신".into(),
                alias: "SKY텔레콤".into()
            }]
        );
        // New customer + new issue type.
        let s = d.suggestions_for(Some("바다정보"), None, Some("대체발송"));
        assert_eq!(
            s,
            vec![
                ContextSuggestion::AddEntity {
                    name: "바다정보".into()
                },
                ContextSuggestion::AddIssueType {
                    name: "대체발송".into()
                }
            ]
        );
        for x in &s {
            assert!(d.apply(x).unwrap());
        }
        assert!(d.canonical_entity("바다정보").is_some() && d.has_issue_type("대체발송"));
        // AI's guess was another known customer: no alias.
        assert!(d
            .suggestions_for(Some("바다정보"), Some("하늘통신"), None)
            .is_empty());
    }
}
