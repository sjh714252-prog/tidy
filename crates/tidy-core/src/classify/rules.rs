//! Rule engine: `rules.toml` → first matching rule → destination folder.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

use crate::config::expand_home;
use crate::paths::normalize_lexical;

#[derive(Debug, Error)]
pub enum RuleError {
    #[error("failed to read rules file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse rules file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("failed to write rules file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("rule \"{rule}\": invalid name_regex: {source}")]
    Regex {
        rule: String,
        #[source]
        source: regex::Error,
    },
    #[error("rule \"{rule}\": invalid size \"{value}\" (use bytes or e.g. \"10MB\")")]
    Size { rule: String, value: String },
    #[error("rule \"{rule}\": invalid dest template: {msg}")]
    Template { rule: String, msg: String },
}

// ── File format ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct RulesFile {
    #[serde(default, rename = "rule")]
    rules: Vec<RuleDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDef {
    name: String,
    #[serde(rename = "match", default)]
    matcher: MatchDef,
    dest: String,
    #[serde(default)]
    confirm: bool,
    /// Lets the AI auto-move into this rule's folder (M4).
    #[serde(default)]
    ai_auto: bool,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MatchDef {
    ext: Option<Vec<String>>,
    name_regex: Option<String>,
    min_size: Option<SizeDef>,
    max_size: Option<SizeDef>,
    older_than_days: Option<u32>,
    source_app: Option<Vec<String>>,
    from_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SizeDef {
    Bytes(u64),
    Text(String),
}

// ── Compiled rules ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub confirm: bool,
    pub ai_auto: bool,
    ext: Option<Vec<String>>,
    name_regex: Option<Regex>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    older_than: Option<Duration>,
    source_app: Option<Vec<String>>,
    from_dir: Option<PathBuf>,
    dest: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Var {
    Year,
    Month,
    Day,
    Ext,
    Stem,
    SourceDirName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Lit(String),
    Var(Var),
}

#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

/// Facts about a file that rules can match on.
#[derive(Debug, Clone)]
pub struct FileFacts {
    pub path: PathBuf,
    pub name: String,
    pub stem: String,
    /// Lower-case, without the dot. Empty if none.
    pub ext: String,
    pub size: u64,
    pub modified: SystemTime,
}

/// Result of classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub rule_name: String,
    /// Folder the file should go into.
    pub dest_dir: PathBuf,
    pub confirm: bool,
}

impl FileFacts {
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        Ok(Self {
            path: path.to_path_buf(),
            name,
            stem,
            ext,
            size: meta.len(),
            modified: meta.modified().unwrap_or_else(|_| SystemTime::now()),
        })
    }

    fn parent(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new(""))
    }
}

