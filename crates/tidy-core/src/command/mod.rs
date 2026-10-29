//! Natural-language commands (SPEC §14.13): "지난달 받은 KT 관련 엑셀 전부
//! 프로젝트 폴더로" → what (keywords, kinds, dates, source folder, size)
//! and where (a destination folder) → the files from the search index →
//! a saved plan ([`PlanSource::Command`]) the user approves before anything
//! moves (CLAUDE.md rule 6). Applying is [`restructure::apply`]: one batch,
//! undoable at once.
//!
//! The AI interprets the sentence when it is available; the offline
//! parser ([`parse_rules`]) always works and is the fallback (rule 4).
//! The AI only sees the sentence, today's date and folder names — never
//! file contents or file lists.

mod parse;

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

pub use parse::parse_rules;

use crate::ai::AiError;
use crate::classify::{AiCall, AiClassifier};
use crate::config::{expand_home, Config};
use crate::paths::normalize_lexical;
use crate::report::restructure::{self, FolderNote, Plan, PlanError, PlanSource, PlannedMove};
use crate::search::{search, MatchedIn, Query, SearchOptions};
use crate::store::{Store, StoreError};

/// Most files one command finds (more → "처음 500개만").
pub const MAX_FILES: usize = 500;
/// How deep destination / source folder names are looked up.
const FOLDER_DEPTH: usize = 3;
/// Folder entries visited per lookup at most (big NAS shares).
const FOLDER_SCAN_CAP: usize = 50_000;
const AI_MAX_TOKENS: u32 = 1024;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("명령이 비어 있습니다")]
    Empty,
    #[error("{0}")]
    Unsupported(String),
    #[error("어떤 파일인지 알 수 없습니다 — 이름의 단어·종류(PDF, 엑셀…)·기간(지난달…)·폴더 중 하나는 알려 주세요")]
    NoTarget,
    #[error("어디로 옮길지 알 수 없습니다 — \"… 폴더로 옮겨줘\"처럼 옮길 곳을 알려 주세요")]
    NoDestination,
    #[error("옮길 곳을 만들 폴더가 없습니다 — 감시 폴더를 먼저 정하거나 전체 경로를 적어 주세요")]
    NoPlace,
    #[error("옮길 곳이 잘못되었습니다: {0}")]
    BadDestination(String),
    /// Only inside [`run`]: answered by falling back to the offline parser.
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Plan(#[from] PlanError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CommandAction {
    Move,
    #[default]
    Find,
}

/// What the sentence asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Intent {
    pub action: CommandAction,
    /// Name / body words, all must match.
    pub keywords: Vec<String>,
    /// Lower-case extensions without dots; empty = any.
    pub exts: Vec<String>,
    /// Modified on or after this day.
    pub after: Option<NaiveDate>,
    /// Modified before this day (exclusive).
    pub before: Option<NaiveDate>,
    /// Where the files are: a watch folder alias, folder name or path.
    pub source: Option<String>,
    /// Where they go: a folder name or path.
    pub destination: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
}

