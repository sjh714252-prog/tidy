//! Polling of network folders (SPEC §5.1, §5.9).
//!
//! Every `poll_interval` the folder is listed and each file's (size, mtime)
//! compared with the previous listing; new or changed files go to the same
//! stabilizer as OS events (`EventSource::Poll`), which also drops the ones
//! the event path already reported. The first listing is only a baseline.
//!
//! When the folder cannot be listed (share disconnected, NAS asleep) only
//! that folder is paused and re-checked every `reconnect` (60 s). Once it is
//! back, OS events are re-registered and the next listing is compared with
//! the last one taken before the disconnect, so files added meanwhile are
//! picked up.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use notify::Watcher as _;
use serde::Serialize;

use super::{IgnoreFilter, Input};
use crate::config::{RootKind, WatchRoot};

/// Live state of one watch folder, for status displays.
#[derive(Debug, Clone, Serialize)]
pub struct RootState {
    /// Canonical path when it could be resolved, else as configured.
    pub path: PathBuf,
    pub kind: RootKind,
    /// The folder can currently be listed.
    pub online: bool,
    /// OS change events are registered for it.
    pub events: bool,
    /// Polling period in seconds (None = events only).
    pub poll_secs: Option<u64>,
    /// When `online` last changed (or watching started).
    pub since: SystemTime,
    /// Files seen in the last listing (polled folders only).
    pub files: Option<usize>,
}

pub(super) type States = Arc<Mutex<Vec<RootState>>>;
pub(super) type SharedNotify = Arc<Mutex<notify::RecommendedWatcher>>;

type Stamp = (u64, Option<SystemTime>);
type Snapshot = HashMap<PathBuf, Stamp>;

pub(super) struct PollRoot {
    pub idx: usize,
    pub cfg: WatchRoot,
    pub path: PathBuf,
    pub interval: Duration,
    pub online: bool,
    /// `path` is canonical (false while a share offline at startup has
    /// not been seen yet).
    pub resolved: bool,
    pub snapshot: Option<Snapshot>,
    pub next: Instant,
}

pub(super) struct Poller {
    pub roots: Vec<PollRoot>,
    pub reconnect: Duration,
    pub filter: IgnoreFilter,
    pub states: States,
    pub notify: SharedNotify,
    pub input: Sender<Input>,
}

impl Poller {
    /// Runs until `stop` is disconnected (the `Watcher` was dropped).
    pub fn run(mut self, stop: Receiver<()>) {
        loop {
            let now = Instant::now();
            for i in 0..self.roots.len() {
                if self.roots[i].next <= now && !self.poll_one(i) {
                    return; // stabilizer gone
                }
            }
            let next = self
                .roots
                .iter()
                .map(|r| r.next)
                .min()
                .unwrap_or_else(|| now + self.reconnect);
            let wait = next.saturating_duration_since(Instant::now());
            match stop.recv_timeout(wait.max(Duration::from_millis(10))) {
                Err(RecvTimeoutError::Timeout) => {}
                _ => return,
            }
        }
    }

    /// Check one folder. Returns false when the stabilizer has gone away.
    fn poll_one(&mut self, i: usize) -> bool {
        let reachable = if self.roots[i].resolved {
            self.roots[i].path.is_dir()
        } else {
            self.try_resolve(i)
        };
        if !reachable {
            self.set_online(i, false);
            self.roots[i].next = Instant::now() + self.reconnect;
            return true;
        }
        let Some(listing) = list(&self.roots[i], &self.filter) else {
            // Vanished while listing: same as offline.
            self.set_online(i, false);
            self.roots[i].next = Instant::now() + self.reconnect;
            return true;
        };
        if !self.roots[i].online {
            self.set_online(i, true);
            self.rewatch(i);
        }
        let root = &mut self.roots[i];
        let mut changed = Vec::new();
        if let Some(old) = &root.snapshot {
            for (p, stamp) in &listing.files {
                if old.get(p) != Some(stamp) {
                    changed.push(p.clone());
                }
            }
        }
        // Keep what could not be listed this time (permission error on a
        // subfolder…) so it does not come back as "new" next time.
        let mut snap = listing.files;
        if let Some(old) = root.snapshot.take() {
            for (p, s) in old {
                if listing.failed.iter().any(|d| p.starts_with(d)) {
                    snap.entry(p).or_insert(s);
                }
            }
        }
        let count = snap.len();
        root.snapshot = Some(snap);
        root.next = Instant::now() + root.interval;
        let idx = root.idx;
        if let Some(s) = self.lock_states().get_mut(idx) {
            s.files = Some(count);
        }
        for p in changed {
            if self.input.send(Input::Poll(p)).is_err() {
                return false;
            }
        }
        true
    }

