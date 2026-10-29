//! (M16, SPEC §14.13) What the NAS agent and the PC app send each other.
//! JSON, camelCase like the app's own DTOs so the app can pass most of it
//! straight to the screen. Paths are the NAS's own path strings.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{DuplicatePolicy, OrganizeMode, WatchRoot};
use crate::pending::Decision;
use crate::search::{MatchedIn, SearchHit};
use crate::store::{OcrState, PendingEntry, PendingSource};

/// Path prefix of every endpoint.
pub const API: &str = "/api/v1";
/// Default port of `tidy serve`.
pub const DEFAULT_PORT: u16 = 7841;
/// Largest request body the agent reads.
pub const MAX_BODY: usize = 1024 * 1024;

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn name_and_dir(p: &Path) -> (String, String) {
    (
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        p.parent().map(s).unwrap_or_default(),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    /// Tidy version of the agent.
    pub version: String,
    /// The folder the PC may pick from (`TIDY_DATA_ROOT`).
    pub data_root: String,
    pub watches: Vec<AgentWatchState>,
    /// Watching is running (false: no folders yet, or an error).
    pub watching: bool,
    /// Why watching is not running, if it should be.
    pub error: Option<String>,
    /// Files waiting in the NAS's confirm queue.
    pub pending: u64,
    /// Indexed files (search).
    pub files: u64,
    /// Pictures / scans still waiting for text recognition.
    pub ocr_waiting: u64,
    /// Files organized since local midnight of the agent.
    pub today_moved: u64,
    /// Unix seconds the agent started.
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentWatchState {
    pub path: String,
    pub alias: Option<String>,
    pub online: bool,
    /// "in_place" | "collect"
    pub mode: String,
    pub organize_root: Option<String>,
    /// Watched by polling (event limit reached, or set so).
    pub polling: bool,
}

/// One search result (the app's `SearchHit` shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHit {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub ext: Option<String>,
    pub size: u64,
    pub mtime: i64,
    /// name | content | both | filter | semantic
    pub matched_in: String,
    pub name_highlight: String,
    pub snippet: String,
    pub text_status: String,
    pub exists: bool,
    pub similarity: Option<f32>,
    pub vault: bool,
    pub pii: Option<String>,
    pub ocr: bool,
}

impl From<SearchHit> for RemoteHit {
    fn from(h: SearchHit) -> Self {
        let (name, dir) = name_and_dir(&h.file.path);
        Self {
            exists: h.file.path.exists(),
            path: s(&h.file.path),
            name,
            dir,
            ext: h.file.ext,
            size: h.file.size,
            mtime: h.file.mtime,
            matched_in: match h.matched_in {
                MatchedIn::Name => "name",
                MatchedIn::Content => "content",
                MatchedIn::Both => "both",
                MatchedIn::Filter => "filter",
                MatchedIn::Semantic => "semantic",
            }
            .into(),
            name_highlight: h.name_highlight,
            snippet: h.snippet,
            text_status: h.file.text_status.as_str().into(),
            similarity: h.similarity,
            vault: h.file.vault,
            pii: h
                .file
                .pii_flags
                .filter(|f| *f != 0)
                .map(|f| crate::pii::summary(f, &h.file.pii_counts.unwrap_or_default())),
            ocr: h.file.ocr == Some(OcrState::Ok),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSearch {
    pub hits: Vec<RemoteHit>,
    pub note: Option<String>,
}

/// One confirm-queue entry (the app's `PendingItem` shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemotePending {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub dir: String,
    pub suggested_dest: Option<String>,
    pub rule: Option<String>,
    /// "rule" | "ai"
    pub source: String,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
    pub suggested_name: Option<String>,
    pub created_at: i64,
    pub exists: bool,
    pub size: Option<u64>,
    pub encrypt: bool,
    pub pii: Option<String>,
    pub duplicate_of: Option<String>,
}

impl From<PendingEntry> for RemotePending {
    fn from(p: PendingEntry) -> Self {
        let meta = std::fs::metadata(&p.path).ok();
        let (name, dir) = name_and_dir(&p.path);
        Self {
            id: p.id,
            path: s(&p.path),
            name,
            dir,
            suggested_dest: p.suggested_dest.as_deref().map(s),
            source: p.source.as_str().into(),
            rule: (p.source == PendingSource::Rule)
                .then(|| p.reason.clone())
                .flatten(),
            reason: (p.source == PendingSource::Ai)
                .then(|| p.reason.clone())
                .flatten(),
            confidence: p.confidence,
            suggested_name: p.suggested_name,
            created_at: p.created_at,
            exists: meta.as_ref().is_some_and(|m| m.is_file()),
            size: meta.map(|m| m.len()),
            encrypt: p.encrypt,
            pii: p.pii.filter(|f| f.is_pii()).map(|f| f.summary()),
            duplicate_of: p
                .suggested_dest
                .as_deref()
                .filter(|_| !p.encrypt)
                .and_then(|d| crate::pending::duplicate_at_dest(&p.path, d, None))
                .map(|e| s(&e)),
        }
    }
}

/// The user's answer to a queued file (the app's `Decision` shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RemoteDecision {
    #[serde(rename_all = "camelCase")]
    Move {
        dest_dir: Option<String>,
        #[serde(default)]
        new_name: Option<String>,
        #[serde(default)]
        on_duplicate: Option<DuplicatePolicy>,
    },
    Skip,
    Ignore,
}

impl From<RemoteDecision> for Decision {
    fn from(d: RemoteDecision) -> Self {
        match d {
            RemoteDecision::Move {
                dest_dir,
                new_name,
                on_duplicate,
            } => Decision::Move {
                dest_dir: dest_dir
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty())
                    .map(Into::into),
                new_name: new_name.filter(|n| !n.trim().is_empty()),
                on_duplicate,
            },
            RemoteDecision::Skip => Decision::Skip,
            RemoteDecision::Ignore => Decision::Ignore,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolveRequest {
    pub ids: Vec<i64>,
    pub decision: RemoteDecision,
}

/// Outcome of one queued file (the app's `Resolution` shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteResolution {
    /// moved | duplicate | already_in_place | skipped | ignored | gone | error
    pub kind: String,
    pub path: String,
    pub dst: Option<String>,
    pub batch_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UndoRequest {
    pub batch: String,
}

/// The app's `UndoResult` shape.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteUndo {
    pub restored: Vec<(String, String)>,
    pub conflicts: Vec<String>,
    pub in_trash: Vec<String>,
}

impl From<&crate::mover::UndoReport> for RemoteUndo {
    fn from(r: &crate::mover::UndoReport) -> Self {
        let trash = |c: &&crate::mover::UndoConflict| c.reason == crate::mover::TRASH_UNDO_HINT;
        Self {
            restored: r.restored.iter().map(|(a, b)| (s(a), s(b))).collect(),
            conflicts: r
                .conflicts
                .iter()
                .map(|c| {
                    let p = if trash(&c) { &c.src_path } else { &c.dst_path };
                    format!("{}: {}", s(p), c.reason)
                })
                .collect(),
            in_trash: r
                .conflicts
                .iter()
                .filter(trash)
                .map(|c| s(&c.src_path))
                .collect(),
        }
    }
}

/// One watch folder as the PC edits it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentWatch {
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub mode: OrganizeMode,
    #[serde(default)]
    pub organize_root: Option<String>,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl From<&WatchRoot> for AgentWatch {
    fn from(w: &WatchRoot) -> Self {
        Self {
            path: s(&w.path),
            alias: w.alias.clone(),
            mode: w.mode,
            organize_root: w.organize_root.as_deref().map(s),
            recursive: w.recursive,
            exclude: w.exclude.iter().map(|e| s(e)).collect(),
        }
    }
}

impl AgentWatch {
    /// A local folder on the NAS: change events, body text indexed.
    pub fn to_root(&self) -> WatchRoot {
        WatchRoot {
            path: self.path.trim().into(),
            alias: self
                .alias
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(Into::into),
            recursive: self.recursive,
            mode: self.mode,
            organize_root: self
                .organize_root
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(Into::into),
            exclude: self
                .exclude
                .iter()
                .filter(|e| !e.trim().is_empty())
                .map(|e| e.trim().into())
                .collect(),
            ..WatchRoot::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettings {
    /// Read-only: the folder everything must be inside.
    #[serde(default)]
    pub data_root: String,
    pub watches: Vec<AgentWatch>,
    /// Folders indexed for search only (nothing moves there).
    #[serde(default)]
    pub index_only: Vec<String>,
    /// rules.toml text.
    pub rules: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDir {
    pub name: String,
    pub path: String,
    pub has_children: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizeRequest {
    pub path: String,
}

/// "규칙대로 지금 정리" preview: nothing moved yet.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizePreview {
    pub path: String,
    /// Files the rules would move.
    pub to_move: usize,
    /// A few (from, to) examples.
    pub examples: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizeResult {
    pub batch_id: String,
    pub moved: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorBody {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_matches_the_app_shape() {
        let d: RemoteDecision = serde_json::from_str(
            r#"{"kind":"move","destDir":"/data/a","newName":null,"onDuplicate":"rename"}"#,
        )
        .unwrap();
        assert_eq!(
            Decision::from(d),
            Decision::Move {
                dest_dir: Some("/data/a".into()),
                new_name: None,
                on_duplicate: Some(DuplicatePolicy::Rename)
            }
        );
        let d: RemoteDecision = serde_json::from_str(r#"{"kind":"skip"}"#).unwrap();
        assert_eq!(Decision::from(d), Decision::Skip);
    }
}