impl Intent {
    fn has_target(&self) -> bool {
        !self.keywords.is_empty()
            || !self.exts.is_empty()
            || self.after.is_some()
            || self.before.is_some()
            || self.source.is_some()
            || self.min_size.is_some()
            || self.max_size.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParsedBy {
    Ai,
    Rules,
}

#[derive(Debug, Clone, Serialize)]
pub struct FoundFile {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
    /// name | content | both | filter
    pub matched_in: MatchedIn,
}

#[derive(Debug, Clone, Serialize)]
pub struct Destination {
    pub path: PathBuf,
    /// The folder does not exist yet (made when the plan is applied).
    pub new_folder: bool,
    /// Other folders with a matching name (first one was chosen).
    pub alternatives: Vec<PathBuf>,
}

/// What a command found / proposes. Nothing has moved.
#[derive(Debug, Clone, Serialize)]
pub struct CommandResult {
    pub text: String,
    pub intent: Intent,
    pub parsed_by: ParsedBy,
    /// Why the AI was not used / what it said, if worth showing.
    pub note: Option<String>,
    /// Human pieces: "기간 2026-09-01 ~ 2026-09-30", "종류 PDF"…
    pub understood: Vec<String>,
    pub files: Vec<FoundFile>,
    /// More files matched than [`MAX_FILES`].
    pub truncated: bool,
    pub destination: Option<Destination>,
    /// Saved move plan (move commands with at least one file).
    pub plan: Option<Plan>,
    pub warnings: Vec<String>,
}

pub struct CommandOptions {
    pub today: NaiveDate,
    /// Interpret without the AI even when it is available.
    pub rules_only: bool,
    pub ai_timeout: Duration,
    /// `[report] protected` plus extras (canonical paths).
    pub protected: Vec<PathBuf>,
}

impl CommandOptions {
    pub fn new(protected: Vec<PathBuf>) -> Self {
        Self {
            today: Local::now().date_naive(),
            rules_only: false,
            ai_timeout: Duration::from_secs(20),
            protected,
        }
    }
}

/// Interpret `text`, find the files and (for a move) save a plan.
pub fn run(
    text: &str,
    cfg: &Config,
    store: &Store,
    ai: Option<&AiClassifier>,
    opts: &CommandOptions,
) -> Result<CommandResult, CommandError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CommandError::Empty);
    }
    let places = Places::from_config(cfg);
    let (mut intent, parsed_by, note) = match ai.filter(|_| !opts.rules_only) {
        Some(ai) => match parse_ai(text, ai, store, &places, opts) {
            Ok(it) => (it, ParsedBy::Ai, None),
            Err(CommandError::Unsupported(why)) => return Err(CommandError::Unsupported(why)),
            Err(e) => {
                let it = parse_rules(text, opts.today)?;
                (
                    it,
                    ParsedBy::Rules,
                    Some(format!("AI 해석 실패 — AI 없이 해석함 ({e})")),
                )
            }
        },
        None => (parse_rules(text, opts.today)?, ParsedBy::Rules, None),
    };
    let mut warnings = Vec::new();

    // Where the files are.
    let mut sources = Vec::new();
    if let Some(src) = intent.source.clone() {
        match places.find_source(&src) {
            Some(p) => sources.push(p),
            None => {
                warnings.push(format!(
                    "'{src}' 폴더를 찾지 못해 이름·본문 단어로 찾았습니다"
                ));
                intent.source = None;
                if !intent.keywords.iter().any(|k| k == &src) {
                    intent.keywords.push(src);
                }
            }
        }
    }
    if !intent.has_target() {
        return Err(CommandError::NoTarget);
    }
    let destination = match (intent.action, &intent.destination) {
        (CommandAction::Move, None) => return Err(CommandError::NoDestination),
        (CommandAction::Move, Some(d)) => Some(places.find_destination(d)?),
        (CommandAction::Find, _) => None,
    };
    if let Some(d) = &destination {
        if d.new_folder {
            warnings.push(format!("새 폴더를 만듭니다: {}", d.path.display()));
        }
        if !d.alternatives.is_empty() {
            warnings.push(format!(
                "같은 이름 폴더가 {}개 더 있어 첫 번째로 골랐습니다 (다른 곳이면 전체 경로를 적어 주세요)",
                d.alternatives.len()
            ));
        }
    }

    // The files (from the search index).
    let q = Query {
        terms: intent.keywords.clone(),
        exts: intent.exts.clone(),
        in_dir: None,
        in_any: sources.clone(),
        after: intent.after.and_then(local_ts),
        before: intent.before.and_then(local_ts),
        min_size: intent.min_size,
        max_size: intent.max_size,
    };
    let hits = search(
        store,
        &q,
        &SearchOptions {
            limit: MAX_FILES * 4,
            ..Default::default()
        },
    )?;
    let filter = cfg.ignore_filter().ok();
    let mut files = Vec::new();
    let mut truncated = false;
    let mut inside_dest = 0usize;
    for h in hits {
        let p = &h.file.path;
        if !p.is_file() {
            continue;
        }
        if opts.protected.iter().any(|d| p.starts_with(d)) {
            continue;
        }
        if filter
            .as_ref()
            .is_some_and(|f| f.in_excluded_dir(p) || f.is_ignored(p))
        {
            continue;
        }
        if let Some(d) = &destination {
            if p.parent() == Some(d.path.as_path()) {
                inside_dest += 1;
                continue;
            }
        }
        if files.len() == MAX_FILES {
            truncated = true;
            break;
        }
        files.push(FoundFile {
            path: p.clone(),
            size: h.file.size,
            mtime: h.file.mtime,
            matched_in: h.matched_in,
        });
    }
    if truncated {
        warnings.push(format!(
            "{MAX_FILES}개가 넘어 처음 {MAX_FILES}개만 보여 줍니다 — 조건을 더 좁혀 주세요"
        ));
    }
    if inside_dest > 0 {
        warnings.push(format!("{inside_dest}개는 이미 옮길 곳에 있어 뺐습니다"));
    }
    if files.is_empty() {
        let st = store.index_stats()?;
        warnings.push(if st.files == 0 {
            "검색 색인이 비어 있습니다 — 감시 폴더 색인이 끝난 뒤 다시 해 보세요".into()
        } else {
            "조건에 맞는 파일이 없습니다".into()
        });
    }

    let plan = match &destination {
        Some(d) if !files.is_empty() => {
            let moves = files
                .iter()
                .enumerate()
                .map(|(i, f)| PlannedMove {
                    id: i as u32 + 1,
                    from: f.path.clone(),
                    to: d.path.join(f.path.file_name().unwrap_or_default()),
                    is_dir: false,
                    files: 1,
                    bytes: f.size,
                    reason: format!("명령: {text}"),
                    warnings: Vec::new(),
                    context: None,
                    encrypt: false,
                })
                .collect();
            let plan = Plan {
                id: 0,
                root: d.path.clone(),
                source: PlanSource::Command,
                created_at: chrono::Utc::now().timestamp(),
                summary: text.to_string(),
                folders: if d.new_folder {
                    vec![FolderNote {
                        path: d.path.display().to_string(),
                        reason: "새 폴더".into(),
                    }]
                } else {
                    Vec::new()
                },
                moves,
                rejected: Vec::new(),
                applied_batch: None,
                anchor: None,
            };
            Some(restructure::save(store, plan)?)
        }
        _ => None,
    };

    Ok(CommandResult {
        text: text.to_string(),
        understood: understood(&intent, &sources, destination.as_ref()),
        intent,
        parsed_by,
        note,
        files,
        truncated,
        destination,
        plan,
        warnings,
    })
}

