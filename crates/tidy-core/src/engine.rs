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
    /// (0.18.0, SPEC §14.24) Reading what a file is from its name and
    /// contents (`[curate] name_sense`): the dictionary it matches against.
    sense: Option<std::sync::Arc<SenseDict>>,
    /// (0.19.0, SPEC §14.29) Watching: folders the user made are moved
    /// whole (asked), and files just made to write in are never moved on
    /// their own. `None` (`tidy classify`, tests): file by file as before.
    guards: Option<WatchGuards>,
    /// (0.19.0) Files seen empty while watching ("새로 만들기"): once they
    /// have content they are asked about, not moved.
    born_empty: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    /// (0.19.0) Why the file being handled must be asked about instead of
    /// moved (set for one [`Engine::handle`] call).
    ask: std::sync::Mutex<Option<&'static str>>,
}

/// (0.19.0, SPEC §14.29) What watching needs to judge a folder the user
/// made: the same checks as the work-context preview (excluded, protected,
/// rule destinations and organize roots, Tidy-made, still changing).
pub struct WatchGuards {
    /// Judges folders ([`crate::curate::Curator::judge_folder`]); also the
    /// dictionary for customer names. Needs no AI.
    pub scanner: std::sync::Arc<crate::curate::Curator>,
    /// `[report] protected`.
    pub protected: Vec<PathBuf>,
    /// Every "모으기" root (organized results, never folder items).
    pub organize_roots: Vec<PathBuf>,
}

/// (0.19.0) [`Engine::propose_folder`]'s answer.
#[derive(Debug)]
pub enum FolderStep {
    /// Still being filled / copied: ask again later.
    Wait,
    /// Judged: queued (`Some(Pending)`), or left alone (`None`).
    Done(Option<Action>),
}

/// Where [`Engine::folder_dest`] puts a folder, and why.
struct FolderPick {
    parent: PathBuf,
    why: String,
    source: PendingSource,
    confidence: Option<f64>,
}

/// (0.19.0) Skip reason of a file inside a folder the user made: the
/// watch loop then proposes the folder as a whole ([`Engine::bundle_unit`]).
pub const IN_FOLDER: &str = "inside a folder; the folder is moved as one";
/// (0.19.0) Skip reason of an empty file (just made, nothing written yet).
pub const EMPTY_FILE: &str = "empty file (just made?); left until it has content";
/// (0.19.0) Rule label of a whole-folder proposal.
pub const FOLDER_RULE: &str = "폴더째 옮기기";
/// (0.19.0) Why a just-made file is asked about.
pub const NEW_FILE_ASK: &str = "새로 만든 파일이라 옮기기 전에 묻습니다";

/// (0.18.0) The dictionary [`Engine::sense_step`] reads, re-read when its
/// file changes (an "어느 고객사?" answer adds an alias while watching).
struct SenseDict {
    path: Option<PathBuf>,
    cur: std::sync::Mutex<(Option<std::time::SystemTime>, crate::curate::ContextDict)>,
}

