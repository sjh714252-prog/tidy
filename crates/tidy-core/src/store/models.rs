use std::path::PathBuf;

use serde::Serialize;

/// Who decided a move (`move_log.decided_by`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DecidedBy {
    Rule,
    Ai,
    User,
    Restructure,
}

impl DecidedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            DecidedBy::Rule => "rule",
            DecidedBy::Ai => "ai",
            DecidedBy::User => "user",
            DecidedBy::Restructure => "restructure",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "rule" => DecidedBy::Rule,
            "ai" => DecidedBy::Ai,
            "user" => DecidedBy::User,
            "restructure" => DecidedBy::Restructure,
            _ => return None,
        })
    }
}

/// A row about to be inserted into `move_log`.
#[derive(Debug, Clone)]
pub struct NewMove {
    pub batch_id: String,
    pub src_path: PathBuf,
    pub dst_path: PathBuf,
    pub hash: Option<String>,
    pub size: Option<u64>,
    pub decided_by: DecidedBy,
    pub rule_name: Option<String>,
    pub ai_confidence: Option<f64>,
    /// (M9) `encrypt`: `dst_path` is a password-protected zip of the file.
    pub op: MoveOp,
}

/// `move_log.op` (M9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MoveOp {
    #[default]
    Move,
    /// Moved into an AES-256 zip (`<name>.zip`); undo decrypts it back.
    Encrypt,
    /// (M14) Sent to the OS trash as an identical copy of `dst_path` (the
    /// copy that stays). Undo cannot restore it; it says so.
    Trash,
}

impl MoveOp {
    pub fn as_str(self) -> &'static str {
        match self {
            MoveOp::Move => "move",
            MoveOp::Encrypt => "encrypt",
            MoveOp::Trash => "trash",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "encrypt" => MoveOp::Encrypt,
            "trash" => MoveOp::Trash,
            _ => MoveOp::Move,
        }
    }
}

/// A row of `move_log`.
#[derive(Debug, Clone, Serialize)]
pub struct MoveLogEntry {
    pub id: i64,
    pub batch_id: String,
    pub src_path: PathBuf,
    pub dst_path: PathBuf,
    pub hash: Option<String>,
    pub size: Option<u64>,
    pub decided_by: DecidedBy,
    pub rule_name: Option<String>,
    pub ai_confidence: Option<f64>,
    /// Unix seconds.
    pub moved_at: i64,
    /// Unix seconds; `None` while the move is still in effect.
    pub undone_at: Option<i64>,
    /// (M6) The row was written but the file transfer has not finished
    /// (a long copy to a NAS is running, or it was interrupted by a crash
    /// and waits for [`crate::mover::Mover::recover_interrupted`]).
    pub in_progress: bool,
    pub op: MoveOp,
    /// (M9) Hash of the zip an encrypt produced.
    pub dst_hash: Option<String>,
}

/// `files.text_status`: what happened to the body text of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextStatus {
    /// Not attempted (metadata-only root).
    None,
    /// Text extracted and indexed.
    Ok,
    /// No extractor for this file type.
    Unsupported,
    /// An extractor exists but failed; metadata is still indexed.
    Failed,
    /// Larger than `index.max_content_size_mb`; metadata only.
    Skipped,
}

impl TextStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TextStatus::None => "none",
            TextStatus::Ok => "ok",
            TextStatus::Unsupported => "unsupported",
            TextStatus::Failed => "failed",
            TextStatus::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => TextStatus::None,
            "ok" => TextStatus::Ok,
            "unsupported" => TextStatus::Unsupported,
            "failed" => TextStatus::Failed,
            "skipped" => TextStatus::Skipped,
            _ => return None,
        })
    }
}

/// What the indexer needs to decide whether a file changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub id: i64,
    pub size: u64,
    /// Unix seconds.
    pub mtime: i64,
    /// Body text was never attempted (`text_status = none`): indexed as
    /// metadata only, e.g. a NAS folder outside its content window.
    pub text_none: bool,
    /// (M9) The row is an encrypted vault zip: its text came from the
    /// file before it was locked, so it is never re-extracted.
    pub vault: bool,
    /// (M9) Text was extracted but no personal-information verdict was
    /// stored yet (indexed before M9): judge it from the stored text.
    pub pii_unjudged: bool,
}