fn local_ts(d: NaiveDate) -> Option<i64> {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
        .earliest()
        .map(|t| t.timestamp())
}

/// "기간 2026-09-01 ~ 2026-09-30" style pieces for the UI / CLI.
fn understood(it: &Intent, sources: &[PathBuf], dest: Option<&Destination>) -> Vec<String> {
    let mut out = Vec::new();
    out.push(
        match it.action {
            CommandAction::Move => "옮기기",
            CommandAction::Find => "찾기",
        }
        .to_string(),
    );
    match (it.after, it.before) {
        (Some(a), Some(b)) => {
            let last = b.pred_opt().unwrap_or(b);
            if a == last {
                out.push(format!("날짜 {a}"));
            } else {
                out.push(format!("기간 {a} ~ {last}"));
            }
        }
        (Some(a), None) => out.push(format!("기간 {a}부터")),
        (None, Some(b)) => out.push(format!("기간 {b} 전까지")),
        (None, None) => {}
    }
    if !it.exts.is_empty() {
        out.push(format!("종류 {}", kind_label(&it.exts)));
    }
    if !it.keywords.is_empty() {
        out.push(format!("단어 {}", it.keywords.join(" · ")));
    }
    for s in sources {
        out.push(format!("있는 곳 {}", s.display()));
    }
    if let Some(n) = it.min_size {
        out.push(format!("크기 {} 이상", human_size(n)));
    }
    if let Some(n) = it.max_size {
        out.push(format!("크기 {} 이하", human_size(n)));
    }
    if let Some(d) = dest {
        out.push(format!(
            "옮길 곳 {}{}",
            d.path.display(),
            if d.new_folder { " (새 폴더)" } else { "" }
        ));
    }
    out
}

