//! The long-running watch loop shared by `tidy watch` and the desktop app:
//! watcher events → [`Engine`] (rules → pending / move) → search index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::classify::{AiClassifier, RuleError, RuleSet};
use crate::config::Config;
use crate::engine::{new_batch_id, Action, Engine, EngineError};
use crate::index::IndexOptions;
use crate::mover::{ExpectedMoves, Mover, ProgressFn, RecoveryReport, DEFAULT_RECOVERY_GRACE};
use crate::store::{Store, StoreError};
use crate::watcher::{existing_files, IgnoreFilter, RootState, WatchError, WatchOptions, Watcher};

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
}

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
    let grace = opts.recovery_grace.unwrap_or(DEFAULT_RECOVERY_GRACE);
    let recover = |mover: &Mover, on_event: &mut dyn FnMut(WatchEvent<'_>)| match mover
        .recover_interrupted(grace)
    {
        Ok(r) if !r.is_empty() => on_event(WatchEvent::Recovered(&r)),
        Ok(_) => {}
        Err(e) => on_event(WatchEvent::Warning(format!("중단된 이동 복구 실패: {e}"))),
    };
    recover(&mover, &mut on_event);
    let filter = IgnoreFilter::new(&cfg.classify.ignore_globs)?;
    let mut engine = Engine::new(rules, mover, filter.clone()).dry_run(opts.dry_run);
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
    while !stop.load(Ordering::Relaxed) {
        let next = rx.recv_timeout(Duration::from_millis(500));
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
            _ => {
                retries.remove(&ev.path);
            }
        }
        on_event(WatchEvent::Handled(&ev.path, &res));
    }
    Ok(())
}
