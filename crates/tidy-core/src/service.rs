//! The long-running watch loop shared by `tidy watch` and the desktop app:
//! watcher events → [`Engine`] (rules → pending / move) → search index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::ai::AiAvailability;
use crate::classify::{AiClassifier, RuleError, RuleSet};
use crate::config::Config;
use crate::engine::{new_batch_id, Action, Engine, EngineError};
use crate::index::IndexOptions;
use crate::mover::{ExpectedMoves, Mover, ProgressFn, RecoveryReport, DEFAULT_RECOVERY_GRACE};
use crate::search::{EmbedReport, SemanticSearch};
use crate::store::{Store, StoreError};
use crate::watcher::{existing_files, RootState, WatchError, WatchOptions, Watcher};

/// How often a locked/busy file is retried before giving up.
pub const MAX_RETRIES: u32 = 5;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("no [[watch]] folders configured")]
    NoWatchFolders,
    #[error(transparent)]
    Rules(#[from] RuleError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Watch(#[from] WatchError),
}

#[derive(Clone, Default)]
pub struct WatchRunOptions {
    /// Rules file; default `cfg.classify.rules`.
    pub rules: Option<PathBuf>,
    pub process_existing: bool,
    pub dry_run: bool,
    /// Keep the search index current (default on in the app and CLI).
    pub index: bool,
    /// Use the AI step when `[ai]` is enabled and usable (key present…).
    pub ai: bool,
    /// Override watcher timings (tests: sub-second polling).
    pub watch_options: Option<WatchOptions>,
    /// Grace period before a 'moving' row counts as interrupted.
    pub recovery_grace: Option<Duration>,
    /// Live progress of long copy-verify moves (NAS). Called on the watch
    /// thread while the copy runs, so it must not block.
    pub progress: Option<ProgressFn>,
    /// (M9) Vault passwords to use instead of the OS keychain (tests).
    pub vault_keys: Option<crate::security::VaultKeys>,
}

impl std::fmt::Debug for WatchRunOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchRunOptions")
            .field("rules", &self.rules)
            .field("process_existing", &self.process_existing)
            .field("dry_run", &self.dry_run)
            .field("index", &self.index)
            .field("ai", &self.ai)
            .finish_non_exhaustive()
    }
}