fn kind_label(exts: &[String]) -> String {
    let has = |e: &str| exts.iter().any(|x| x == e);
    let named = [
        ("pdf", "PDF"),
        ("xlsx", "엑셀"),
        ("docx", "워드"),
        ("hwp", "한글"),
        ("pptx", "PPT"),
        ("jpg", "사진"),
        ("mp4", "영상"),
        ("mp3", "음악"),
        ("zip", "압축"),
    ];
    let words: Vec<&str> = named
        .iter()
        .filter(|(e, _)| has(e))
        .map(|(_, w)| *w)
        .collect();
    if exts.len() > 8 && has("pdf") && has("docx") {
        return "문서".into();
    }
    if words.is_empty() || exts.len() <= 2 && words.len() < exts.len() {
        return exts.join(", ");
    }
    words.join(", ")
}

fn human_size(n: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if n >= 1024 * MB {
        format!("{:.1}GB", n as f64 / (1024 * MB) as f64)
    } else if n >= MB {
        format!("{}MB", n / MB)
    } else {
        format!("{}KB", n / 1024)
    }
}

// ── folders ─────────────────────────────────────────────────────────────

/// Folders a command can name: organize roots first, then watch folders,
/// then index-only folders. `aliases` = (name, path) of watch folders.
struct Places {
    bases: Vec<PathBuf>,
    aliases: Vec<(String, PathBuf)>,
    excluded: Vec<PathBuf>,
}

impl Places {
    fn from_config(cfg: &Config) -> Self {
        let mut bases: Vec<PathBuf> = Vec::new();
        let mut push = |p: PathBuf| {
            let p = normalize_lexical(&expand_home(&p));
            if !p.as_os_str().is_empty() && !bases.contains(&p) {
                bases.push(p);
            }
        };
        for w in &cfg.watches {
            if w.anchor() != w.path.as_path() {
                push(w.anchor().to_path_buf());
            }
        }
        for w in &cfg.watches {
            push(w.path.clone());
        }
        for r in &cfg.index_only {
            push(r.path.clone());
        }
        let mut aliases = Vec::new();
        for w in &cfg.watches {
            let path = normalize_lexical(&expand_home(&w.path));
            if let Some(a) = &w.alias {
                aliases.push((a.clone(), path.clone()));
            }
            if let Some(n) = path.file_name() {
                aliases.push((n.to_string_lossy().into_owned(), path.clone()));
            }
        }
        for (names, dir) in [
            (
                &["바탕화면", "바탕 화면", "desktop"][..],
                dirs::desktop_dir(),
            ),
            (
                &["다운로드", "downloads", "download"][..],
                dirs::download_dir(),
            ),
            (&["문서", "내 문서", "documents"][..], dirs::document_dir()),
            (&["사진", "pictures"][..], dirs::picture_dir()),
        ] {
            if let Some(d) = dir {
                for n in names {
                    aliases.push((n.to_string(), d.clone()));
                }
            }
        }
        Self {
            bases,
            aliases,
            excluded: cfg
                .watches
                .iter()
                .flat_map(|w| w.exclude.iter().cloned())
                .collect(),
        }
    }

    fn find_source(&self, name: &str) -> Option<PathBuf> {
        let p = expand_home(Path::new(name.trim()));
        if p.is_absolute() {
            return p.is_dir().then(|| normalize_lexical(&p));
        }
        let key = norm(name);
        // Watch folders first (their alias or name), then well-known OS
        // folders, then any folder of that exact name.
        if let Some((_, p)) = self
            .aliases
            .iter()
            .find(|(a, p)| norm(a) == key && p.is_dir())
        {
            return Some(p.clone());
        }
        self.matches(name)
            .into_iter()
            .find(|(exact, _, _)| *exact)
            .map(|(_, _, p)| p)
    }

