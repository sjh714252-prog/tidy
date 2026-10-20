//! Engine: one file in → classify → (confirm? pending : move). Shared by
//! `tidy watch`, `--process-existing` and the Tauri app.
//!
//! Pipeline (SPEC §5.2): rules → (no rule) AI → (no AI, AI failure or low
//! confidence) ask the user. Without AI the engine behaves exactly as in M1:
//! files no rule matches are left alone.
//!
//! (M9, SPEC §14.2) Before the rules, mode A sends personal files to the
//! vault folder; a rule with `encrypt = "pii" | "always"` locks its file
//! into a zip at its destination (mode B). With `approval = "confirm"`
//! (default) — or when no vault password is set yet — encryptions wait in
//! the queue instead; a file is never moved unencrypted in their place.
//!
//! (M10, SPEC §14.3) With `[curate] enabled`, files no rule takes (or, with
//! `prefer`, every file before the rules) are placed by work context —
//! `{entity}/{yyyymmdd}_{issue}/{doc_kind}` — instead of the classic AI
//! step. Only a confident answer into a folder that already exists moves on
//! its own; a new customer, issue or topic is asked in the confirm popup.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::classify::{AiClassifier, Anchors, Encrypt, FileFacts, RuleSet};
use crate::config::Approval;
use crate::index::{IndexOptions, Indexer};
use crate::mover::{hash_file, MoveOutcome, MoveRequest, Mover, MoverError};
use crate::pii::PiiFinding;
use crate::security::SecurityPolicy;
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
        /// (M9) `dst` is an encrypted zip of the file.
        encrypted: bool,
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
        /// (M9) Approving encrypts the file.
        encrypt: bool,
        /// (M9) What the detector found.
        pii: Option<PiiFinding>,
    },
    /// `--dry-run`: what would happen.
    WouldMove {
        path: PathBuf,
        rule: String,
        dest_dir: PathBuf,
        confirm: bool,
        /// (M9) It would be locked into a zip.
        encrypt: bool,
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
    /// (M9) Personal-information detection and vault settings.
    security: SecurityPolicy,
    /// (M10) Work-context placement (needs `ai`).
    curator: Option<std::sync::Arc<crate::curate::Curator>>,
}

/// Rule label of work-context moves (log, activity, queue).
pub const CONTEXT_RULE: &str = "업무 맥락";