impl RuleSet {
    pub fn load(path: &Path) -> Result<Self, RuleError> {
        let text = std::fs::read_to_string(path).map_err(|source| RuleError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let file: RulesFile = toml::from_str(&text).map_err(|source| RuleError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        Self::compile(file)
    }

    /// Validate `text` and, only if it compiles, write it to `path` as-is
    /// (settings screen "raw TOML" editor). Returns the compiled rules.
    pub fn save_text(path: &Path, text: &str) -> Result<Self, RuleError> {
        let rules = Self::from_toml(text)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| RuleError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, text).map_err(|source| RuleError::Write {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(rules)
    }

    pub fn from_toml(text: &str) -> Result<Self, RuleError> {
        let file: RulesFile = toml::from_str(text).map_err(|source| RuleError::Parse {
            path: PathBuf::from("<inline>"),
            source: Box::new(source),
        })?;
        Self::compile(file)
    }

    fn compile(file: RulesFile) -> Result<Self, RuleError> {
        let mut rules = Vec::new();
        for def in file.rules.into_iter().filter(|r| r.enabled) {
            rules.push(Rule::compile(def)?);
        }
        Ok(Self { rules })
    }

    /// Folders an AI suggestion may move files into without asking
    /// (`ai_auto = true` rules, SPEC §5.2).
    pub fn ai_auto_roots(&self) -> Vec<PathBuf> {
        self.rules
            .iter()
            .filter(|r| r.ai_auto)
            .filter_map(Rule::dest_root)
            .collect()
    }

    /// Fixed destination folders of all rules (AI candidate tree roots).
    pub fn dest_roots(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for p in self.rules.iter().filter_map(Rule::dest_root) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// First matching rule wins (file order = priority).
    pub fn classify(&self, facts: &FileFacts) -> Option<Classification> {
        self.rules
            .iter()
            .find(|r| r.matches(facts))
            .map(|r| Classification {
                rule_name: r.name.clone(),
                dest_dir: r.render_dest(facts),
                confirm: r.confirm,
            })
    }
}

impl Rule {
    fn compile(def: RuleDef) -> Result<Self, RuleError> {
        let name = def.name;
        let m = def.matcher;
        let name_regex = m
            .name_regex
            .map(|re| Regex::new(&re))
            .transpose()
            .map_err(|source| RuleError::Regex {
                rule: name.clone(),
                source,
            })?;
        let size = |s: Option<SizeDef>| -> Result<Option<u64>, RuleError> {
            match s {
                None => Ok(None),
                Some(SizeDef::Bytes(b)) => Ok(Some(b)),
                Some(SizeDef::Text(t)) => parse_size(&t).map(Some).ok_or(RuleError::Size {
                    rule: name.clone(),
                    value: t,
                }),
            }
        };
        let min_size = size(m.min_size)?;
        let max_size = size(m.max_size)?;
        if m.source_app.is_some() {
            tracing::warn!(
                rule = %name,
                "match.source_app is not supported yet (SPEC §13); this rule will never match"
            );
        }
        let dest = parse_template(&def.dest).map_err(|msg| RuleError::Template {
            rule: name.clone(),
            msg,
        })?;
        let from_dir = m.from_dir.map(|d| normalize_dir(&expand_home(&d)));
        Ok(Self {
            name,
            confirm: def.confirm,
            ai_auto: def.ai_auto,
            ext: m.ext.map(|v| {
                v.iter()
                    .map(|e| e.trim_start_matches('.').to_lowercase())
                    .collect()
            }),
            name_regex,
            min_size,
            max_size,
            older_than: m
                .older_than_days
                .map(|d| Duration::from_secs(u64::from(d) * 86_400)),
            source_app: m.source_app,
            from_dir,
            dest,
        })
    }

    pub fn matches(&self, f: &FileFacts) -> bool {
        if self.source_app.is_some() {
            return false;
        }
        if let Some(exts) = &self.ext {
            if !exts.contains(&f.ext) {
                return false;
            }
        }
        if let Some(re) = &self.name_regex {
            if !re.is_match(&f.name) {
                return false;
            }
        }
        if self.min_size.is_some_and(|min| f.size < min) {
            return false;
        }
        if self.max_size.is_some_and(|max| f.size > max) {
            return false;
        }
        if let Some(age) = self.older_than {
            let elapsed = SystemTime::now()
                .duration_since(f.modified)
                .unwrap_or_default();
            if elapsed < age {
                return false;
            }
        }
        if let Some(dir) = &self.from_dir {
            if normalize_dir(f.parent()) != *dir {
                return false;
            }
        }
        true
    }

    /// Render `dest` for a file. Variables are substituted into the template
    /// text first, then the result is turned into a path once; relative
    /// results are resolved against the file's folder and `.`/`..` are
    /// folded away so the logged destination is a clean absolute path.
    /// The fixed part of `dest` before the first variable, as an absolute
    /// folder (`~/Pictures/Screenshots/{yyyy}` → `~/Pictures/Screenshots`).
    /// `None` for relative destinations. Used for the AI candidate tree and
    /// for `ai_auto` (which folders an AI suggestion may move into alone).
    pub fn dest_root(&self) -> Option<PathBuf> {
        let mut lit = String::new();
        let mut cut = false;
        for seg in &self.dest {
            match seg {
                Segment::Lit(s) => lit.push_str(s),
                Segment::Var(_) => {
                    cut = true;
                    break;
                }
            }
        }
        if cut {
            // Drop a partial component ("~/사진/{yyyy}년" → "~/사진").
            let i = lit.rfind(['/', '\\'])?;
            lit.truncate(i);
        }
        let p = normalize_lexical(&expand_home(Path::new(lit.trim())));
        p.is_absolute().then_some(p)
    }

    pub fn render_dest(&self, f: &FileFacts) -> PathBuf {
        let date: DateTime<Local> = f.modified.into();
        let mut out = String::new();
        for seg in &self.dest {
            match seg {
                Segment::Lit(s) => out.push_str(s),
                Segment::Var(Var::Year) => out.push_str(&date.format("%Y").to_string()),
                Segment::Var(Var::Month) => out.push_str(&date.format("%m").to_string()),
                Segment::Var(Var::Day) => out.push_str(&date.format("%d").to_string()),
                Segment::Var(Var::Ext) => out.push_str(&f.ext),
                Segment::Var(Var::Stem) => out.push_str(&f.stem),
                Segment::Var(Var::SourceDirName) => out.push_str(
                    &f.parent()
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            }
        }
        let p = expand_home(Path::new(&out));
        let p = if p.is_relative() {
            f.parent().join(p)
        } else {
            p
        };
        normalize_lexical(&p)
    }
}

fn parse_template(t: &str) -> Result<Vec<Segment>, String> {
    if t.trim().is_empty() {
        return Err("dest is empty".into());
    }
    let mut segs = Vec::new();
    let mut rest = t;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            segs.push(Segment::Lit(rest[..open].to_string()));
        }
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| format!("unclosed '{{' in \"{t}\""))?;
        let var = match &after[..close] {
            "yyyy" => Var::Year,
            "mm" => Var::Month,
            "dd" => Var::Day,
            "ext" => Var::Ext,
            "stem" => Var::Stem,
            "source_dir_name" => Var::SourceDirName,
            other => {
                return Err(format!(
                "unknown variable {{{other}}} (allowed: yyyy, mm, dd, ext, stem, source_dir_name)"
            ))
            }
        };
        segs.push(Segment::Var(var));
        rest = &after[close + 1..];
    }
    if !rest.is_empty() {
        segs.push(Segment::Lit(rest.to_string()));
    }
    Ok(segs)
}

/// Parse "123", "10KB", "1.5 MB", "2g" (binary units) into bytes.
fn parse_size(s: &str) -> Option<u64> {
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

/// Canonical form used for directory comparison (resolves symlinks such as
/// macOS `/var` → `/private/var`; `dunce` avoids `\\?\` prefixes on
/// Windows). Falls back to the input if the folder does not exist.
fn normalize_dir(p: &Path) -> PathBuf {
    dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(path: &Path, size: u64) -> FileFacts {
        let mut f = FileFacts {
            path: path.to_path_buf(),
            name: String::new(),
            stem: String::new(),
            ext: String::new(),
            size,
            modified: SystemTime::now(),
        };
        f.name = path.file_name().unwrap().to_string_lossy().into_owned();
        f.stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        f.ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        f
    }

    const SPEC_RULES: &str = r#"
[[rule]]
name = "스크린샷"
match = { ext = ["png","jpg"], name_regex = "^(스크린샷|Screenshot|화면 캡처)" }
dest = "정리/Screenshots/{yyyy}/{mm}"
confirm = false

[[rule]]
name = "설치파일"
match = { ext = ["exe","msi","dmg","pkg"] }
dest = "정리/Installers"
confirm = true

[[rule]]
name = "큰 파일"
match = { min_size = "1MB" }
dest = "정리/큰 파일/{ext}"
"#;

    #[test]
    fn first_matching_rule_wins() {
        let rs = RuleSet::from_toml(SPEC_RULES).unwrap();
        let shot = facts(Path::new("/d/화면 캡처 2026-10-15 101010.PNG"), 10);
        let c = rs.classify(&shot).unwrap();
        assert_eq!(c.rule_name, "스크린샷");
        assert!(!c.confirm);
        let y = Local::now().format("%Y").to_string();
        let m = Local::now().format("%m").to_string();
        assert_eq!(
            c.dest_dir,
            Path::new("/d")
                .join("정리")
                .join("Screenshots")
                .join(y)
                .join(m)
        );

        let exe = facts(Path::new("/d/setup.EXE"), 5 << 20);
        let c = rs.classify(&exe).unwrap();
        assert_eq!(c.rule_name, "설치파일");
        assert!(c.confirm);

        let big = facts(Path::new("/d/movie.mkv"), 5 << 20);
        assert_eq!(
            rs.classify(&big).unwrap().dest_dir,
            Path::new("/d").join("정리").join("큰 파일").join("mkv")
        );

        assert!(rs.classify(&facts(Path::new("/d/note.txt"), 10)).is_none());
    }

    #[test]
    fn relative_dest_is_under_source_folder() {
        let rs = RuleSet::from_toml(
            "[[rule]]\nname='t'\nmatch={ext=['txt']}\ndest='텍스트 문서/{source_dir_name}'\n",
        )
        .unwrap();
        let f = facts(&Path::new("/home/me/바탕 화면").join("메모.txt"), 1);
        assert_eq!(
            rs.classify(&f).unwrap().dest_dir,
            Path::new("/home/me/바탕 화면")
                .join("텍스트 문서")
                .join("바탕 화면")
        );
    }

    /// PR #1 finding: `dest = "../정리 결과"` used to leave `..` in
    /// `move_log` and the CLI output.
    #[test]
    fn parent_relative_dest_is_normalized() {
        let rs = RuleSet::from_toml(
            "[[rule]]\nname='t'\nmatch={ext=['txt']}\ndest='../정리 결과/{yyyy}'\n",
        )
        .unwrap();
        let f = facts(&Path::new("/home/me/받은 파일").join("메모.txt"), 1);
        let d = rs.classify(&f).unwrap().dest_dir;
        assert!(!d.components().any(|c| c == std::path::Component::ParentDir));
        assert!(d.starts_with(Path::new("/home/me/정리 결과")));
    }

    #[test]
    fn from_dir_and_size_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("다운 로드");
        std::fs::create_dir(&src).unwrap();
        let text = format!(
            "[[rule]]\nname='r'\nmatch={{from_dir='{}', max_size=100}}\ndest='/x'\n",
            src.display().to_string().replace('\\', "/")
        );
        let rs = RuleSet::from_toml(&text).unwrap();
        let file = src.join("a b.bin");
        std::fs::write(&file, b"abc").unwrap();
        let f = FileFacts::from_path(&file).unwrap();
        assert!(rs.classify(&f).is_some());
        let other = dir.path().join("a.bin");
        std::fs::write(&other, b"abc").unwrap();
        assert!(rs
            .classify(&FileFacts::from_path(&other).unwrap())
            .is_none());
        let mut big = f.clone();
        big.size = 101;
        assert!(rs.classify(&big).is_none());
    }

    #[test]
    fn bad_rules_are_reported() {
        let e = RuleSet::from_toml("[[rule]]\nname='x'\ndest='/a/{year}'\n").unwrap_err();
        assert!(e.to_string().contains("unknown variable"));
        let e = RuleSet::from_toml("[[rule]]\nname='x'\nmatch={name_regex='('}\ndest='/a'\n")
            .unwrap_err();
        assert!(matches!(e, RuleError::Regex { .. }));
        let e =
            RuleSet::from_toml("[[rule]]\nname='x'\nmatch={exts=['a']}\ndest='/a'\n").unwrap_err();
        assert!(matches!(e, RuleError::Parse { .. }));
        let e = RuleSet::from_toml("[[rule]]\nname='x'\nmatch={min_size='lots'}\ndest='/a'\n")
            .unwrap_err();
        assert!(matches!(e, RuleError::Size { .. }));
    }

    #[test]
    fn disabled_and_source_app_rules_never_match() {
        let rs = RuleSet::from_toml(
            "[[rule]]\nname='off'\nenabled=false\ndest='/a'\n\
             [[rule]]\nname='app'\nmatch={source_app=['chrome']}\ndest='/b'\n",
        )
        .unwrap();
        assert_eq!(rs.rules().len(), 1);
        assert!(rs.classify(&facts(Path::new("/d/x.txt"), 1)).is_none());
    }

    #[test]
    fn save_text_rejects_invalid_and_keeps_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("규칙 폴더").join("rules.toml");
        let good = "# 주석 유지\n[[rule]]\nname='a'\nmatch={ext=['txt']}\ndest='/x'\n";
        assert_eq!(RuleSet::save_text(&p, good).unwrap().rules().len(), 1);
        assert!(RuleSet::save_text(&p, "[[rule]]\nname='b'\ndest='/a/{nope}'\n").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), good);
    }