    fn find_destination(&self, name: &str) -> Result<Destination, CommandError> {
        let name = name.trim().trim_end_matches(['/', '\\']);
        if name.is_empty() {
            return Err(CommandError::NoDestination);
        }
        let p = expand_home(Path::new(name));
        if p.is_absolute() {
            let p = normalize_lexical(&p);
            return Ok(Destination {
                new_folder: !p.is_dir(),
                path: p,
                alternatives: Vec::new(),
            });
        }
        let rel = Path::new(name);
        if rel.components().any(|c| {
            !matches!(c, std::path::Component::Normal(_))
                || c.as_os_str().to_string_lossy().contains(':')
        }) {
            return Err(CommandError::BadDestination(name.to_string()));
        }
        if rel.components().count() > 1 {
            // "고객사/KT": an existing one under any base, else new under the first.
            let first = self.bases.first().ok_or(CommandError::NoPlace)?;
            let found = self.bases.iter().map(|b| b.join(rel)).find(|p| p.is_dir());
            return Ok(Destination {
                new_folder: found.is_none(),
                path: found.unwrap_or_else(|| first.join(rel)),
                alternatives: Vec::new(),
            });
        }
        // A watch folder named directly ("다운로드로").
        let key = norm(name);
        let mut found = self.matches(name);
        if let Some((_, p)) = self
            .aliases
            .iter()
            .find(|(a, p)| norm(a) == key && p.is_dir())
        {
            found.retain(|(_, _, q)| q != p);
            found.insert(0, (true, 0, p.clone()));
        }
        if let Some((_, _, best)) = found.first().cloned() {
            let alternatives = found
                .iter()
                .skip(1)
                .filter(|(exact, _, _)| *exact)
                .map(|(_, _, p)| p.clone())
                .take(5)
                .collect();
            return Ok(Destination {
                path: best,
                new_folder: false,
                alternatives,
            });
        }
        let first = self.bases.first().ok_or(CommandError::NoPlace)?;
        Ok(Destination {
            path: first.join(name),
            new_folder: true,
            alternatives: Vec::new(),
        })
    }

    /// Folders under the bases whose name equals / contains `name`, best
    /// first: (exact, depth, path).
    fn matches(&self, name: &str) -> Vec<(bool, usize, PathBuf)> {
        let key = norm(name);
        if key.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(bool, usize, PathBuf)> = Vec::new();
        let mut seen = 0usize;
        for base in &self.bases {
            if let Some(n) = base.file_name() {
                let n = norm(&n.to_string_lossy());
                if n == key {
                    out.push((true, 0, base.clone()));
                }
            }
            let walker = walkdir::WalkDir::new(base)
                .min_depth(1)
                .max_depth(FOLDER_DEPTH)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|e| {
                    e.file_type().is_dir()
                        && !e.file_name().to_string_lossy().starts_with('.')
                        && !self.excluded.iter().any(|x| e.path().starts_with(x))
                });
            for e in walker.filter_map(Result::ok) {
                seen += 1;
                if seen > FOLDER_SCAN_CAP {
                    break;
                }
                let n = norm(&e.file_name().to_string_lossy());
                let exact = n == key;
                if exact || n.contains(&key) {
                    let p = normalize_lexical(e.path());
                    if !out.iter().any(|(_, _, q)| q == &p) {
                        out.push((exact, e.depth(), p));
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.as_os_str().len().cmp(&b.2.as_os_str().len()))
        });
        out
    }