/// A file row about to be written (insert or replace by path).
#[derive(Debug, Clone)]
pub struct NewFile {
    pub path: PathBuf,
    pub root_id: i64,
    pub name: String,
    pub ext: Option<String>,
    pub mime: Option<String>,
    pub size: u64,
    pub mtime: i64,
    pub ctime: Option<i64>,
    pub hash: Option<String>,
    pub text_status: TextStatus,
    pub excerpt: Option<String>,
    /// Full extracted text (already truncated by the indexer).
    pub content: Option<String>,
    /// (M9) Personal-information verdict (`None` = not judged).
    pub pii: Option<crate::pii::PiiFinding>,
}

/// A row of `files` without the body text.
#[derive(Debug, Clone, Serialize)]
pub struct FileRecord {
    pub id: i64,
    pub path: PathBuf,
    pub root_id: i64,
    pub name: String,
    pub ext: Option<String>,
    pub mime: Option<String>,
    pub size: u64,
    pub mtime: i64,
    pub hash: Option<String>,
    pub text_status: TextStatus,
    pub excerpt: Option<String>,
    pub indexed_at: i64,
    /// (M9) `None` = not judged ("판정 불가"), 0 = clean.
    pub pii_flags: Option<u32>,
    pub pii_counts: Option<crate::pii::PiiCounts>,
    /// (M9) The path is an encrypted vault zip (🔒).
    pub vault: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IndexStats {
    pub roots: u64,
    pub files: u64,
    pub with_text: u64,
    pub total_bytes: u64,
}

/// Who put a file in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingSource {
    /// A rule with `confirm = true` (`reason` = rule name).
    Rule,
    /// An AI suggestion below the auto threshold (`reason` = model's reason).
    Ai,
}

impl PendingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingSource::Rule => "rule",
            PendingSource::Ai => "ai",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "ai" {
            PendingSource::Ai
        } else {
            PendingSource::Rule
        }
    }
}

/// A row of `pending` (file waiting for user confirmation).
#[derive(Debug, Clone, Serialize)]
pub struct PendingEntry {
    pub id: i64,
    pub path: PathBuf,
    pub suggested_dest: Option<PathBuf>,
    /// Rule name (source = rule) or the AI's one-line reason (source = ai).
    pub reason: Option<String>,
    pub created_at: i64,
    pub source: PendingSource,
    /// AI confidence 0..1 (source = ai).
    pub confidence: Option<f64>,
    /// AI-suggested new file name, if any.
    pub suggested_name: Option<String>,
    /// (M9) Approving encrypts the file into a zip at the destination.
    pub encrypt: bool,
    /// (M9) What the detector found (badge "개인정보: 주민번호 2 · …").
    pub pii: Option<crate::pii::PiiFinding>,
}

/// Input for [`Store::upsert_pending_entry`](super::Store::upsert_pending_entry).
#[derive(Debug, Clone, Copy)]
pub struct NewPending<'a> {
    pub path: &'a std::path::Path,
    pub suggested_dest: Option<&'a std::path::Path>,
    pub reason: Option<&'a str>,
    pub source: PendingSource,
    pub confidence: Option<f64>,
    pub suggested_name: Option<&'a str>,
    pub encrypt: bool,
    pub pii: Option<&'a crate::pii::PiiFinding>,
}

/// What the user did with a queued file (`decisions.action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAction {
    Move,
    RenameMove,
    Skip,
    Ignore,
}

impl DecisionAction {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionAction::Move => "move",
            DecisionAction::RenameMove => "rename_move",
            DecisionAction::Skip => "skip",
            DecisionAction::Ignore => "ignore",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "rename_move" => DecisionAction::RenameMove,
            "skip" => DecisionAction::Skip,
            "ignore" => DecisionAction::Ignore,
            _ => DecisionAction::Move,
        }
    }
}

/// A row of `decisions` (learner input, SPEC §5.2).
#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub id: i64,
    pub file_name: String,
    pub ext: Option<String>,
    pub src_dir: PathBuf,
    pub suggested_dest: Option<PathBuf>,
    /// `None` for skip / ignore.
    pub chosen_dest: Option<PathBuf>,
    pub action: DecisionAction,
    pub source: PendingSource,
    pub decided_at: i64,
}

/// One AI call (`ai_log`). Never contains keys or file contents.
#[derive(Debug, Clone, Serialize, Default)]
pub struct AiLogEntry {
    pub id: i64,
    pub provider: String,
    pub model: String,
    /// classify | embed | restructure | test
    pub purpose: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<u64>,
    pub ok: bool,
    pub error: Option<String>,
    /// Short summary of the answer (destination + confidence).
    pub result: Option<String>,
    pub called_at: i64,
}