    #[test]
    fn dest_roots_stop_at_first_variable() {
        let tmp = tempfile::tempdir().unwrap();
        let b = tmp.path().display().to_string().replace('\\', "/");
        let rs = RuleSet::from_toml(&format!(
            "[[rule]]\nname='a'\ndest='{b}/정리/사진/{{yyyy}}/{{mm}}'\nai_auto=true\n\
             [[rule]]\nname='b'\ndest='{b}/정리/문서/{{yyyy}}년'\n\
             [[rule]]\nname='c'\ndest='상대/{{ext}}'\n\
             [[rule]]\nname='d'\ndest='~/받은 파일'\n"
        ))
        .unwrap();
        let roots: Vec<PathBuf> = rs
            .rules()
            .iter()
            .map(|r| r.dest_root().unwrap_or_default())
            .collect();
        let base = normalize_lexical(tmp.path());
        assert_eq!(roots[0], base.join("정리").join("사진"));
        assert_eq!(roots[1], base.join("정리").join("문서"));
        assert_eq!(roots[2], PathBuf::new());
        assert_eq!(roots[3], dirs::home_dir().unwrap().join("받은 파일"));
        assert_eq!(rs.ai_auto_roots(), vec![base.join("정리").join("사진")]);
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("123"), Some(123));
        assert_eq!(parse_size("10KB"), Some(10 << 10));
        assert_eq!(parse_size("1.5 MB"), Some(3 << 19));
        assert_eq!(parse_size("2g"), Some(2 << 30));
        assert_eq!(parse_size("2GiB"), Some(2 << 30));
        assert_eq!(parse_size("x"), None);
    }

    /// Windows paths with a drive letter, Korean and spaces, and UNC shares.
    #[cfg(windows)]
    #[test]
    fn windows_templates() {
        let rs = RuleSet::from_toml(
            r#"
[[rule]]
name = "문서"
match = { ext = ["hwp"] }
dest = 'C:\Users\홍 길동\Documents\정리 된 문서\{yyyy}'

[[rule]]
name = "NAS"
match = { ext = ["zip"] }
dest = '\\nas\공유 폴더\압축 파일'
"#,
        )
        .unwrap();
        let f = facts(Path::new(r"C:\Users\홍 길동\바탕 화면\보고서 v2.hwp"), 1);
        let d = rs.classify(&f).unwrap().dest_dir;
        let y = Local::now().format("%Y").to_string();
        assert_eq!(
            d,
            PathBuf::from(r"C:\Users\홍 길동\Documents\정리 된 문서").join(y)
        );
        assert!(d.is_absolute());

        let z = facts(Path::new(r"C:\Users\홍 길동\바탕 화면\사진 모음.zip"), 1);
        let d = rs.classify(&z).unwrap().dest_dir;
        assert!(d.is_absolute());
        assert!(matches!(
            d.components().next(),
            Some(std::path::Component::Prefix(p))
                if matches!(p.kind(), std::path::Prefix::UNC(..))
        ));
        assert_eq!(
            d.join("사진 모음.zip"),
            PathBuf::from(r"\\nas\공유 폴더\압축 파일\사진 모음.zip")
        );
    }
}