    /// Top-level folder names under the organize roots (AI hint).
    fn hint_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for b in self.bases.iter().take(4) {
            if let Ok(rd) = std::fs::read_dir(b) {
                let mut names: Vec<String> = rd
                    .filter_map(Result::ok)
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.'))
                    .collect();
                names.sort();
                for n in names {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
        out.truncate(60);
        out
    }
}

/// Case- and space-insensitive folder name key.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

// ── AI interpretation ───────────────────────────────────────────────────

const AI_SYSTEM: &str = "\
너는 파일 정리 프로그램의 명령 해석기다. 사용자의 한 문장 명령을 JSON으로 바꾼다.
- action: 파일을 어딘가로 옮기라는 뜻이면 \"move\", 찾거나 보여 달라는 뜻이면 \"find\". \
  지우기·휴지통·이름 바꾸기·복사·압축 등 그 밖의 일이면 \"unsupported\"이고 explanation에 이유를 쓴다.
- keywords: 파일 이름이나 본문에 들어 있어야 할 단어(회사명·문서 이름 등). 날짜·종류·폴더·동사·\"관련\"·\"전부\" 같은 말은 넣지 않는다. 조사는 뗀다.
- exts: 파일 종류를 확장자로(소문자, 점 없이). 엑셀 = xlsx,xls,xlsm,csv / 워드 = docx,doc / 한글 = hwp,hwpx / PPT = pptx,ppt / 사진 = jpg,jpeg,png,gif,bmp,webp,heic / PDF = pdf. 종류 말이 없으면 빈 배열.
- after / before: 파일 수정일 범위(YYYY-MM-DD). after는 그날 포함, before는 그날 미포함. 기간 말이 없으면 null. \
  \"지난달\"은 오늘 기준 지난 달 1일 ~ 이번 달 1일.
- source: 파일이 지금 있는 폴더(\"바탕화면에서\", \"다운로드에 있는\")의 이름. 없으면 null.
- destination: 옮길 폴더 이름 또는 경로(사용자가 쓴 그대로, \"폴더\"라는 말은 뺀다). 없으면 null.
- min_size_mb / max_size_mb: 크기 조건(MB). 없으면 null.
- explanation: 해석을 한 문장으로.
감시 폴더·정리 폴더 이름 목록이 주어지면 사용자가 그중 하나를 가리킬 때 그 이름을 그대로 쓴다.";

fn ai_schema() -> Value {
    let s_or_null = json!({ "anyOf": [ { "type": "string" }, { "type": "null" } ] });
    let n_or_null = json!({ "anyOf": [ { "type": "number" }, { "type": "null" } ] });
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["move", "find", "unsupported"] },
            "keywords": { "type": "array", "items": { "type": "string" } },
            "exts": { "type": "array", "items": { "type": "string" } },
            "after": s_or_null,
            "before": s_or_null,
            "source": s_or_null,
            "destination": s_or_null,
            "min_size_mb": n_or_null,
            "max_size_mb": n_or_null,
            "explanation": { "type": "string" }
        },
        "required": ["action", "keywords", "exts", "after", "before", "source",
                     "destination", "min_size_mb", "max_size_mb", "explanation"],
        "additionalProperties": false
    })
}

fn ai_user(text: &str, places: &Places, today: NaiveDate) -> String {
    let mut u = format!("오늘: {today} ({})\n명령: {text}\n", weekday_ko(today));
    let watch: Vec<&str> = places.aliases.iter().map(|(a, _)| a.as_str()).collect();
    if !watch.is_empty() {
        let mut w: Vec<&str> = Vec::new();
        for a in watch {
            if !w.contains(&a) {
                w.push(a);
            }
        }
        u.push_str(&format!("감시 폴더: {}\n", w.join(", ")));
    }
    let hint = places.hint_names();
    if !hint.is_empty() {
        u.push_str(&format!("정리 폴더: {}\n", hint.join(", ")));
    }
    u
}

fn weekday_ko(d: NaiveDate) -> &'static str {
    use chrono::Datelike;
    ["월", "화", "수", "목", "금", "토", "일"][d.weekday().num_days_from_monday() as usize]
}

fn parse_ai(
    text: &str,
    ai: &AiClassifier,
    store: &Store,
    places: &Places,
    opts: &CommandOptions,
) -> Result<Intent, CommandError> {
    let user = ai_user(text, places, opts.today);
    let schema = ai_schema();
    let res = ai.call(
        store,
        &AiCall {
            purpose: "command",
            system: AI_SYSTEM,
            user: &user,
            schema: &schema,
            max_tokens: AI_MAX_TOKENS,
            timeout: opts.ai_timeout,
        },
        |v| {
            let it = intent_from_ai(&v)?;
            Ok((
                it,
                Some(format!(
                    "{text} → {}",
                    v["explanation"].as_str().unwrap_or("")
                )),
            ))
        },
    );
    match res {
        Ok(Ok(it)) => Ok(it),
        Ok(Err(why)) => Err(CommandError::Unsupported(why)),
        Err(e) => Err(CommandError::Ai(e)),
    }
}