impl SenseDict {
    fn get(&self) -> crate::curate::ContextDict {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = &self.path {
            let m = std::fs::metadata(p).and_then(|m| m.modified()).ok();
            if m != g.0 {
                if let Ok(d) = crate::curate::ContextDict::load(p) {
                    *g = (m, d);
                }
            }
        }
        g.1.clone()
    }
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
            sense: None,
            guards: None,
            born_empty: Default::default(),
            ask: Default::default(),
        }
    }

    /// (0.19.0, SPEC §14.29) Watch-time protections: user folders whole,
    /// just-made files asked about.
    pub fn with_guards(mut self, g: WatchGuards) -> Self {
        self.guards = Some(g);
        self
    }

    /// (0.18.0, SPEC §14.24) Place log bundles, manuals and runtimes by
    /// what their names and contents say, before the rules (no AI needed).
    pub fn with_sense(mut self, dict: crate::curate::ContextDict) -> Self {
        self.sense = Some(std::sync::Arc::new(SenseDict {
            path: None,
            cur: std::sync::Mutex::new((None, dict)),
        }));
        self
    }

    /// [`Self::with_sense`] from the dictionary file, re-read when it changes.
    pub fn with_sense_file(mut self, path: PathBuf) -> Self {
        self.sense = Some(std::sync::Arc::new(SenseDict {
            path: Some(path),
            cur: std::sync::Mutex::new((None, crate::curate::ContextDict::default())),
        }));
        self
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
        *self.ask.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let action = self.decide_and_act(path, batch_id);
        *self.ask.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let action = action?;
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
        if self.guards.is_some() {
            // (0.19.0, SPEC §14.29) A folder the user made holds one piece
            // of work: never pulled apart file by file.
            if self.bundle_unit(path).is_some() {
                return skipped(IN_FOLDER);
            }
            if let Some(why) = self.fresh(path) {
                if why == EMPTY_FILE {
                    return skipped(EMPTY_FILE);
                }
                *self.ask.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
            }
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
                    let ask = self.asking().is_some();
                    return self.secure(path, vault, VAULT_RULE, Some(f), ask, batch_id);
                }
            }
        }

        let prefer = self.curator.as_ref().is_some_and(|c| c.cfg().prefer);
        let hint = self.curator.as_ref().and_then(|c| {
            path.file_name()
                .and_then(|n| c.customer_hint(&n.to_string_lossy(), Some(self.mover.store())))
        });
        // (0.18.0, SPEC §14.24) A log bundle, a manual, a runtime: its own
        // place, decided from the name / contents before the AI and rules.
        if let Some(a) = self.sense_step(&facts, hint.as_deref(), batch_id)? {
            return Ok(a);
        }
        if prefer {
            if let Some(a) = self.curate_step(&facts, batch_id)? {
                return Ok(a);
            }
        } else if self.ai.is_some() && hint.is_some() {
            // (0.13.0, SPEC §14.19) The name says whose work this is: the
            // customer's folder comes before the type rules (`문서/…`).
            match self.curate_step(&facts, batch_id)? {
                Some(Action::AiFailed { .. }) | None => {}
                Some(a) => return Ok(a),
            }
        }
        // (0.16.0, SPEC §14.22) Still a customer's file (no AI, AI unsure
        // or failed): into that customer's folder by kind, never the type
        // rules' folders where it could not be found by customer.
        if let Some(c) = &hint {
            if let Some(a) = self.customer_kind_step(&facts, c, batch_id)? {
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
            let confirm = c.confirm || self.asking().is_some();
            return self.secure(path, c.dest_dir, &c.rule_name, shown, confirm, batch_id);
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
    /// (0.16.0, SPEC §14.22) A file whose name shows `customer`, placed
    /// without the AI: `{customer}/{kind}` under its organize root. Only
    /// files directly in a watch folder (a sub-folder is organized whole).
    /// The customer folder exists → moved; a new customer folder → asked
    /// in 확인 대기.
    fn customer_kind_step(
        &self,
        facts: &FileFacts,
        customer: &str,
        batch_id: &str,
    ) -> Result<Option<Action>, EngineError> {
        let path = &facts.path;
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        let Some(root) = self.anchors.watch_root_for(path) else {
            return Ok(None);
        };
        if parent != root {
            return Ok(None);
        }
        let store = self.mover.store();
        if let Some(p) = store.get_pending_by_path(path)? {
            if p.source != PendingSource::Rule {
                return Ok(None);
            }
        }
        let anchor = self.anchors.for_path(path).unwrap_or(parent);
        let kind = crate::curate::kinds::kind_of(self.curator.as_ref().map(|c| c.dict()), path);
        let (dest, known) = crate::curate::kinds::customer_kind_dir(anchor, customer, &kind);
        let rule = format!("고객사 폴더 · {kind}");
        if self.dry_run {
            return Ok(Some(Action::WouldMove {
                path: path.clone(),
                rule,
                dest_dir: dest,
                confirm: !known,
                encrypt: false,
            }));
        }
        if self.was_undone(path)? {
            return Ok(Some(Action::Skipped {
                path: path.clone(),
                reason: "restored by undo; left in place",
            }));
        }
        if !known {
            store.upsert_pending(path, Some(&dest), Some(&rule))?;
            return Ok(Some(Action::Pending {
                path: path.clone(),
                rule,
                suggested: dest,
                source: PendingSource::Rule,
                confidence: None,
                reason: Some(format!(
                    "파일 이름에 고객사 '{customer}'가 보입니다 — 새 고객사 폴더를 만들까요?"
                )),
                encrypt: false,
                pii: None,
            }));
        }
        Ok(Some(self.move_with(
            path,
            dest,
            batch_id,
            &rule,
            DecidedBy::Rule,
            None,
            false,
        )?))
    }

    /// (0.18.0, SPEC §14.24) See [`crate::curate::sense`]. Only files right
    /// in a watch folder (a sub-folder is organized whole).
    fn sense_step(
        &self,
        facts: &FileFacts,
        hint: Option<&str>,
        batch_id: &str,
    ) -> Result<Option<Action>, EngineError> {
        use crate::curate::sense;
        let Some(sd) = &self.sense else {
            return Ok(None);
        };
        let path = &facts.path;
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        if self.anchors.watch_root_for(path) != Some(parent) {
            return Ok(None);
        }
        let store = self.mover.store();
        if let Some(p) = store.get_pending_by_path(path)? {
            if p.reason.as_deref() == Some(sense::ASK_CUSTOMER_RULE) {
                // The question stays; the rules must not take the file meanwhile.
                return Ok(Some(Action::Skipped {
                    path: path.clone(),
                    reason: "already waiting for confirmation",
                }));
            }
            if p.source != PendingSource::Rule {
                return Ok(None);
            }
        }
        let anchor = self.anchors.for_path(path).unwrap_or(parent).to_path_buf();
        let dict = sd.get();
        let clues = sense::clues_for(Some(&dict), Some(store), &anchor);
        let name = facts.name.clone();
        let entries = sense::zip_entries(path, 2000);
        let customer = hint
            .map(str::to_string)
            .or_else(|| clues.customer_in(&name))
            .or_else(|| clues.customer_in_list(&entries));
        let Some(shape) = sense::shape(path, &clues, customer.as_deref(), Some(&entries)) else {
            return Ok(None);
        };
        let what = shape.describe();
        let (dest, known, ask) = match sense::dest(&anchor, &shape, customer.as_deref()) {
            Some((d, k)) => (d, k, false),
            None if sense::ask_customer(&shape, path) => {
                let sub = sense::customer_sub(&shape).unwrap_or_default();
                (anchor.join(sub), true, true)
            }
            None => return Ok(None),
        };
        let rule = if ask {
            sense::ASK_CUSTOMER_RULE.to_string()
        } else {
            match &customer {
                Some(c)
                    if matches!(
                        shape,
                        sense::Shape::Log { .. } | sense::Shape::Manual { .. }
                    ) =>
                {
                    format!("{c} · {what}")
                }
                _ => what.clone(),
            }
        };
        if self.dry_run {
            return Ok(Some(Action::WouldMove {
                path: path.clone(),
                rule,
                dest_dir: dest,
                confirm: ask || !known,
                encrypt: false,
            }));
        }
        if self.was_undone(path)? {
            return Ok(Some(Action::Skipped {
                path: path.clone(),
                reason: "restored by undo; left in place",
            }));
        }
        if ask || !known {
            store.upsert_pending(path, Some(&dest), Some(&rule))?;
            return Ok(Some(Action::Pending {
                path: path.clone(),
                rule,
                suggested: dest,
                source: PendingSource::Rule,
                confidence: None,
                reason: Some(if ask {
                    format!(
                        "{what} — 어느 고객사 자료인가요? 고르면 그 고객사 폴더의 로그로 옮깁니다"
                    )
                } else {
                    format!(
                        "{what}: 이름에 고객사 '{}'가 보입니다 — 새 고객사 폴더를 만들까요?",
                        customer.as_deref().unwrap_or("")
                    )
                }),
                encrypt: false,
                pii: None,
            }));
        }
        Ok(Some(self.move_with(
            path,
            dest,
            batch_id,
            &rule,
            DecidedBy::Rule,
            None,
            false,
        )?))
    }

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
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        // (M12, SPEC §14.7) Only files directly in a watch folder are placed
        // by work context while watching. A file inside a sub-folder belongs
        // to that folder, which is organized as a whole from the home tab
        // (preview + approval) — never scattered file by file.
        let root = self.anchors.watch_root_for(path);
        if root.is_none_or(|r| parent != r) {
            return Ok(None);
        }
        let input = cur
            .input_in(path, root.unwrap_or(parent), Some(store))
            .map_err(|source| EngineError::Io {
                path: path.clone(),
                source,
            })?;
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
            // (0.12.0) The issue folder exists but a level below it (kind)
            // does not, or the AI is unsure: say it is an existing folder.
            _ => format!(
                "이미 있는 폴더에 넣을까요? (확신도 {:.0}%)",
                c.confidence * 100.0
            ),
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
        // (0.12.5) Like work context (M12): a file inside a sub-folder of a
        // watch folder belongs to that folder (a customer's logs, a source
        // tree) — the AI never pulls it out on its own. Rules still apply.
        if self.in_sub_folder(path) {
            return Ok(Action::NoRule(path.clone()));
        }
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

    fn asking(&self) -> Option<&'static str> {
        *self.ask.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// (0.19.0, SPEC §14.29) The folder directly in the watch folder that
    /// holds `path` when `path` is inside one (`바탕 화면/전송자격 …/a.pdf`
    /// → `바탕 화면/전송자격 …`). `None` for files right in the watch folder,
    /// outside every watch folder, or when watch guards are off.
    pub fn bundle_unit(&self, path: &Path) -> Option<PathBuf> {
        self.guards.as_ref()?;
        let root = self.anchors.watch_root_for(path)?;
        let rel = path.strip_prefix(root).ok()?;
        let mut parts = rel.components();
        let first = parts.next()?;
        parts.next()?;
        Some(root.join(first))
    }

    /// (0.19.0, SPEC §14.29) Was `path` just made by the user to write in?
    /// [`EMPTY_FILE`]: empty, leave it. [`NEW_FILE_ASK`]: has content now,
    /// ask before moving. `None`: an ordinary file.
    fn fresh(&self, path: &Path) -> Option<&'static str> {
        let meta = std::fs::metadata(path).ok()?;
        let mut born = self.born_empty.lock().unwrap_or_else(|e| e.into_inner());
        if meta.len() == 0 {
            if born.len() > 10_000 {
                born.clear();
            }
            born.insert(path.to_path_buf());
            return Some(EMPTY_FILE);
        }
        let name = path.file_name()?.to_string_lossy();
        let local = self
            .anchors
            .watch_root_for(path)
            .is_some_and(|_| !crate::paths::looks_like_network(path));
        if born.contains(path)
            || crate::fresh::is_blank_template(&name)
            || (local && crate::fresh::authored_recently(path, &meta, std::time::SystemTime::now()))
        {
            return Some(NEW_FILE_ASK);
        }
        None
    }

    /// (0.19.0, SPEC §14.29) A folder the user made in a watch folder and
    /// filled, once nothing inside changed for `[curate] folder_settle_secs`:
    /// one 확인 대기 item "폴더째 옮기기" (never moved on its own), with a
    /// suggested place — a same-named folder there already, the work
    /// context (AI), a customer name in the folder / file names, the rule
    /// all its files agree on, else `기타`. Its files' own queue items go.
    pub fn propose_folder(&self, unit: &Path) -> Result<FolderStep, EngineError> {
        use crate::curate::FolderVerdict;
        let Some(g) = &self.guards else {
            return Ok(FolderStep::Done(None));
        };
        let Some(root) = self.anchors.watch_root_for(unit).map(Path::to_path_buf) else {
            return Ok(FolderStep::Done(None));
        };
        if !std::fs::symlink_metadata(unit).is_ok_and(|m| m.is_dir()) {
            return Ok(FolderStep::Done(None)); // gone, or a link
        }
        let store = self.mover.store();
        if store.is_ignored(unit)? {
            return Ok(FolderStep::Done(None));
        }
        let anchor = self.anchors.for_path(unit).unwrap_or(&root).to_path_buf();
        let mut skip = self.rules.dest_roots_for(std::slice::from_ref(&anchor));
        skip.extend(g.organize_roots.iter().cloned());
        let u = match g.scanner.judge_folder(
            unit,
            &root,
            &anchor,
            &self.filter,
            &g.protected,
            &skip,
            Some(store),
            std::time::SystemTime::now(),
        ) {
            FolderVerdict::Unit(u) => *u,
            FolderVerdict::Unsettled => return Ok(FolderStep::Wait),
            FolderVerdict::Kept(why) => {
                tracing::info!(path = %unit.display(), %why, "folder left as it is");
                return Ok(FolderStep::Done(None));
            }
            FolderVerdict::Leftover | FolderVerdict::Hidden => return Ok(FolderStep::Done(None)),
        };
        let pick = self.folder_dest(&u, &anchor)?;
        if self.dry_run {
            return Ok(FolderStep::Done(Some(Action::WouldMove {
                path: unit.to_path_buf(),
                rule: FOLDER_RULE.into(),
                dest_dir: pick.parent,
                confirm: true,
                encrypt: false,
            })));
        }
        for p in store.list_pending()? {
            if p.path != unit && p.path.starts_with(unit) {
                store.remove_pending_id(p.id)?;
            }
        }
        let reason = format!(
            "{FOLDER_RULE} — 직접 만든 폴더라 안의 파일 {}개를 흩지 않고 함께 옮깁니다. {}",
            u.files, pick.why
        );
        store.upsert_pending_entry(&NewPending {
            path: unit,
            suggested_dest: Some(&pick.parent),
            reason: Some(&reason),
            source: pick.source,
            confidence: pick.confidence,
            suggested_name: None,
            encrypt: false,
            pii: None,
        })?;
        Ok(FolderStep::Done(Some(Action::Pending {
            path: unit.to_path_buf(),
            rule: FOLDER_RULE.into(),
            suggested: pick.parent,
            source: pick.source,
            confidence: pick.confidence,
            reason: Some(reason),
            encrypt: false,
            pii: None,
        })))
    }

    /// Where a folder item would go (the folder it goes INTO, keeping its
    /// name) and why. Never fails on the AI: it falls back to names / rules.
    fn folder_dest(
        &self,
        u: &crate::curate::FolderUnit,
        anchor: &Path,
    ) -> Result<FolderPick, EngineError> {
        let store = self.mover.store();
        let rule_pick = |parent: PathBuf, why: String| FolderPick {
            parent,
            why,
            source: PendingSource::Rule,
            confidence: None,
        };
        // 1. A folder of that name is already in the organize root: join it.
        let same = anchor.join(&u.name);
        if same.is_dir() && same != u.path && !u.path.starts_with(&same) {
            return Ok(rule_pick(
                anchor.to_path_buf(),
                "정리 위치에 같은 이름의 폴더가 있어 그 안에 합칩니다.".into(),
            ));
        }
        // 2. The work context (AI), when it is on.
        if let (Some(cur), Some(ai)) = (&self.curator, &self.ai) {
            let input = crate::curate::CurateInput::from_unit(u.clone());
            match cur.place(ai, store, &input, anchor) {
                Ok(Some(p)) if cur.is_classified(&p.curation) => {
                    let new = cur.dest_for(&p.curation, p.date, anchor, Some(&u.name));
                    if let Some(parent) = new.parent().filter(|d| d.starts_with(anchor)) {
                        return Ok(FolderPick {
                            parent: parent.to_path_buf(),
                            why: format!("AI 판단: {}", p.curation.reason),
                            source: PendingSource::Ai,
                            confidence: Some(p.curation.confidence),
                        });
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(path = %u.path.display(), "folder placement by AI failed: {e}")
                }
            }
        }
        // 3. A customer's name in the folder's name, else in its files' names.
        if let Some(g) = &self.guards {
            let hint = std::iter::once(u.name.as_str())
                .chain(u.samples.iter().map(String::as_str))
                .find_map(|n| {
                    let n = n.rsplit(['/', '\\']).next().unwrap_or(n);
                    g.scanner.customer_hint(n, Some(store))
                });
            if let Some(c) = hint {
                let (dir, known) = crate::curate::kinds::customer_dir(anchor, &c);
                let why = if known {
                    format!("이름에 고객사 '{c}'가 보여 그 고객사 폴더로.")
                } else {
                    format!("이름에 고객사 '{c}'가 보입니다 — 새 고객사 폴더를 만듭니다.")
                };
                return Ok(rule_pick(dir, why));
            }
        }
        // 4. Every file inside matches the same rule: that rule's folder.
        let mut agreed: Option<(PathBuf, String)> = None;
        let mut all = true;
        for e in walkdir::WalkDir::new(&u.path)
            .min_depth(1)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .take(200)
        {
            let c = FileFacts::from_path(e.path())
                .ok()
                .and_then(|f| self.rules.classify_in(&f, Some(anchor)));
            match (c, &agreed) {
                (Some(c), None) => agreed = Some((c.dest_dir, c.rule_name)),
                (Some(c), Some((d, _))) if *d == c.dest_dir => {}
                _ => {
                    all = false;
                    break;
                }
            }
        }
        if let (true, Some((dir, rule))) = (all, agreed) {
            return Ok(rule_pick(
                dir,
                format!("안의 파일이 모두 '{rule}' 규칙에 맞아 그 폴더로."),
            ));
        }
        // 5. Unknown work: under `기타` (the layout of files without a customer).
        let other = self
            .guards
            .as_ref()
            .and_then(|g| {
                g.scanner
                    .cfg()
                    .other_template
                    .split(['/', '\\'])
                    .next()
                    .map(str::trim)
                    .filter(|f| !f.is_empty() && !f.contains('{'))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "기타".into());
        Ok(rule_pick(
            anchor.join(other),
            "어느 고객사·종류인지 몰라 기타 아래로 — [다른 곳…]으로 바꿀 수 있습니다.".into(),
        ))
    }

    /// `path` lies below a sub-folder of its watch folder (not directly in
    /// it). Files outside every watch folder (`tidy classify`) are not.
    pub fn in_sub_folder(&self, path: &Path) -> bool {
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        self.anchors
            .watch_root_for(path)
            .is_some_and(|r| parent != r)
    }

    /// (0.12.5) Drop AI suggestions waiting in the queue for files inside
    /// sub-folders of a watch folder — made by earlier versions that judged
    /// such files one by one (a customer folder's `tools/go/src/…/x.go` →
    /// 기타/개발/소스코드). Accepting them would scatter the folder; the
    /// folder is organized as a whole instead. Files are not touched.
    /// Returns the paths dropped.
    pub fn drop_scattering_pending(&self) -> Result<Vec<PathBuf>, EngineError> {
        let store = self.mover.store();
        let mut out = Vec::new();
        for p in store.list_pending()? {
            if p.source == PendingSource::Ai && self.in_sub_folder(&p.path) {
                store.remove_pending_id(p.id)?;
                out.push(p.path);
            }
        }
        Ok(out)
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
        if let Some(why) = self.asking() {
            // (0.19.0, SPEC §14.29) Just made by the user: asked, not moved.
            let source = if by == DecidedBy::Ai {
                PendingSource::Ai
            } else {
                PendingSource::Rule
            };
            let reason = match source {
                PendingSource::Rule => format!("{rule} · 새로 만든 파일"),
                PendingSource::Ai => format!("{why} — {rule}"),
            };
            self.mover.store().upsert_pending_entry(&NewPending {
                path,
                suggested_dest: Some(&dest_dir),
                reason: Some(&reason),
                source,
                confidence,
                suggested_name: None,
                encrypt,
                pii: None,
            })?;
            return Ok(Action::Pending {
                path: path.to_path_buf(),
                rule: rule.to_string(),
                suggested: dest_dir,
                source,
                confidence,
                reason: Some(why.to_string()),
                encrypt,
                pii: None,
            });
        }
        let out = self.mover.move_file(&MoveRequest {
            src: path.to_path_buf(),
            dest_dir,
            batch_id: batch_id.to_string(),
            decided_by: by,
            rule_name: (by == DecidedBy::Rule).then(|| rule.to_string()),
            ai_confidence: confidence,
            new_name: None,
            encrypt,
            on_duplicate: None,
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
    // (0.14.0) A re-made empty folder has nothing to index.
    for (from, to) in report.restored.iter().filter(|(_, to)| !to.is_dir()) {
        if let Err(e) = indexer.moved(from, to, opts) {
            tracing::warn!("index update after undo failed: {e}");
        }
    }
}