    /// A folder that was offline at startup: resolve its canonical path
    /// once it appears, so event paths and moves match.
    fn try_resolve(&mut self, i: usize) -> bool {
        match dunce::canonicalize(&self.roots[i].cfg.path) {
            Ok(p) if p.is_dir() => {
                self.roots[i].resolved = true;
                if self.roots[i].path != p {
                    self.roots[i].path = p.clone();
                    self.roots[i].snapshot = None;
                    let idx = self.roots[i].idx;
                    if let Some(s) = self.lock_states().get_mut(idx) {
                        s.path = p;
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn lock_states(&self) -> std::sync::MutexGuard<'_, Vec<RootState>> {
        self.states.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_online(&mut self, i: usize, online: bool) {
        let root = &mut self.roots[i];
        if root.online == online {
            return;
        }
        root.online = online;
        if online {
            tracing::info!(path = %root.path.display(), "network folder reachable again");
        } else {
            tracing::warn!(path = %root.path.display(), "network folder unreachable; paused, retrying");
        }
        let idx = root.idx;
        let mut st = self.lock_states();
        if let Some(s) = st.get_mut(idx) {
            s.online = online;
            s.since = SystemTime::now();
            if !online {
                s.events = false;
            }
        }
    }

    /// Re-register OS events after a reconnect (the old handle is stale).
    fn rewatch(&mut self, i: usize) {
        let root = &self.roots[i];
        if !root.cfg.uses_events() {
            return;
        }
        let mode = if root.cfg.recursive {
            notify::RecursiveMode::Recursive
        } else {
            notify::RecursiveMode::NonRecursive
        };
        let ok = {
            let mut w = self.notify.lock().unwrap_or_else(|e| e.into_inner());
            let _ = w.unwatch(&root.path);
            match w.watch(&root.path, mode) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(path = %root.path.display(), "events unavailable, polling only: {e}");
                    false
                }
            }
        };
        let idx = root.idx;
        if let Some(s) = self.lock_states().get_mut(idx) {
            s.events = ok;
        }
    }
}

struct Listing {
    files: Snapshot,
    /// Folders that could not be read.
    failed: Vec<PathBuf>,
}

/// List regular, non-ignored files (metadata only). `None` if the root
/// itself became unreadable.
fn list(root: &PollRoot, filter: &IgnoreFilter) -> Option<Listing> {
    let depth = if root.cfg.recursive { usize::MAX } else { 1 };
    let mut out = Listing {
        files: HashMap::new(),
        failed: Vec::new(),
    };
    for entry in walkdir::WalkDir::new(&root.path)
        .min_depth(1)
        .max_depth(depth)
        .follow_links(false)
    {
        match entry {
            Ok(e) if e.file_type().is_file() && !filter.is_ignored(e.path()) => {
                if let Ok(m) = e.metadata() {
                    out.files
                        .insert(e.into_path(), (m.len(), m.modified().ok()));
                }
            }
            Ok(_) => {}
            Err(e) => {
                if let Some(p) = e.path() {
                    out.failed.push(p.to_path_buf());
                }
            }
        }
    }
    if !root.path.is_dir() || out.failed.iter().any(|p| p == &root.path) {
        return None;
    }
    Some(out)
}