/// `Ok(Err(why))` = the AI said the command is unsupported.
fn intent_from_ai(v: &Value) -> Result<Result<Intent, String>, AiError> {
    let s = |k: &str| {
        v[k].as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "null")
            .map(str::to_string)
    };
    let list = |k: &str| -> Vec<String> {
        v[k].as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let date = |k: &str| -> Result<Option<NaiveDate>, AiError> {
        match s(k) {
            None => Ok(None),
            Some(d) => NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                .map(Some)
                .map_err(|_| AiError::BadResponse(format!("{k} is not YYYY-MM-DD: {d}"))),
        }
    };
    let mb = |k: &str| {
        v[k].as_f64()
            .filter(|n| *n > 0.0)
            .map(|n| (n * 1024.0 * 1024.0) as u64)
    };
    let action = match v["action"].as_str() {
        Some("move") => CommandAction::Move,
        Some("find") => CommandAction::Find,
        Some("unsupported") => {
            let why = s("explanation").unwrap_or_else(|| "지원하지 않는 명령입니다".into());
            return Ok(Err(format!("{why} — 옮기기와 찾기만 할 수 있습니다")));
        }
        other => return Err(AiError::BadResponse(format!("unknown action {other:?}"))),
    };
    let mut keywords = Vec::new();
    for k in list("keywords") {
        let k = parse::strip_particles(&k);
        if !keywords.contains(&k) {
            keywords.push(k);
        }
    }
    let mut exts = Vec::new();
    for e in list("exts") {
        let e = e.trim_start_matches('.').to_lowercase();
        if !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()) && !exts.contains(&e) {
            exts.push(e);
        }
    }
    let destination = s("destination").map(|d| {
        d.trim_end_matches("폴더")
            .trim()
            .trim_matches(|c: char| "\"'“”‘’「」".contains(c))
            .to_string()
    });
    let mut it = Intent {
        action,
        keywords,
        exts,
        after: date("after")?,
        before: date("before")?,
        source: s("source"),
        destination: destination.filter(|d| !d.is_empty()),
        min_size: mb("min_size_mb"),
        max_size: mb("max_size_mb"),
    };
    if it.destination.is_some() && it.action == CommandAction::Find {
        it.destination = None;
    }
    Ok(Ok(it))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_answer_to_intent() {
        let v = json!({
            "action": "move", "keywords": ["KT를"], "exts": [".XLSX", "xls"],
            "after": "2026-09-01", "before": "2026-10-01", "source": null,
            "destination": "프로젝트 폴더", "min_size_mb": null, "max_size_mb": 1.0,
            "explanation": "지난달 KT 엑셀을 프로젝트로"
        });
        let it = intent_from_ai(&v).unwrap().unwrap();
        assert_eq!(it.keywords, vec!["KT"]);
        assert_eq!(it.exts, vec!["xlsx", "xls"]);
        assert_eq!(it.after, NaiveDate::from_ymd_opt(2026, 9, 1));
        assert_eq!(it.destination.as_deref(), Some("프로젝트"));
        assert_eq!(it.max_size, Some(1024 * 1024));
        let bad = json!({ "action": "move", "after": "지난달" });
        assert!(intent_from_ai(&bad).is_err());
        let no = json!({ "action": "unsupported", "explanation": "삭제 요청" });
        assert!(intent_from_ai(&no).unwrap().is_err());
    }

    #[test]
    fn labels() {
        let it = Intent {
            action: CommandAction::Move,
            exts: vec!["xlsx".into(), "xls".into(), "xlsm".into(), "csv".into()],
            after: NaiveDate::from_ymd_opt(2026, 9, 1),
            before: NaiveDate::from_ymd_opt(2026, 10, 1),
            keywords: vec!["KT".into()],
            ..Default::default()
        };
        let u = understood(&it, &[], None);
        assert_eq!(
            u,
            vec![
                "옮기기",
                "기간 2026-09-01 ~ 2026-09-30",
                "종류 엑셀",
                "단어 KT"
            ]
        );
        assert_eq!(kind_label(&["log".into()]), "log");
        assert_eq!(kind_label(&["pdf".into()]), "PDF");
    }
}
