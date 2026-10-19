//! Engine: one file in → classify → (confirm? pending : move). Shared by
//! `tidy watch`, `--process-existing` and the Tauri app.
//!
//! Pipeline (SPEC §5.2): rules → (no rule) AI → (no AI, AI failure or low
//! confidence) ask the user. Without AI the engine behaves exactly as in M1:
//! files no rule matches are left alone.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::classify::{AiClassifier, Anchors, FileFacts, RuleSet};
use crate::index::{IndexOptions, Indexer};
use crate::mover::{hash_file, MoveOutcome, MoveRequest, Mover, MoverError};
use crate::store::{DecidedBy, NewPending, PendingSource, StoreError};
use crate::watcher::IgnoreFilter;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Mover(#[from] MoverError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl EngineError {
    /// Retry later (file locked / still being written by another app).
    pub fn is_transient(&self) -> bool {
        match self {
            EngineError::Mover(e) => e.is_transient(),
            EngineError::Io { source, .. } => source.kind() == std::io::ErrorKind::PermissionDenied,
            EngineError::Store(_) => false,
        }
    }
}

/// What happened to one file.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Moved {
        src: PathBuf,
        dst: PathBuf,
        /// Rule name, or "AI" for an automatic AI move.
        rule: String,
        batch_id: String,
        log_id: i64,
        by: DecidedBy,
        /// AI confidence (AI moves only).
        confidence: Option<f64>,
    },
    /// Waiting for the user: a rule with `confirm = true`, or an AI
    /// suggestion that may not move on its own.
    Pending {
        path: PathBuf,
        /// Rule name, or "AI".
        rule: String,
        suggested: PathBuf,
        source: PendingSource,
        confidence: Option<f64>,
        /// The AI's one-line reason.
        reason: Option<String>,
    },
    /// `--dry-run`: what would happen.
    WouldMove {
        path: PathBuf,
        rule: String,
        dest_dir: PathBuf,
        confirm: bool,
    },
    Duplicate {
        src: PathBuf,
        existing: PathBuf,
        rule: String,
    },
    AlreadyInPlace {
        path: PathBuf,
        rule: String,
    },
    NoRule(PathBuf),
    /// No rule matched and the AI could not help (offline, no key, timeout,
    /// bad answer): treated like [`Action::NoRule`], the file stays.
    AiFailed {
        path: PathBuf,
        error: String,
    },
    Skipped {
        path: PathBuf,
        reason: &'static str,
    },
}

pub struct Engine {
    rules: RuleSet,
    mover: Mover,
    filter: IgnoreFilter,
    dry_run: bool,
    /// When set, every handled file is reflected in the search index
    /// (moved files under their new path, the rest in place).
    index: Option<IndexOptions>,
    /// AI step for files no rule matches (None = rules-only mode).
    ai: Option<AiClassifier>,
    /// Watch folder anchors for relative destinations / `{root}` (M8).
    anchors: Anchors,
}

