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
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IndexStats {
    pub roots: u64,
    pub files: u64,
    pub with_text: u64,
    pub total_bytes: u64,
}

/// A row of `pending` (file waiting for user confirmation).
#[derive(Debug, Clone, Serialize)]
pub struct PendingEntry {
    pub id: i64,
    pub path: PathBuf,
    pub suggested_dest: Option<PathBuf>,
    pub reason: Option<String>,
    pub created_at: i64,
}
