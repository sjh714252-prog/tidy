//! Engine: one file in → classify → (confirm? pending : move). Shared by
//! `tidy watch`, `--process-existing` and (later) the Tauri app.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::classify::{FileFacts, RuleSet};
use crate::mover::{hash_file, MoveOutcome, MoveRequest, Mover, MoverError};
use crate::store::{DecidedBy, StoreError};
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Moved {
        src: PathBuf,
        dst: PathBuf,
        rule: String,
        batch_id: String,
        log_id: i64,
    },
    /// Rule has `confirm = true`: queued in `pending`, not moved.
    Pending {
        path: PathBuf,
        rule: String,
        suggested: PathBuf,
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
        }
    }

    pub fn dry_run(mut self, on: bool) -> Self {
        self.dry_run = on;
        self
    }

    pub fn mover(&self) -> &Mover {
        &self.mover
    }

    pub fn handle(&self, path: &Path, batch_id: &str) -> Result<Action, EngineError> {
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
        let facts = FileFacts::from_path(path).map_err(|source| EngineError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let Some(c) = self.rules.classify(&facts) else {
            return Ok(Action::NoRule(path.to_path_buf()));
        };
        if self.dry_run {
            return Ok(Action::WouldMove {
                path: path.to_path_buf(),
                rule: c.rule_name,
                dest_dir: c.dest_dir,
                confirm: c.confirm,
            });
        }

        // A file the user brought back with `tidy undo` stays where it is
        // (works across processes because it is checked in the DB).
        let undone = store.undone_hashes(path)?;
        if !undone.is_empty() && undone.contains(&hash_file(path)?) {
            return skipped("restored by undo; left in place");
        }

        if c.confirm {
            store.upsert_pending(path, Some(&c.dest_dir), Some(&c.rule_name))?;
            return Ok(Action::Pending {
                path: path.to_path_buf(),
                rule: c.rule_name,
                suggested: c.dest_dir,
            });
        }

        let out = self.mover.move_file(&MoveRequest {
            src: path.to_path_buf(),
            dest_dir: c.dest_dir,
            batch_id: batch_id.to_string(),
            decided_by: DecidedBy::Rule,
            rule_name: Some(c.rule_name.clone()),
            ai_confidence: None,
        })?;
        Ok(match out {
            MoveOutcome::Moved { log_id, src, dst } => Action::Moved {
                src,
                dst,
                rule: c.rule_name,
                batch_id: batch_id.to_string(),
                log_id,
            },
            MoveOutcome::SkippedDuplicate { src, existing } => Action::Duplicate {
                src,
                existing,
                rule: c.rule_name,
            },
            MoveOutcome::AlreadyInPlace(path) => Action::AlreadyInPlace {
                path,
                rule: c.rule_name,
            },
        })
    }
}