pub fn new_batch_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl Engine {
    pub fn new(rules: RuleSet, mover: Mover, filter: IgnoreFilter) -> Self {
        Self {
            rules,
            mover,
            filter,
            dry_run: false,
            index: None,
            ai: None,
            anchors: Anchors::default(),
        }
    }

    /// Resolve relative rule destinations against the watch folders'
    /// anchors (SPEC §14.1) instead of each file's own folder.
    pub fn with_anchors(mut self, anchors: Anchors) -> Self {
        self.anchors = anchors;
        self
    }

    pub fn dry_run(mut self, on: bool) -> Self {
        self.dry_run = on;
        self
    }

    /// Keep the search index current for handled files (SPEC §5.5).
    pub fn with_index(mut self, opts: IndexOptions) -> Self {
        self.index = Some(opts);
        self
    }

    /// Ask `ai` about files no rule matches.
    pub fn with_ai(mut self, ai: AiClassifier) -> Self {
        self.ai = Some(ai);
        self
    }

    pub fn ai(&self) -> Option<&AiClassifier> {
        self.ai.as_ref()
    }

    pub fn mover(&self) -> &Mover {
        &self.mover
    }

    /// Classify and act on one file, then update the index if enabled.
    /// Index failures are logged and never turn a successful move into an
    /// error.
    pub fn handle(&self, path: &Path, batch_id: &str) -> Result<Action, EngineError> {
        let action = self.decide_and_act(path, batch_id)?;
        if let Some(opts) = &self.index {
            reindex_for(self.mover.store(), &action, opts);
        }
        Ok(action)
    }

    fn decide_and_act(&self, path: &Path, batch_id: &str) -> Result<Action, EngineError> {
        let skipped = |reason| {
            Ok(Action::Skipped {
                path: path.to_path_buf(),
                reason,
            })
        };
        if self.filter.is_ignored(path) {
            return skipped("ignored by ignore_globs");
        }
        let store = self.mover.store();
        if store.is_ignored(path)? {
            return skipped("permanently ignored");
        }
        if self.placed_by_us(path)? {
            return skipped("placed here by Tidy; left in place");
        }
        let facts = FileFacts::from_path(path).map_err(|source| EngineError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let Some(c) = self.rules.classify_in(&facts, self.anchors.for_path(path)) else {
            return self.ai_step(&facts, batch_id);
        };
        if self.dry_run {
            return Ok(Action::WouldMove {
                path: path.to_path_buf(),
                rule: c.rule_name,
                dest_dir: c.dest_dir,
                confirm: c.confirm,
            });
        }
        if self.was_undone(path)? {
            return skipped("restored by undo; left in place");
        }

        if c.confirm {
            store.upsert_pending(path, Some(&c.dest_dir), Some(&c.rule_name))?;
            return Ok(Action::Pending {
                path: path.to_path_buf(),
                rule: c.rule_name,
                suggested: c.dest_dir,
                source: PendingSource::Rule,
                confidence: None,
                reason: None,
            });
        }
        self.move_to(
            path,
            c.dest_dir,
            batch_id,
            &c.rule_name,
            DecidedBy::Rule,
            None,
        )
    }

    /// A file the user brought back with `tidy undo` stays where it is
    /// (works across processes because it is checked in the DB).
    fn was_undone(&self, path: &Path) -> Result<bool, EngineError> {
        let undone = self.mover.store().undone_hashes(path)?;
        Ok(!undone.is_empty() && undone.contains(&hash_file(path)?))
    }

    /// Tidy itself moved this exact content here (an active move). A polled
    /// NAS folder sees such files minutes later, long after the watcher's
    /// "expected move" token has expired, or from another process; they
    /// are not classified again (no second AI call, no ping-pong).
    fn placed_by_us(&self, path: &Path) -> Result<bool, EngineError> {
        let Some(placed) = self.mover.store().placed_hash(path)? else {
            return Ok(false);
        };
        Ok(hash_file(path)? == placed)
    }

    /// No rule matched: ask the AI (if any). Failures fall back to leaving
    /// the file alone — never an error for the caller (CLAUDE.md rule 4).
    fn ai_step(&self, facts: &FileFacts, batch_id: &str) -> Result<Action, EngineError> {
        let path = &facts.path;
        let Some(ai) = &self.ai else {
            return Ok(Action::NoRule(path.clone()));
        };
        let store = self.mover.store();
        // Already waiting for the user's answer to an AI suggestion: don't
        // pay for the same question again on every save.
        if let Some(p) = store.get_pending_by_path(path)? {
            if p.source == PendingSource::Ai {
                return Ok(Action::Skipped {
                    path: path.clone(),
                    reason: "already waiting for confirmation",
                });
            }
        }
        if !self.dry_run && self.was_undone(path)? {
            return Ok(Action::Skipped {
                path: path.clone(),
                reason: "restored by undo; left in place",
            });
        }
        let s = match ai.suggest(store, facts) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), "AI suggestion failed: {e}");
                return Ok(Action::AiFailed {
                    path: path.clone(),
                    error: e.to_string(),
                });
            }
        };
        let auto = ai.auto_allowed(&s);
        if self.dry_run {
            return Ok(Action::WouldMove {
                path: path.clone(),
                rule: format!("AI {:.0}%", s.confidence * 100.0),
                dest_dir: s.dest_dir,
                confirm: !auto,
            });
        }
        if auto {
            return self.move_to(
                path,
                s.dest_dir,
                batch_id,
                "AI",
                DecidedBy::Ai,
                Some(s.confidence),
            );
        }
        store.upsert_pending_entry(&NewPending {
            path,
            suggested_dest: Some(&s.dest_dir),
            reason: Some(&s.reason),
            source: PendingSource::Ai,
            confidence: Some(s.confidence),
            suggested_name: s.suggested_rename.as_deref(),
        })?;
        Ok(Action::Pending {
            path: path.clone(),
            rule: "AI".into(),
            suggested: s.dest_dir,
            source: PendingSource::Ai,
            confidence: Some(s.confidence),
            reason: Some(s.reason),
        })
    }

    fn move_to(
        &self,
        path: &Path,
        dest_dir: PathBuf,
        batch_id: &str,
        rule: &str,
        by: DecidedBy,
        confidence: Option<f64>,
    ) -> Result<Action, EngineError> {
        let out = self.mover.move_file(&MoveRequest {
            src: path.to_path_buf(),
            dest_dir,
            batch_id: batch_id.to_string(),
            decided_by: by,
            rule_name: (by == DecidedBy::Rule).then(|| rule.to_string()),
            ai_confidence: confidence,
            new_name: None,
        })?;
        Ok(match out {
            MoveOutcome::Moved { log_id, src, dst } => Action::Moved {
                src,
                dst,
                rule: rule.to_string(),
                batch_id: batch_id.to_string(),
                log_id,
                by,
                confidence,
            },
            MoveOutcome::SkippedDuplicate { src, existing } => Action::Duplicate {
                src,
                existing,
                rule: rule.to_string(),
            },
            MoveOutcome::AlreadyInPlace(path) => Action::AlreadyInPlace {
                path,
                rule: rule.to_string(),
            },
        })
    }
}

/// Index side effects of an [`Action`]. Never fails: problems are logged.
pub fn reindex_for(store: &crate::store::Store, action: &Action, opts: &IndexOptions) {
    let indexer = Indexer::new(store);
    let res = match action {
        Action::Moved { src, dst, .. } => indexer.moved(src, dst, opts).map(drop),
        Action::Pending { path, .. }
        | Action::Duplicate { src: path, .. }
        | Action::AlreadyInPlace { path, .. }
        | Action::NoRule(path)
        | Action::AiFailed { path, .. } => indexer.index_file(path, opts).map(drop),
        Action::WouldMove { .. } | Action::Skipped { .. } => Ok(()),
    };
    if let Err(e) = res {
        tracing::warn!("index update failed: {e}");
    }
}

/// Index side effects of an undo: restored files move back in the index.
pub fn reindex_undo(
    store: &crate::store::Store,
    report: &crate::mover::UndoReport,
    opts: &IndexOptions,
) {
    let indexer = Indexer::new(store);
    for (from, to) in &report.restored {
        if let Err(e) = indexer.moved(from, to, opts) {
            tracing::warn!("index update after undo failed: {e}");
        }
    }
}