/// Events reported to the caller (CLI prints them, the app emits them).
pub enum WatchEvent<'a> {
    /// Watching started on these canonical folders.
    Started(&'a [PathBuf]),
    /// AI step state at startup: `Ok("anthropic / claude-haiku-5-5")`, or
    /// why it is off (rules-only mode). Not sent when AI is not enabled.
    Ai(Result<String, String>),
    Handled(&'a Path, &'a Result<Action, EngineError>),
    /// Configuration problems worth showing (DB on a share…), at startup.
    Warning(String),
    /// Interrupted moves were resolved (startup, or a share came back).
    Recovered(&'a RecoveryReport),
    /// A watch folder went offline / came back / changed event mode.
    Root(&'a RootState),
    /// The AI became unavailable after repeated failures (with the last
    /// error) or answered again. Files skip the AI while it is unavailable
    /// and are offered to it again once it is back.
    AiHealth(AiAvailability, Option<String>),
}

/// Files the AI failed on, offered to it once more after the next success
/// (an outage, or just a failed call). Bounded.
const MAX_DEFERRED: usize = 1000;

/// Run until `stop` is set (checked every 500 ms) or the watcher dies.
/// `expected` is shared with any other [`Mover`] in the process (e.g. the
/// app's confirm popup) so their moves are not picked up as new files.
pub fn run_watch(
    cfg: &Config,
    opts: &WatchRunOptions,
    expected: Arc<ExpectedMoves>,
    stop: &AtomicBool,
    mut on_event: impl FnMut(WatchEvent<'_>),
) -> Result<(), ServiceError> {
    if cfg.watches.is_empty() {
        return Err(ServiceError::NoWatchFolders);
    }
    for w in cfg.warnings() {
        tracing::warn!("{w}");
        on_event(WatchEvent::Warning(w));
    }
    crate::netio::network_io().set_limit(usize::from(cfg.mover.nas_concurrency));
    let rules_path = opts
        .rules
        .clone()
        .unwrap_or_else(|| cfg.classify.rules.clone());
    let rules = RuleSet::load(&rules_path)?;
    let store = Store::open(&cfg.store.db_path())?;
    let pruned = store.prune_move_log(cfg.mover.undo_retention_days)?;
    if pruned > 0 {
        tracing::info!(pruned, "old move_log rows pruned");
    }
    if opts.index {
        // Watch folders are index roots too (SPEC §5.5).
        for w in &cfg.watches {
            if let Ok(p) = dunce::canonicalize(&w.path) {
                store.upsert_root(&p, w.kind, true, w.index_content)?;
            }
        }
    }
    let mut mover = Mover::new(store, cfg.mover.duplicate_policy).with_expected(expected.clone());
    if let Some(p) = &opts.progress {
        mover = mover.with_progress(Arc::clone(p));
    }
    if let Some(k) = &opts.vault_keys {
        mover = mover.with_vault_keys(k.clone());
    }
    let grace = opts.recovery_grace.unwrap_or(DEFAULT_RECOVERY_GRACE);
    let recover = |mover: &Mover, on_event: &mut dyn FnMut(WatchEvent<'_>)| match mover
        .recover_interrupted(grace)
    {
        Ok(r) if !r.is_empty() => on_event(WatchEvent::Recovered(&r)),
        Ok(_) => {}
        Err(e) => on_event(WatchEvent::Warning(format!("중단된 이동 복구 실패: {e}"))),
    };
    recover(&mover, &mut on_event);
    let filter = cfg.ignore_filter()?;
    let mut engine = Engine::new(rules, mover, filter.clone())
        .dry_run(opts.dry_run)
        .with_anchors(crate::classify::Anchors::from_config(cfg))
        .with_security(crate::security::SecurityPolicy::from_config(cfg));
    if opts.index {
        engine = engine.with_index(IndexOptions::from_config(cfg));
    }
    if opts.ai && cfg.ai.enabled {
        // Re-read the rules for the AI's view (ai_auto folders, tree roots).
        let rules = RuleSet::load(&rules_path)?;
        match AiClassifier::from_config(cfg, &rules) {
            Ok(ai) => {
                on_event(WatchEvent::Ai(Ok(format!(
                    "{} / {}",
                    ai.provider_name(),
                    ai.model()
                ))));
                engine = engine.with_ai(ai);
                if cfg.curate.enabled {
                    match crate::curate::Curator::from_config(cfg) {
                        Ok(c) => engine = engine.with_curator(Arc::new(c)),
                        Err(e) => on_event(WatchEvent::Warning(format!(
                            "업무 맥락 정리를 쓰지 않습니다: {e}"
                        ))),
                    }
                }
            }
            Err(e) => on_event(WatchEvent::Ai(Err(e.to_string()))),
        }
    }

    let (watcher, rx) = Watcher::start(
        &cfg.watches,
        opts.watch_options
            .clone()
            .unwrap_or_else(|| WatchOptions::from_config(&cfg.classify)),
        expected,
    )?;

    if opts.process_existing {
        let batch = new_batch_id();
        for root in &cfg.watches {
            if !root.path.is_dir() {
                continue; // network folder offline: its files come later
            }
            for path in existing_files(root, &filter) {
                let res = engine.handle(&path, &batch);
                on_event(WatchEvent::Handled(&path, &res));
            }
        }
    }
    on_event(WatchEvent::Started(watcher.roots()));
    let mut roots = watcher.root_states();
    for r in roots.iter().filter(|r| !r.online) {
        on_event(WatchEvent::Root(r));
    }

    let mut retries: HashMap<PathBuf, u32> = HashMap::new();
    let mut deferred: Vec<PathBuf> = Vec::new();
    // Each path is retried once, so a file the AI always fails on cannot
    // cost a request after every success.
    let mut retried: std::collections::HashSet<PathBuf> = Default::default();
    while !stop.load(Ordering::Relaxed) {
        let next = rx.recv_timeout(Duration::from_millis(500));
        if let Some(ai) = engine.ai() {
            if next.is_err() && ai.health().probe_due() {
                // Idle and the AI is down: a tiny request to see if it's back.
                let _ = ai.test_connection(engine.mover().store());
            }
            if let Some(change) = ai.health().take_change() {
                on_event(WatchEvent::AiHealth(change, ai.health().last_error()));
            }
            // The AI answers again (last call succeeded): retry the files it
            // failed on — including those that failed before the outage was
            // declared unavailable.
            if !deferred.is_empty()
                && ai.health().is_available()
                && ai.health().consecutive_failures() == 0
            {
                for p in deferred.drain(..) {
                    watcher.requeue(p);
                }
            }
        }
        // Folder state changes (share lost / back).
        let now = watcher.root_states();
        let mut reconnected = false;
        for (old, new) in roots.iter().zip(&now) {
            if old.online != new.online || old.events != new.events {
                reconnected |= new.online && !old.online;
                on_event(WatchEvent::Root(new));
            }
        }
        roots = now;
        if reconnected {
            // Moves to/from that share that were cut off can be resolved now.
            recover(engine.mover(), &mut on_event);
        }
        let ev = match next {
            Ok(ev) => ev,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let res = engine.handle(&ev.path, &new_batch_id());
        match &res {
            Err(e) if e.is_transient() => {
                let n = retries.entry(ev.path.clone()).or_default();
                *n += 1;
                if *n <= MAX_RETRIES {
                    tracing::info!(path = %ev.path.display(), attempt = *n, "busy, retrying: {e}");
                    watcher.requeue(ev.path);
                    continue;
                }
                retries.remove(&ev.path);
            }
            Ok(Action::AiFailed { path, .. }) => {
                retries.remove(&ev.path);
                if retried.len() > 10 * MAX_DEFERRED {
                    retried.clear();
                }
                if deferred.len() < MAX_DEFERRED
                    && !deferred.contains(path)
                    && retried.insert(path.clone())
                {
                    deferred.push(path.clone());
                }
            }
            _ => {
                retries.remove(&ev.path);
            }
        }
        on_event(WatchEvent::Handled(&ev.path, &res));
    }
    Ok(())
}

/// Files embedded per pass of [`run_embed_loop`].
pub const EMBED_PASS: usize = 256;

/// Background embedding for semantic search (shared by `tidy watch` and
/// the app): embeds files whose vector is missing or stale, pass after
/// pass while there is a backlog, then every `interval`. Uses its own DB
/// connection. Provider errors are reported (once per distinct error) and
/// retried later; they never stop the loop or anything else.
pub fn run_embed_loop(
    cfg: &Config,
    sem: &SemanticSearch,
    stop: &AtomicBool,
    interval: Duration,
    mut on_report: impl FnMut(&EmbedReport),
) -> Result<(), ServiceError> {
    let store = Store::open(&cfg.store.db_path())?;
    let mut last_error: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        let mut again = false;
        match sem.embed_pending(&store, EMBED_PASS, |_| {}) {
            Ok(r) => {
                let new_error = r.error.is_some() && r.error != last_error;
                if r.embedded + r.reused > 0 || new_error {
                    on_report(&r);
                }
                again = r.remaining > 0 && r.error.is_none() && r.embedded + r.reused > 0;
                last_error = r.error;
            }
            Err(e) => tracing::warn!("embedding pass failed: {e}"),
        }
        if again {
            continue;
        }
        let until = std::time::Instant::now() + interval;
        while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

/// (M15) Background text recognition (shared by `tidy watch` and the
/// app): recognizes waiting images and scanned PDFs pass after pass while
/// there is a backlog, then every `interval`. Re-reads nothing but the
/// store; `cfg.ocr.enabled` is the caller's to check. Without an engine
/// the loop only reports why (once) and checks again later.
pub fn run_ocr_loop(
    cfg: &Config,
    stop: &AtomicBool,
    interval: Duration,
    mut on_report: impl FnMut(Result<&crate::index::ocr::OcrReport, &str>),
) -> Result<(), ServiceError> {
    use crate::index::ocr;
    let store = Store::open(&cfg.store.db_path())?;
    let mut engine: Option<Box<dyn ocr::OcrEngine>> = None;
    let mut last_error: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        let mut again = false;
        if engine.is_none() {
            match ocr::build_engine(&cfg.ocr) {
                Ok(e) => {
                    engine = Some(e);
                    last_error = None;
                }
                Err(e) => {
                    let msg = e.to_string();
                    if last_error.as_deref() != Some(msg.as_str()) {
                        on_report(Err(&msg));
                    }
                    last_error = Some(msg);
                }
            }
        }
        if let Some(e) = &engine {
            // One pass at a time per process (a restarted loop may overlap
            // the old one for a moment).
            static PASS: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _pass = PASS.lock().unwrap_or_else(|e| e.into_inner());
            match ocr::run_pass(cfg, &store, e.as_ref(), ocr::OCR_PASS, |_| {}) {
                Ok(r) => {
                    let did = r.recognized + r.empty + r.failed;
                    if did > 0 {
                        on_report(Ok(&r));
                    }
                    again = did > 0 && r.remaining > 0;
                }
                Err(e) => tracing::warn!("ocr pass failed: {e}"),
            }
        }
        if again {
            continue;
        }
        let until = std::time::Instant::now() + interval;
        while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}

/// (M16) Background picture descriptions (shared by `tidy watch` and the
/// app): describes waiting pictures pass after pass while there is a
/// backlog (within `[vision] daily_limit`), then every `interval`.
/// `cfg.vision.enabled` is the caller's to check. When the model cannot be
/// used the loop reports why (once per new reason) and checks again later.
pub fn run_vision_loop(
    cfg: &Config,
    stop: &AtomicBool,
    interval: Duration,
    mut on_report: impl FnMut(Result<&crate::index::vision::VisionReport, &str>),
) -> Result<(), ServiceError> {
    use crate::index::{ocr, vision};
    let store = Store::open(&cfg.store.db_path())?;
    let mut describer: Option<vision::VisionDescriber> = None;
    let mut last_error: Option<String> = None;
    // Pictures whose text is still being read wait for it (their
    // personal-information verdict may change), when something reads it.
    let wait_for_ocr = cfg.ocr.enabled && ocr::build_engine(&cfg.ocr).is_ok();
    fn report_error(
        last: &mut Option<String>,
        msg: String,
        on_report: &mut dyn FnMut(Result<&crate::index::vision::VisionReport, &str>),
    ) {
        if last.as_deref() != Some(msg.as_str()) {
            on_report(Err(&msg));
        }
        *last = Some(msg);
    }
    while !stop.load(Ordering::Relaxed) {
        let mut again = false;
        if describer.is_none() {
            match vision::VisionDescriber::from_config(cfg) {
                Ok(d) => describer = Some(d),
                Err(e) => report_error(&mut last_error, e.to_string(), &mut on_report),
            }
        }
        if let Some(d) = &describer {
            static PASS: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _pass = PASS.lock().unwrap_or_else(|e| e.into_inner());
            match vision::run_pass(cfg, &store, d, vision::VISION_PASS, wait_for_ocr, |_| {}) {
                Ok(r) => {
                    let did = r.described + r.empty + r.failed;
                    if did > 0 || r.skipped > 0 {
                        on_report(Ok(&r));
                    }
                    match &r.stopped {
                        Some(e) => report_error(&mut last_error, e.clone(), &mut on_report),
                        None => last_error = None,
                    }
                    again = did > 0 && r.remaining > 0 && !r.limited && r.stopped.is_none();
                }
                Err(e) => tracing::warn!("vision pass failed: {e}"),
            }
        }
        if again {
            continue;
        }
        let until = std::time::Instant::now() + interval;
        while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Ok(())
}
