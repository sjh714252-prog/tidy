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

/// A row of `pending` (file waiting for user confirmation).
#[derive(Debug, Clone, Serialize)]
pub struct PendingEntry {
    pub id: i64,
    pub path: PathBuf,
    pub suggested_dest: Option<PathBuf>,
    pub reason: Option<String>,
    pub created_at: i64,
}