/// Label of mode A moves (rule name in the log and the queue).
pub const VAULT_RULE: &str = "개인정보 보관";

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
            security: SecurityPolicy::default(),
            curator: None,
        }
    }

    /// (M10) Place files by work context (used only together with an AI).
    pub fn with_curator(mut self, c: std::sync::Arc<crate::curate::Curator>) -> Self {
        self.curator = Some(c);
        self
    }

    /// (M9) Detection, vault folder and approval settings.
    pub fn with_security(mut self, s: SecurityPolicy) -> Self {
        self.security = s;
        self
    }

    pub fn security(&self) -> &SecurityPolicy {
        &self.security
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

        // (M9) Mode A: personal files go to the vault before any rule.
        let mut finding: Option<PiiFinding> = None;
        if let Some(vault) = self.security.vault_dir.clone() {
            if path.starts_with(&vault) {
                return skipped("in the vault folder; left in place");
            }
            if self.security.detector.enabled() {
                let f = self.judge(&facts);
                finding = Some(f);
                if f.is_pii() {
                    if self.dry_run {
                        return Ok(Action::WouldMove {
                            path: path.to_path_buf(),
                            rule: VAULT_RULE.into(),
                            dest_dir: vault,
                            confirm: self.security.approval == Approval::Confirm,
                            encrypt: true,
                        });
                    }
                    if self.was_undone(path)? {
                        return skipped("restored by undo; left in place");
                    }
                    return self.secure(path, vault, VAULT_RULE, Some(f), false, batch_id);
                }
            }
        }

        let prefer = self.curator.as_ref().is_some_and(|c| c.cfg().prefer);
        if prefer {
            if let Some(a) = self.curate_step(&facts, batch_id)? {
                return Ok(a);
            }
        }
        let Some(c) = self.rules.classify_in(&facts, self.anchors.for_path(path)) else {
            if self.curator.is_some() && self.ai.is_some() {
                if prefer {
                    return Ok(Action::NoRule(path.to_path_buf()));
                }
                return Ok(self
                    .curate_step(&facts, batch_id)?
                    .unwrap_or_else(|| Action::NoRule(path.to_path_buf())));
            }
            return self.ai_step(&facts, batch_id);
        };
        let encrypt = match c.encrypt {
            Encrypt::Never => false,
            Encrypt::Always => true,
            Encrypt::Pii => {
                let f = match finding {
                    Some(f) => f,
                    None => self.judge(&facts),
                };
                finding = Some(f);
                f.is_pii()
            }
        };
        if self.dry_run {
            return Ok(Action::WouldMove {
                path: path.to_path_buf(),
                rule: c.rule_name,
                dest_dir: c.dest_dir,
                confirm: c.confirm || (encrypt && self.security.approval == Approval::Confirm),
                encrypt,
            });
        }
        if self.was_undone(path)? {
            return skipped("restored by undo; left in place");
        }
        if encrypt {
            let shown = finding.filter(PiiFinding::is_pii);
            return self.secure(path, c.dest_dir, &c.rule_name, shown, c.confirm, batch_id);
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
                encrypt: false,
                pii: None,
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

    /// (M9) Personal-information verdict: the index row's when it is
    /// current, otherwise read the text now. With `ai_check`, an AI that
    /// answers "not personal" clears a regex hit; AI failures keep it.
    pub fn judge(&self, facts: &FileFacts) -> PiiFinding {
        let sec = &self.security;
        let store = self.mover.store();
        let mtime = facts
            .modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut f = match store.fresh_pii(&facts.path, facts.size, mtime) {
            Ok(Some(f)) if f.status != crate::pii::PiiStatus::Unknown => f,
            _ => sec
                .detector
                .judge_path(&facts.path, sec.max_bytes, sec.max_chars),
        };
        if f.is_pii() && sec.ai_check {
            if let Some(ai) = &self.ai {
                match ai.confirm_pii(store, facts, &f) {
                    Ok(true) => {}
                    Ok(false) => {
                        f.status = crate::pii::PiiStatus::Clean;
                        f.flags = 0;
                    }
                    Err(e) => tracing::warn!("AI personal-information check failed: {e}"),
                }
            }
        }
        f
    }

    /// (M9) Encrypt `path` into `dest_dir`, or queue it when approval is
    /// needed (`approval = confirm`, the rule asks, or no password yet).
    fn secure(
        &self,
        path: &Path,
        dest_dir: PathBuf,
        rule: &str,
        pii: Option<PiiFinding>,
        rule_confirm: bool,
        batch_id: &str,
    ) -> Result<Action, EngineError> {
        let store = self.mover.store();
        let has_password = self
            .mover
            .vault_keys()
            .map(|k| k.has_current())
            .unwrap_or(false);
        if rule_confirm || self.security.approval == Approval::Confirm || !has_password {
            store.upsert_pending_entry(&NewPending {
                path,
                suggested_dest: Some(&dest_dir),
                reason: Some(rule),
                source: PendingSource::Rule,
                confidence: None,
                suggested_name: None,
                encrypt: true,
                pii: pii.as_ref(),
            })?;
            return Ok(Action::Pending {
                path: path.to_path_buf(),
                rule: rule.to_string(),
                suggested: dest_dir,
                source: PendingSource::Rule,
                confidence: None,
                reason: None,
                encrypt: true,
                pii,
            });
        }
        if let Some(opts) = &self.index {
            prepare_vault_index(store, path, opts);
        }
        self.move_with(path, dest_dir, batch_id, rule, DecidedBy::Rule, None, true)
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

    /// (M10) Work-context placement of one file. `Ok(None)`: no confident
    /// answer (the caller falls back to rules / leaves the file).
    fn curate_step(
        &self,
        facts: &FileFacts,
        batch_id: &str,
    ) -> Result<Option<Action>, EngineError> {
        let (Some(cur), Some(ai)) = (&self.curator, &self.ai) else {
            return Ok(None);
        };
        let path = &facts.path;
        let store = self.mover.store();
        if let Some(p) = store.get_pending_by_path(path)? {
            if p.source == PendingSource::Ai {
                return Ok(Some(Action::Skipped {
                    path: path.clone(),
                    reason: "already waiting for confirmation",
                }));
            }
        }
        if !self.dry_run && self.was_undone(path)? {
            return Ok(Some(Action::Skipped {
                path: path.clone(),
                reason: "restored by undo; left in place",
            }));
        }
        let input = cur
            .input(path, Some(store))
            .map_err(|source| EngineError::Io {
                path: path.clone(),
                source,
            })?;
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        let anchor = self.anchors.for_path(path).unwrap_or(parent);
        let placed = match cur.place(ai, store, &input, anchor) {
            Ok(Some(p)) if cur.is_classified(&p.curation) => p,
            Ok(_) => return Ok(None),
            Err(e) => {
                tracing::warn!(path = %path.display(), "work-context analysis failed: {e}");
                return Ok(Some(Action::AiFailed {
                    path: path.clone(),
                    error: e.to_string(),
                }));
            }
        };
        let c = &placed.curation;
        let auto = placed.known_folder && c.confidence >= ai.auto_threshold();
        if self.dry_run {
            return Ok(Some(Action::WouldMove {
                path: path.clone(),
                rule: CONTEXT_RULE.into(),
                dest_dir: placed.dest_dir,
                confirm: !auto,
                encrypt: false,
            }));
        }
        let ymd = placed.date.format("%Y%m%d");
        if auto {
            let a = self.move_with(
                path,
                placed.dest_dir.clone(),
                batch_id,
                CONTEXT_RULE,
                DecidedBy::Ai,
                Some(c.confidence),
                false,
            )?;
            if let Action::Moved { dst, .. } = &a {
                let mut kept = c.clone();
                kept.date = Some(placed.date);
                if let Err(e) = store.put_curation(&input.key, dst, &kept, None) {
                    tracing::warn!("curation cache update failed: {e}");
                }
            }
            return Ok(Some(a));
        }
        let question = match (
            c.entity.as_deref(),
            placed.known_entity,
            placed.known_folder,
        ) {
            (Some(e), false, _) => {
                format!("새 고객사 '{e}'의 '{}'({ymd}) 폴더를 만들까요?", c.issue)
            }
            (Some(e), true, false) => {
                format!("{e}의 새 이슈 '{}'({ymd})으로 폴더를 만들까요?", c.issue)
            }
            (None, _, false) => format!(
                "새 주제 '{}' 폴더를 만들까요?",
                c.topic.as_deref().unwrap_or("미분류")
            ),
            _ => format!("확신도 {:.0}%", c.confidence * 100.0),
        };
        let reason = format!("{question} — {}", c.reason);
        store.upsert_pending_entry(&NewPending {
            path,
            suggested_dest: Some(&placed.dest_dir),
            reason: Some(&reason),
            source: PendingSource::Ai,
            confidence: Some(c.confidence),
            suggested_name: None,
            encrypt: false,
            pii: None,
        })?;
        Ok(Some(Action::Pending {
            path: path.clone(),
            rule: CONTEXT_RULE.into(),
            suggested: placed.dest_dir,
            source: PendingSource::Ai,
            confidence: Some(c.confidence),
            reason: Some(reason),
            encrypt: false,
            pii: None,
        }))
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
                encrypt: false,
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
            encrypt: false,
            pii: None,
        })?;
        Ok(Action::Pending {
            path: path.clone(),
            rule: "AI".into(),
            suggested: s.dest_dir,
            source: PendingSource::Ai,
            confidence: Some(s.confidence),
            reason: Some(s.reason),
            encrypt: false,
            pii: None,
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
        self.move_with(path, dest_dir, batch_id, rule, by, confidence, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn move_with(
        &self,
        path: &Path,
        dest_dir: PathBuf,
        batch_id: &str,
        rule: &str,
        by: DecidedBy,
        confidence: Option<f64>,
        encrypt: bool,
    ) -> Result<Action, EngineError> {
        let out = self.mover.move_file(&MoveRequest {
            src: path.to_path_buf(),
            dest_dir,
            batch_id: batch_id.to_string(),
            decided_by: by,
            rule_name: (by == DecidedBy::Rule).then(|| rule.to_string()),
            ai_confidence: confidence,
            new_name: None,
            encrypt,
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
                encrypted: encrypt,
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
        Action::Moved {
            src,
            dst,
            encrypted: true,
            ..
        } => indexer.vaulted(src, dst, opts).map(drop),
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

/// (M9) Index a file right before it is encrypted, so its text (search,
/// 🔒 results) survives in the row that will point at the zip.
pub fn prepare_vault_index(store: &crate::store::Store, path: &Path, opts: &IndexOptions) {
    if let Err(e) = Indexer::new(store).index_file(path, opts) {
        tracing::warn!("index before encrypt failed: {e}");
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
