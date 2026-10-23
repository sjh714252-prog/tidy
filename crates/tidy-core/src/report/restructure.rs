//! Restructure plans (SPEC §5.7): a list of proposed moves for one folder,
//! from the rules (works without AI) or from the AI (SPEC §8.2), validated
//! against the real tree, shown as a before/after diff, and applied only
//! when the user approves — selected moves only, all in one `move_log`
//! batch so `tidy undo --batch` (or the history screen) reverts it at once.
//!
//! Never applied automatically (CLAUDE.md rule 6). Nothing is deleted:
//! folders emptied by a plan stay where they are.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::scan::{scan, DirNode, ScanError, ScanOptions};
use super::stats::{join, walk};
use super::tree::{render_tree, TreeOptions};
use crate::ai::{prompts, AiError};
use crate::classify::{AiCall, AiClassifier, FileFacts, RuleSet};
use crate::engine::{new_batch_id, reindex_for, Action};
use crate::index::IndexOptions;
use crate::mover::{DirMoveRequest, MoveOutcome, MoveRequest, Mover, TMP_MARKER};
use crate::paths::normalize_lexical;
use crate::store::{DecidedBy, Store, StoreError};
use crate::watcher::IgnoreFilter;

#[derive(Debug, Error)]
pub enum PlanError {
    #[error(transparent)]
    Scan(#[from] ScanError),
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("no restructure plan with id {0}")]
    NotFound(i64),
    #[error("plan #{id} was already applied (batch {batch}); undo that batch instead")]
    AlreadyApplied { id: i64, batch: String },
    #[error("stored plan is unreadable: {0}")]
    Corrupt(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanSource {
    /// What `rules.toml` would do with the files already there.
    Rules,
    /// An AI proposal (SPEC §8.2).
    Ai,
    /// (M10) Work-context curation: customer / dated issue / kind (§14.3).
    Context,
}

impl PlanSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanSource::Rules => "rules",
            PlanSource::Ai => "ai",
            PlanSource::Context => "context",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedMove {
    /// 1-based, stable within the plan (used for selection).
    pub id: u32,
    /// Absolute source (file or folder).
    pub from: PathBuf,
    /// Absolute new path (file path including name, or new folder path).
    pub to: PathBuf,
    pub is_dir: bool,
    pub files: u64,
    pub bytes: u64,
    /// Rule name or the AI's reason.
    pub reason: String,
    /// Things the user should know before approving.
    pub warnings: Vec<String>,
    /// (M10) What the AI judged for this file (work-context plans).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextNote>,
    /// (M10) Lock it into an encrypted zip at the destination (personal
    /// information with the vault mode on, SPEC §14.2).
    #[serde(default)]
    pub encrypt: bool,
}

/// (M10) The work-context judgement behind a planned move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ContextNote {
    pub entity: Option<String>,
    pub issue: String,
    /// Folder date after merging (YYYY-MM-DD).
    pub date: String,
    /// The file's own date (AI's date, else modified), before merging.
    pub own_date: String,
    pub doc_kind: String,
    pub topic: Option<String>,
    pub confidence: f64,
    pub reason: String,
    /// What the AI said, kept when the user changes it (dictionary learning).
    pub ai_entity: Option<String>,
    pub ai_issue: String,
    /// Below `min_confidence`: left in place unless the user assigns it.
    pub unclassified: bool,
    /// Changed by the user in the preview.
    pub edited: bool,
    /// `curation.key` of the file.
    pub key: String,
    /// (M12) A folder item: its inner tree (a few levels) for the preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FolderNote {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rejected {
    pub from: String,
    pub to: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// 0 until saved with [`save`].
    pub id: i64,
    pub root: PathBuf,
    pub source: PlanSource,
    pub created_at: i64,
    pub summary: String,
    pub folders: Vec<FolderNote>,
    pub moves: Vec<PlannedMove>,
    /// AI moves that failed validation (shown, never applied).
    pub rejected: Vec<Rejected>,
    pub applied_batch: Option<String>,
    /// (M10) Anchor relative layouts were resolved against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    /// Folder tree before / after the selected moves (text, depth 3).
    pub before: String,
    pub after: String,
    /// Selected moves that leave the root: (from, to).
    pub outside: Vec<(PathBuf, PathBuf)>,
    pub files_moved: u64,
    /// Folders (relative) that end up with no files. They are left in
    /// place — nothing is deleted.
    pub emptied: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyReport {
    pub batch_id: String,
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// (path, why) — gone, identical copy already there, protected…
    pub skipped: Vec<(PathBuf, String)>,
    pub errors: Vec<(PathBuf, String)>,
}

/// One file of an expanded move.
#[derive(Debug, Clone, PartialEq)]
struct FileMove {
    src: PathBuf,
    dest_dir: PathBuf,
    new_name: Option<String>,
    encrypt: bool,
    context: Option<ContextNote>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Inside `dir` (or equal)?
fn within(p: &Path, dir: &Path) -> bool {
    p.starts_with(dir)
}

// ── rules plan ──────────────────────────────────────────────────────────

/// What `rules` would do with the files already in `root` (files only;
/// `recursive` = also files in subfolders). Rules with `confirm = true` are
/// included too — applying a plan is itself the confirmation.
pub fn plan_from_rules(
    root: &Path,
    rules: &RuleSet,
    recursive: bool,
    filter: &IgnoreFilter,
    protected: &[PathBuf],
) -> Result<Plan, PlanError> {
    plan_from_rules_in(root, rules, recursive, filter, protected, None)
}

/// [`plan_from_rules`] with relative rule destinations and `{root}`
/// resolved against `anchor` (the watch folder's anchor, SPEC §14.1);
/// `None` = each file's own folder.
pub fn plan_from_rules_in(
    root: &Path,
    rules: &RuleSet,
    recursive: bool,
    filter: &IgnoreFilter,
    protected: &[PathBuf],
    anchor: Option<&Path>,
) -> Result<Plan, PlanError> {
    let root =
        dunce::canonicalize(root).map_err(|_| ScanError::NotADirectory(root.to_path_buf()))?;
    let mut moves = Vec::new();
    let walker = walkdir::WalkDir::new(&root)
        .min_depth(1)
        .max_depth(if recursive { usize::MAX } else { 1 })
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            !e.file_name().to_string_lossy().starts_with('.') && !filter.in_excluded_dir(e.path())
        });
    for e in walker.filter_map(Result::ok) {
        let path = e.path();
        if !e.file_type().is_file()
            || filter.is_ignored(path)
            || protected.iter().any(|p| within(path, p))
        {
            continue;
        }
        let Ok(facts) = FileFacts::from_path(path) else {
            continue;
        };
        let Some(c) = rules.classify_in(&facts, anchor) else {
            continue;
        };
        if path.parent() == Some(c.dest_dir.as_path()) {
            continue;
        }
        let to = c.dest_dir.join(&facts.name);
        let mut warnings = Vec::new();
        if protected.iter().any(|p| within(&to, p)) {
            continue;
        }
        if to.exists() {
            warnings.push("목적지에 같은 이름이 있어 '(1)'처럼 번호가 붙습니다".into());
        }
        if !within(&to, &root) {
            warnings.push("이 폴더 밖으로 옮겨집니다".into());
        }
        moves.push(PlannedMove {
            id: moves.len() as u32 + 1,
            from: path.to_path_buf(),
            to,
            is_dir: false,
            files: 1,
            bytes: facts.size,
            reason: if c.confirm {
                format!("{} (확인 규칙)", c.rule_name)
            } else {
                c.rule_name
            },
            warnings,
            context: None,
            encrypt: false,
        });
    }
    let n = moves.len();
    Ok(Plan {
        id: 0,
        root,
        source: PlanSource::Rules,
        created_at: now(),
        summary: format!("rules.toml을 지금 있는 파일에 적용하면 {n}개가 옮겨집니다."),
        folders: Vec::new(),
        moves,
        rejected: Vec::new(),
        applied_batch: None,
        anchor: None,
    })
}

// ── AI plan ─────────────────────────────────────────────────────────────

/// Max lines of the tree summary sent to the model.
const MAX_SUMMARY_LINES: usize = 600;
/// Root-level files are listed one by one up to this many.
const MAX_ROOT_FILES: usize = 300;
/// Room for a long list of moves.
const RESTRUCTURE_MAX_TOKENS: u32 = 16_000;

/// Tree summary for the restructure prompt: depth ≤ 3 folders with file
/// count, size and up to 5 example names; every root-level file.
pub fn tree_summary(tree: &DirNode) -> String {
    let mut lines: Vec<String> = Vec::new();
    for f in tree.files.iter().take(MAX_ROOT_FILES) {
        lines.push(format!("{}  ({} bytes)", f.name, f.size));
    }
    if tree.files.len() > MAX_ROOT_FILES {
        lines.push(format!(
            "… 루트 파일 {}개 더",
            tree.files.len() - MAX_ROOT_FILES
        ));
    }
    fn rec(node: &DirNode, path: &str, depth: usize, lines: &mut Vec<String>) {
        for (name, child) in &node.dirs {
            let p = join(path, name);
            let mut examples: Vec<&str> = child
                .files
                .iter()
                .take(5)
                .map(|f| f.name.as_str())
                .collect();
            if child.files.len() > 5 {
                examples.push("…");
            }
            lines.push(format!(
                "{p}/  ({} files, {} bytes){}{}",
                child.file_count,
                child.total_size,
                if examples.is_empty() { "" } else { "  예: " },
                examples.join(", ")
            ));
            if depth < 3 {
                rec(child, &p, depth + 1, lines);
            }
        }
    }
    rec(tree, "", 1, &mut lines);
    if lines.len() > MAX_SUMMARY_LINES {
        let more = lines.len() - MAX_SUMMARY_LINES;
        lines.truncate(MAX_SUMMARY_LINES);
        lines.push(format!("… {more}줄 생략"));
    }
    lines.join("\n") + "\n"
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    folders: Vec<FolderNote>,
    #[serde(default)]
    moves: Vec<RawMove>,
}

#[derive(Debug, Deserialize)]
struct RawMove {
    from: String,
    to: String,
    #[serde(default)]
    reason: String,
}

/// A model-supplied relative path → absolute path under `root`, or why not.
fn resolve_rel(root: &Path, rel: &str) -> Result<PathBuf, &'static str> {
    let rel = rel.trim().replace('\\', "/");
    if rel.starts_with("//") {
        return Err("루트 밖이나 절대 경로"); // UNC share
    }
    // A single leading "/" is read as "from the root".
    let rel = rel.trim_matches('/');
    if rel.is_empty() || rel == "." {
        return Err("빈 경로");
    }
    let p = Path::new(rel);
    if p.components().any(|c| {
        matches!(
            c,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) || rel.contains(':')
    {
        return Err("루트 밖이나 절대 경로");
    }
    Ok(normalize_lexical(&root.join(p)))
}

/// Ask the AI for a restructure plan of `root` (SPEC §5.7, §8.2) and
/// validate every move against the real tree.
pub fn plan_from_ai(
    root: &Path,
    ai: &AiClassifier,
    store: &Store,
    protected: &[PathBuf],
    timeout: std::time::Duration,
    max_moves: usize,
) -> Result<Plan, PlanError> {
    let res = scan(root, &ScanOptions::default())?;
    let root = res.root.clone();
    let summary = tree_summary(&res.tree);
    let protected_rel: Vec<String> = protected
        .iter()
        .filter_map(|p| p.strip_prefix(&root).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|p| !p.is_empty())
        .collect();
    let user = prompts::restructure_user(&root.display().to_string(), &summary, &protected_rel);
    let schema = prompts::restructure_schema();
    let raw: RawPlan = ai.call(
        store,
        &AiCall {
            purpose: "restructure",
            system: prompts::RESTRUCTURE_SYSTEM,
            user: &user,
            schema: &schema,
            max_tokens: RESTRUCTURE_MAX_TOKENS,
            timeout,
        },
        |v| {
            let raw: RawPlan = serde_json::from_value(v).map_err(|e| {
                AiError::BadResponse(format!("plan does not match the schema: {e}"))
            })?;
            let n = raw.moves.len();
            Ok((raw, Some(format!("{n} moves proposed"))))
        },
    )?;
    Ok(validate_ai_plan(
        &root, &res.tree, raw, protected, max_moves,
    ))
}

fn validate_ai_plan(
    root: &Path,
    tree: &DirNode,
    raw: RawPlan,
    protected: &[PathBuf],
    max_moves: usize,
) -> Plan {
    let mut moves: Vec<PlannedMove> = Vec::new();
    let mut rejected = Vec::new();
    let mut targets: HashSet<PathBuf> = HashSet::new();
    let reject = |r: &RawMove, why: &str, rejected: &mut Vec<Rejected>| {
        rejected.push(Rejected {
            from: r.from.clone(),
            to: r.to.clone(),
            reason: why.to_string(),
        })
    };
    // Sizes from the scan, for the plan summary.
    let mut sizes: HashMap<PathBuf, u64> = HashMap::new();
    walk(tree, "", &mut |dir, node| {
        for f in &node.files {
            sizes.insert(normalize_lexical(&root.join(join(dir, &f.name))), f.size);
        }
    });

    for r in raw.moves {
        if moves.len() >= max_moves {
            reject(&r, "이동 개수 상한 초과", &mut rejected);
            continue;
        }
        let (from, to) = match (resolve_rel(root, &r.from), resolve_rel(root, &r.to)) {
            (Ok(f), Ok(t)) => (f, t),
            (Err(e), _) | (_, Err(e)) => {
                reject(&r, e, &mut rejected);
                continue;
            }
        };
        if from == to {
            continue;
        }
        let meta = match std::fs::symlink_metadata(&from) {
            Ok(m) if m.is_file() || m.is_dir() => m,
            _ => {
                reject(&r, "원본이 없음", &mut rejected);
                continue;
            }
        };
        if protected
            .iter()
            .any(|p| within(&from, p) || within(&to, p) || within(p, &from))
        {
            reject(&r, "건드리지 말 폴더", &mut rejected);
            continue;
        }
        if meta.is_dir() && within(&to, &from) {
            reject(&r, "폴더를 자기 안으로 옮길 수 없음", &mut rejected);
            continue;
        }
        if moves.iter().any(|m| m.is_dir && within(&from, &m.from))
            || moves.iter().any(|m| within(&m.from, &from))
        {
            reject(&r, "다른 이동과 겹침", &mut rejected);
            continue;
        }
        if !targets.insert(to.clone()) {
            reject(&r, "같은 목적지로 가는 이동이 이미 있음", &mut rejected);
            continue;
        }
        let mut warnings = Vec::new();
        let (files, bytes) = if meta.is_dir() {
            let (mut n, mut b) = (0u64, 0u64);
            for (p, s) in &sizes {
                if within(p, &from) {
                    n += 1;
                    b += s;
                }
            }
            if to.is_dir() {
                warnings.push("기존 폴더와 합쳐집니다".into());
            }
            (n, b)
        } else {
            if name_of(&from) != name_of(&to) {
                warnings.push(format!("이름이 바뀝니다: {}", name_of(&to)));
            }
            if to.exists() {
                warnings.push("목적지에 같은 이름이 있어 '(1)'처럼 번호가 붙습니다".into());
            }
            (1, meta.len())
        };
        if meta.is_dir() && files == 0 {
            reject(&r, "빈 폴더 (옮길 파일 없음)", &mut rejected);
            continue;
        }
        moves.push(PlannedMove {
            id: moves.len() as u32 + 1,
            from,
            to,
            is_dir: meta.is_dir(),
            files,
            bytes,
            reason: r.reason.trim().chars().take(200).collect(),
            warnings,
            context: None,
            encrypt: false,
        });
    }
    Plan {
        id: 0,
        root: root.to_path_buf(),
        source: PlanSource::Ai,
        created_at: now(),
        summary: raw.summary.trim().chars().take(1000).collect(),
        folders: raw.folders,
        moves,
        rejected,
        applied_batch: None,
        anchor: None,
    }
}

// ── storage ─────────────────────────────────────────────────────────────

/// Store the plan and return it with its id.
pub fn save(store: &Store, mut plan: Plan) -> Result<Plan, PlanError> {
    let json = serde_json::to_string(&plan).map_err(|e| PlanError::Corrupt(e.to_string()))?;
    plan.id = store.save_plan(&plan.root, plan.source.as_str(), &json)?;
    let json = serde_json::to_string(&plan).map_err(|e| PlanError::Corrupt(e.to_string()))?;
    store.update_plan_json(plan.id, &json)?;
    Ok(plan)
}

pub fn load(store: &Store, id: i64) -> Result<Plan, PlanError> {
    let row = store.get_plan(id)?.ok_or(PlanError::NotFound(id))?;
    let mut plan: Plan =
        serde_json::from_str(&row.plan_json).map_err(|e| PlanError::Corrupt(e.to_string()))?;
    plan.id = row.id;
    plan.applied_batch = row.applied_batch;
    Ok(plan)
}

// ── preview & apply ─────────────────────────────────────────────────────

fn selected<'a>(plan: &'a Plan, ids: Option<&'a [u32]>) -> impl Iterator<Item = &'a PlannedMove> {
    plan.moves
        .iter()
        .filter(move |m| ids.is_none_or(|ids| ids.contains(&m.id)))
}

/// Expand moves into per-file moves (a folder move moves every file in it,
/// keeping the inner structure).
fn expand(plan: &Plan, ids: Option<&[u32]>) -> Vec<FileMove> {
    let mut out = Vec::new();
    for m in selected(plan, ids) {
        // Unclassified work-context items stay where they are.
        if m.from == m.to {
            continue;
        }
        if m.is_dir {
            let walker = walkdir::WalkDir::new(&m.from)
                .min_depth(1)
                .sort_by_file_name()
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
                .filter(|e| !e.file_name().to_string_lossy().contains(TMP_MARKER));
            for e in walker {
                let Ok(rel) = e.path().strip_prefix(&m.from) else {
                    continue;
                };
                let target = m.to.join(rel);
                out.push(FileMove {
                    src: e.path().to_path_buf(),
                    dest_dir: target.parent().unwrap_or(&m.to).to_path_buf(),
                    new_name: None,
                    encrypt: false,
                    context: None,
                });
            }
        } else {
            out.push(file_move(plan, m));
        }
    }
    out
}

/// One planned file move as a mover request's parts.
fn file_move(plan: &Plan, m: &PlannedMove) -> FileMove {
    let new = name_of(&m.to);
    // An encrypted move's `to` ends in ".zip"; the mover names it.
    let renamed = !m.encrypt && new != name_of(&m.from);
    FileMove {
        src: m.from.clone(),
        dest_dir: m.to.parent().unwrap_or(&plan.root).to_path_buf(),
        new_name: renamed.then_some(new),
        encrypt: m.encrypt,
        context: m.context.clone(),
    }
}

/// Before/after folder trees for the selected moves (`None` = all).
pub fn preview(plan: &Plan, ids: Option<&[u32]>) -> Result<Preview, PlanError> {
    let res = scan(&plan.root, &ScanOptions::default())?;
    let opts = TreeOptions {
        max_depth: Some(3),
        show_files: true,
        show_size: false,
        max_files_per_dir: 8,
    };
    let before = render_tree(&res.tree, &opts);

    let moves = expand(plan, ids);
    let by_src: HashMap<&Path, &FileMove> = moves.iter().map(|m| (m.src.as_path(), m)).collect();
    let mut after_files: Vec<(String, u64)> = Vec::new();
    let mut outside = Vec::new();
    let mut moved = 0u64;
    walk(&res.tree, "", &mut |dir, node| {
        for f in &node.files {
            let rel = join(dir, &f.name);
            let abs = normalize_lexical(&res.root.join(&rel));
            match by_src.get(abs.as_path()) {
                None => after_files.push((rel, f.size)),
                Some(m) => {
                    moved += 1;
                    let mut name = m.new_name.clone().unwrap_or_else(|| f.name.clone());
                    if m.encrypt {
                        name = crate::mover::zip_name_for(&name);
                    }
                    let target = m.dest_dir.join(&name);
                    match target.strip_prefix(&res.root) {
                        Ok(r) => after_files.push((r.to_string_lossy().replace('\\', "/"), f.size)),
                        Err(_) => outside.push((abs.clone(), target)),
                    }
                }
            }
        }
    });
    let after_tree = DirNode::from_files(
        &res.tree.name,
        after_files.iter().map(|(p, s)| (p.as_str(), *s)),
    );
    let mut after_dirs: HashSet<String> = HashSet::new();
    walk(&after_tree, "", &mut |dir, node| {
        if node.file_count > 0 {
            after_dirs.insert(dir.to_string());
        }
    });
    let mut emptied = Vec::new();
    walk(&res.tree, "", &mut |dir, node| {
        if !dir.is_empty() && node.file_count > 0 && !after_dirs.contains(dir) {
            emptied.push(dir.to_string());
        }
    });
    // Folders moved as a whole go with their contents (M12): not "left
    // empty". Report only the top-most remaining ones.
    let moved_dirs: Vec<String> = selected(plan, ids)
        .filter(|m| m.is_dir && m.from != m.to)
        .filter_map(|m| m.from.strip_prefix(&res.root).ok())
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .collect();
    let tops: Vec<String> = emptied
        .iter()
        .filter(|d| {
            !emptied
                .iter()
                .any(|o| *d != o && d.starts_with(&format!("{o}/")))
        })
        .filter(|d| {
            !moved_dirs
                .iter()
                .any(|m| *d == m || d.starts_with(&format!("{m}/")))
        })
        .cloned()
        .collect();
    Ok(Preview {
        before,
        after: render_tree(&after_tree, &opts),
        outside,
        files_moved: moved,
        emptied: tops,
    })
}

/// Apply the selected moves of a saved plan (`None` = all) as ONE batch
/// through the mover. Only called on explicit user approval.
pub fn apply(
    mover: &Mover,
    plan: &Plan,
    ids: Option<&[u32]>,
    protected: &[PathBuf],
    index: Option<&IndexOptions>,
) -> Result<ApplyReport, PlanError> {
    if let Some(batch) = &plan.applied_batch {
        return Err(PlanError::AlreadyApplied {
            id: plan.id,
            batch: batch.clone(),
        });
    }
    let store = mover.store();
    let batch_id = new_batch_id();
    let label = match plan.source {
        PlanSource::Context => format!("업무 맥락 #{}", plan.id),
        _ => format!("정리 제안 #{}", plan.id),
    };
    let mut report = ApplyReport {
        batch_id: batch_id.clone(),
        ..Default::default()
    };
    let record = |report: &mut ApplyReport,
                  out: Result<MoveOutcome, String>,
                  src: &Path,
                  note: Option<&ContextNote>,
                  encrypted: bool| match out {
        Ok(MoveOutcome::Moved { log_id, src, dst }) => {
            if let Some(note) = note {
                // Remember the folder date this group got, so new files
                // of the same issue join it (SPEC §14.3).
                crate::curate::remember_applied(store, note, &dst);
            }
            if let Some(opts) = index {
                reindex_for(
                    store,
                    &Action::Moved {
                        src: src.clone(),
                        dst: dst.clone(),
                        rule: label.clone(),
                        batch_id: batch_id.clone(),
                        log_id,
                        by: DecidedBy::Restructure,
                        confidence: None,
                        encrypted,
                    },
                    opts,
                );
            }
            report.moved.push((src, dst));
        }
        Ok(MoveOutcome::SkippedDuplicate { src, existing }) => report.skipped.push((
            src,
            format!("같은 내용이 이미 있음: {}", existing.display()),
        )),
        Ok(MoveOutcome::AlreadyInPlace(p)) => report.skipped.push((p, "이미 그 자리".into())),
        Err(e) => report.errors.push((src.to_path_buf(), e)),
    };
    for m in selected(plan, ids) {
        // Unclassified work-context items stay where they are.
        if m.from == m.to {
            continue;
        }
        if protected
            .iter()
            .any(|p| within(&m.from, p) || within(&m.to, p))
        {
            report
                .skipped
                .push((m.from.clone(), "건드리지 말 폴더".into()));
            continue;
        }
        if m.is_dir {
            // (M12) A folder moves as one unit: every file in the batch,
            // emptied source folders cleaned up (SPEC §14.7).
            if !m.from.is_dir() {
                report.skipped.push((
                    m.from.clone(),
                    "폴더가 없음 (이미 옮겨졌거나 지워짐)".into(),
                ));
                continue;
            }
            let req = DirMoveRequest {
                src: m.from.clone(),
                dest: m.to.clone(),
                batch_id: batch_id.clone(),
                decided_by: DecidedBy::Restructure,
                rule_name: Some(label.clone()),
                ai_confidence: m.context.as_ref().map(|c| c.confidence),
            };
            match mover.move_dir(&req) {
                Ok(r) => {
                    for o in r.outcomes {
                        let src = match &o {
                            MoveOutcome::Moved { src, .. }
                            | MoveOutcome::SkippedDuplicate { src, .. } => src.clone(),
                            MoveOutcome::AlreadyInPlace(p) => p.clone(),
                        };
                        record(&mut report, Ok(o), &src, None, false);
                    }
                    for (f, e) in r.errors {
                        report.errors.push((f, e));
                    }
                    if let Some(note) = &m.context {
                        crate::curate::remember_applied(store, note, &m.to);
                    }
                }
                Err(e) => report.errors.push((m.from.clone(), e.to_string())),
            }
            continue;
        }
        let fm = file_move(plan, m);
        if !fm.src.is_file() {
            report
                .skipped
                .push((fm.src, "파일이 없음 (이미 옮겨졌거나 지워짐)".into()));
            continue;
        }
        let req = MoveRequest {
            src: fm.src.clone(),
            dest_dir: fm.dest_dir.clone(),
            batch_id: batch_id.clone(),
            decided_by: DecidedBy::Restructure,
            rule_name: Some(label.clone()),
            ai_confidence: fm.context.as_ref().map(|c| c.confidence),
            new_name: fm.new_name.clone(),
            encrypt: fm.encrypt,
        };
        if fm.encrypt {
            if let Some(opts) = index {
                crate::engine::prepare_vault_index(store, &fm.src, opts);
            }
        }
        let out = mover.move_file(&req).map_err(|e| e.to_string());
        record(&mut report, out, &fm.src, fm.context.as_ref(), fm.encrypt);
    }
    if plan.id > 0 && !report.moved.is_empty() {
        store.mark_plan_applied(plan.id, &batch_id)?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::MockProvider;
    use crate::config::{Config, DuplicatePolicy};
    use serde_json::json;
    use std::fs;
    use std::sync::Arc;

    fn desk() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap().join("바탕 화면");
        fs::create_dir_all(root.join("옛 자료").join("2024")).unwrap();
        fs::create_dir_all(root.join("업무")).unwrap();
        fs::write(root.join("KT 과금 9월.xlsx"), "9").unwrap();
        fs::write(root.join("KT 과금 10월.xlsx"), "10").unwrap();
        fs::write(root.join("스크린샷 1.png"), "p").unwrap();
        fs::write(root.join("옛 자료").join("a.txt"), "a").unwrap();
        fs::write(root.join("옛 자료").join("2024").join("b.txt"), "bb").unwrap();
        fs::write(root.join("업무").join("계약서.pdf"), "c").unwrap();
        (tmp, root)
    }

    fn ai_with(answer: serde_json::Value) -> AiClassifier {
        let mut cfg = Config::default();
        cfg.ai.enabled = true;
        AiClassifier::with_provider(
            Arc::new(MockProvider::new(answer)),
            &cfg,
            &RuleSet::default(),
        )
        .unwrap()
    }

    #[test]
    fn ai_plan_is_validated_previewed_and_applied_as_one_batch() {
        let (_tmp, root) = desk();
        let store = Store::open_in_memory().unwrap();
        let ai = ai_with(json!({
            "summary": "업무 파일을 모으고 옛 자료를 보관함으로",
            "folders": [{"path": "업무/KT", "reason": "KT 과금 문서"}],
            "moves": [
                {"from": "KT 과금 9월.xlsx", "to": "업무/KT/KT 과금 9월.xlsx", "reason": "KT"},
                {"from": "KT 과금 10월.xlsx", "to": "업무/KT/KT 과금 10월.xlsx", "reason": "KT"},
                {"from": "옛 자료", "to": "보관/옛 자료", "reason": "오래된 자료"},
                {"from": "../밖.txt", "to": "x.txt", "reason": "탈출"},
                {"from": "C:/Windows/x", "to": "x", "reason": "절대"},
                {"from": "없는 파일.txt", "to": "y.txt", "reason": "없음"},
                {"from": "업무", "to": "업무/하위", "reason": "자기 안으로"},
                {"from": "옛 자료/a.txt", "to": "z.txt", "reason": "겹침"},
                {"from": "스크린샷 1.png", "to": "개인/스크린샷 1.png", "reason": "보호"}
            ]
        }));
        let protected = vec![root.join("개인")];
        let plan = plan_from_ai(
            &root,
            &ai,
            &store,
            &protected,
            std::time::Duration::from_secs(5),
            500,
        )
        .unwrap();
        assert_eq!(plan.moves.len(), 3, "{:#?}", plan.rejected);
        assert_eq!(plan.rejected.len(), 6);
        let dir_move = plan.moves.iter().find(|m| m.is_dir).unwrap();
        assert_eq!((dir_move.files, dir_move.bytes), (2, 3));
        assert_eq!(store.recent_ai_log(1).unwrap()[0].purpose, "restructure");

        let plan = save(&store, plan).unwrap();
        let loaded = load(&store, plan.id).unwrap();
        assert_eq!(loaded, plan);

        // Preview only the KT moves.
        let p = preview(&plan, Some(&[1, 2])).unwrap();
        assert_eq!(p.files_moved, 2);
        assert!(p.after.contains("KT"), "{}", p.after);
        assert!(p.outside.is_empty());
        assert!(p.emptied.is_empty());
        // "옛 자료" moves as a whole (M12): it is not "left empty".
        assert_eq!(preview(&plan, None).unwrap().emptied, Vec::<String>::new());

        // Apply everything except nothing is auto: explicit call.
        let mover = Mover::new(store, DuplicatePolicy::Skip);
        let r = apply(&mover, &plan, None, &protected, None).unwrap();
        assert_eq!(r.moved.len(), 4, "{r:#?}");
        assert!(root
            .join("업무")
            .join("KT")
            .join("KT 과금 9월.xlsx")
            .is_file());
        assert!(root
            .join("보관")
            .join("옛 자료")
            .join("2024")
            .join("b.txt")
            .is_file());
        // (M12) A folder moved as a whole leaves no empty shell behind
        // (only empty folders are removed; SPEC §14.7).
        assert!(!root.join("옛 자료").exists());
        let e = mover.store().moves_in_batch(&r.batch_id).unwrap();
        assert_eq!(e.len(), 4);
        assert!(e.iter().all(|m| m.decided_by == DecidedBy::Restructure));
        let plan = load(mover.store(), plan.id).unwrap();
        assert!(matches!(
            apply(&mover, &plan, None, &protected, None),
            Err(PlanError::AlreadyApplied { .. })
        ));

        // Whole restructure undone at once.
        let u = mover.undo_batch(&r.batch_id).unwrap();
        assert_eq!(u.restored.len(), 4);
        assert!(root.join("KT 과금 9월.xlsx").is_file());
        assert!(root.join("옛 자료").join("2024").join("b.txt").is_file());
    }

    #[test]
    fn rules_plan_needs_no_ai() {
        let (_tmp, root) = desk();
        let out = root.parent().unwrap().join("사진 정리");
        let rules = RuleSet::from_toml(&format!(
            "[[rule]]\nname='스크린샷'\nmatch={{ext=['png']}}\ndest='{}'\n\
             [[rule]]\nname='엑셀'\nmatch={{ext=['xlsx']}}\ndest='{}/업무'\nconfirm=true\n",
            out.display().to_string().replace('\\', "/"),
            root.display().to_string().replace('\\', "/")
        ))
        .unwrap();
        let plan =
            plan_from_rules(&root, &rules, false, &IgnoreFilter::new(&[]).unwrap(), &[]).unwrap();
        assert_eq!(plan.moves.len(), 3);
        assert!(plan.moves.iter().any(|m| m.reason == "엑셀 (확인 규칙)"));
        let png = plan.moves.iter().find(|m| m.reason == "스크린샷").unwrap();
        assert!(png.warnings.iter().any(|w| w.contains("밖")));
        let p = preview(&plan, None).unwrap();
        assert_eq!(p.outside.len(), 1);
        assert_eq!(p.files_moved, 3);
        // Selecting nothing changes nothing.
        let mover = Mover::new(Store::open_in_memory().unwrap(), DuplicatePolicy::Skip);
        let r = apply(&mover, &plan, Some(&[]), &[], None).unwrap();
        assert!(r.moved.is_empty());
        assert!(root.join("스크린샷 1.png").exists());
    }

    #[test]
    fn model_paths_stay_under_root() {
        let root = Path::new("/r");
        assert_eq!(
            resolve_rel(root, "업무/KT").unwrap(),
            Path::new("/r").join("업무").join("KT")
        );
        assert_eq!(
            resolve_rel(root, "/업무").unwrap(),
            Path::new("/r").join("업무")
        );
        assert_eq!(
            resolve_rel(root, "업무\\KT").unwrap(),
            Path::new("/r").join("업무").join("KT")
        );
        for bad in [
            "",
            ".",
            "../x",
            "a/../../x",
            "C:/Windows",
            "\\\\nas\\share\\x",
            "//nas/x",
        ] {
            assert!(resolve_rel(root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn summary_lists_root_files_and_folder_counts() {
        let (_tmp, root) = desk();
        let res = scan(&root, &ScanOptions::default()).unwrap();
        let s = tree_summary(&res.tree);
        assert!(s.contains("KT 과금 9월.xlsx"));
        assert!(s.contains("옛 자료/  (2 files"), "{s}");
        assert!(s.contains("옛 자료/2024/"));
    }
}
