//! Watcher (SPEC §5.1): `notify` events → stabilization → `FileEvent`.
//!
//! A file is reported once its size and mtime have not changed for
//! `stabilize` (default 3s), so half-written downloads are not touched.
//! Temp files (`ignore_globs`), our own scratch copies and paths the mover
//! just touched ("expected moves") are ignored. Network folders are also
//! polled ([`poll`], M6) and feed the same stabilizer with
//! `EventSource::Poll`; a file both paths saw is reported once.

mod poll;

pub use poll::RootState;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use notify::{EventKind, RecursiveMode, Watcher as _};
use thiserror::Error;

use crate::config::{RootKind, WatchRoot};
use crate::mover::{ExpectedMoves, TMP_MARKER};

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("invalid ignore glob \"{glob}\": {source}")]
    Glob {
        glob: String,
        #[source]
        source: globset::Error,
    },
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
    #[error("none of the configured watch folders exist")]
    NoRoots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEventKind {
    Created,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSource {
    Watch,
    Poll,
}

#[derive(Debug, Clone)]
pub struct FileEvent {
    pub path: PathBuf,
    pub kind: FileEventKind,
    pub source: EventSource,
    pub at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub stabilize: Duration,
    pub ignore_globs: Vec<String>,
    /// How often pending files are re-checked.
    pub tick: Duration,
    /// How often an unreachable network folder is re-checked (SPEC: 60 s).
    pub reconnect: Duration,
    /// Poll every polled folder at this period instead of its own
    /// `poll_interval_s` (tests).
    pub poll_override: Option<Duration>,
}

/// SPEC §5.9: re-check a disconnected share every 60 s.
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(60);

impl WatchOptions {
    pub fn from_config(c: &crate::config::Classify) -> Self {
        Self {
            stabilize: Duration::from_secs(c.stabilize_secs),
            ignore_globs: c.ignore_globs.clone(),
            tick: Duration::from_millis(250),
            reconnect: RECONNECT_INTERVAL,
            poll_override: None,
        }
    }
}

/// File-name filter shared by the watcher and the initial scan.
#[derive(Debug, Clone)]
pub struct IgnoreFilter {
    globs: GlobSet,
}

impl IgnoreFilter {
    pub fn new(globs: &[String]) -> Result<Self, WatchError> {
        let mut b = GlobSetBuilder::new();
        for g in globs {
            let glob = GlobBuilder::new(g)
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .map_err(|source| WatchError::Glob {
                    glob: g.clone(),
                    source,
                })?;
            b.add(glob);
        }
        Ok(Self {
            globs: b.build().map_err(|source| WatchError::Glob {
                glob: globs.join(", "),
                source,
            })?,
        })
    }

    pub fn is_ignored(&self, path: &Path) -> bool {
        match path.file_name() {
            Some(name) => name.to_string_lossy().contains(TMP_MARKER) || self.globs.is_match(name),
            None => true,
        }
    }
}

/// Regular, non-ignored files currently in `root` (used for
/// `--process-existing`). Follows `root.recursive`.
pub fn existing_files(root: &WatchRoot, filter: &IgnoreFilter) -> Vec<PathBuf> {
    let depth = if root.recursive { usize::MAX } else { 1 };
    walkdir::WalkDir::new(&root.path)
        .min_depth(1)
        .max_depth(depth)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && !filter.is_ignored(e.path()))
        .map(|e| e.into_path())
        .collect()
}

enum Input {
    Notify(notify::Result<notify::Event>),
    Requeue(PathBuf),
    /// New or changed according to a folder listing.
    Poll(PathBuf),
}

/// Running watcher. Dropping it stops watching and closes the event channel.
pub struct Watcher {
    _notify: poll::SharedNotify,
    input: Sender<Input>,
    _thread: JoinHandle<()>,
    /// Dropping it stops the poller thread.
    _stop_poll: Option<Sender<()>>,
    roots: Vec<PathBuf>,
    states: poll::States,
}

impl Watcher {
    pub fn start(
        roots: &[WatchRoot],
        opts: WatchOptions,
        expected: Arc<ExpectedMoves>,
    ) -> Result<(Self, Receiver<FileEvent>), WatchError> {
        let filter = IgnoreFilter::new(&opts.ignore_globs)?;
        let (in_tx, in_rx) = mpsc::channel::<Input>();
        let (out_tx, out_rx) = mpsc::channel::<FileEvent>();

        let notify_tx = in_tx.clone();
        let mut w = notify::recommended_watcher(move |res| {
            let _ = notify_tx.send(Input::Notify(res));
        })?;

        let mut active = Vec::new();
        let mut states = Vec::new();
        let mut polled = Vec::new();
        for r in roots {
            let network = r.kind == RootKind::Network;
            // Canonical roots so event paths match what the mover records
            // (macOS reports /private/var for /var; no \\?\ on Windows).
            let (path, online) = match dunce::canonicalize(&r.path) {
                Ok(p) if p.is_dir() => (p, true),
                // A network share may be offline right now: keep it and
                // let the poller wait for it.
                _ if network => {
                    tracing::warn!(path = %r.path.display(), "network folder unreachable; waiting for it");
                    (r.path.clone(), false)
                }
                _ => {
                    tracing::warn!(path = %r.path.display(), "watch folder missing; skipped");
                    continue;
                }
            };
            let mut events = false;
            if online && r.uses_events() {
                let mode = if r.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                match w.watch(&path, mode) {
                    Ok(()) => events = true,
                    // Events on a share are best-effort; polling covers it.
                    Err(e) if network => {
                        tracing::warn!(path = %path.display(), "events unavailable, polling only: {e}")
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            let interval = r.poll_interval().map(|i| opts.poll_override.unwrap_or(i));
            tracing::info!(path = %path.display(), recursive = r.recursive, events,
                poll_secs = interval.map(|i| i.as_secs()), "watching");
            states.push(RootState {
                path: path.clone(),
                kind: r.kind,
                online,
                events,
                poll_secs: interval.map(|i| i.as_secs()),
                since: SystemTime::now(),
                files: None,
            });
            if let Some(interval) = interval {
                polled.push(poll::PollRoot {
                    idx: states.len() - 1,
                    cfg: r.clone(),
                    path: path.clone(),
                    interval,
                    online,
                    resolved: online,
                    snapshot: None,
                    // Baseline listing right away.
                    next: Instant::now(),
                });
            }
            active.push(path);
        }
        if active.is_empty() {
            return Err(WatchError::NoRoots);
        }

        let notify = Arc::new(Mutex::new(w));
        let states = Arc::new(Mutex::new(states));
        let stop_poll = if polled.is_empty() {
            None
        } else {
            let (stop_tx, stop_rx) = mpsc::channel::<()>();
            let poller = poll::Poller {
                roots: polled,
                reconnect: opts.reconnect,
                filter: filter.clone(),
                states: Arc::clone(&states),
                notify: Arc::clone(&notify),
                input: in_tx.clone(),
            };
            std::thread::Builder::new()
                .name("tidy-poller".into())
                .spawn(move || poller.run(stop_rx))
                .expect("spawn poller thread");
            Some(stop_tx)
        };

        let thread = std::thread::Builder::new()
            .name("tidy-stabilizer".into())
            .spawn(move || stabilizer(in_rx, out_tx, opts, filter, expected))
            .expect("spawn stabilizer thread");

        Ok((
            Self {
                _notify: notify,
                input: in_tx,
                _thread: thread,
                _stop_poll: stop_poll,
                roots: active,
                states,
            },
            out_rx,
        ))
    }

    /// Canonical paths actually being watched.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Check `path` again later (e.g. it was locked by another program).
    pub fn requeue(&self, path: PathBuf) {
        let _ = self.input.send(Input::Requeue(path));
    }

    /// Live state of every watch folder (online / events / polling).
    pub fn root_states(&self) -> Vec<RootState> {
        self.states
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

struct Tracked {
    kind: FileEventKind,
    source: EventSource,
    size: u64,
    mtime: Option<SystemTime>,
    since: Instant,
}

/// What was last reported for a path, so a file seen twice in the same
/// state (OS events and polling, or a redelivered OS event) is handled once
/// (SPEC §5.1 dedupe).
type Emitted = HashMap<PathBuf, ((u64, Option<SystemTime>), Instant)>;

/// How long a reported (size, mtime) is remembered for dedupe.
const EMITTED_TTL: Duration = Duration::from_secs(3600);

fn stabilizer(
    rx: Receiver<Input>,
    tx: Sender<FileEvent>,
    opts: WatchOptions,
    filter: IgnoreFilter,
    expected: Arc<ExpectedMoves>,
) {
    let mut pending: HashMap<PathBuf, Tracked> = HashMap::new();
    let mut emitted: Emitted = HashMap::new();
    let mut last_tick = Instant::now();
    let mut last_prune = Instant::now();
    loop {
        match rx.recv_timeout(opts.tick) {
            Ok(Input::Notify(Ok(ev))) => {
                let kind = match ev.kind {
                    EventKind::Create(_) => FileEventKind::Created,
                    EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                        FileEventKind::Modified
                    }
                    EventKind::Remove(_) => {
                        for p in &ev.paths {
                            pending.remove(p);
                        }
                        continue;
                    }
                    EventKind::Access(_) => continue,
                };
                for p in ev.paths {
                    if filter.is_ignored(&p) || expected.is_expected(&p) {
                        continue;
                    }
                    track(&mut pending, p, kind, EventSource::Watch);
                }
            }
            Ok(Input::Notify(Err(e))) => tracing::warn!("watch error: {e}"),
            Ok(Input::Requeue(p)) => {
                pending.remove(&p);
                emitted.remove(&p);
                track(&mut pending, p, FileEventKind::Modified, EventSource::Watch);
            }
            Ok(Input::Poll(p)) => {
                if filter.is_ignored(&p) || expected.is_expected(&p) || pending.contains_key(&p) {
                    continue;
                }
                if let Some((stamp, _)) = emitted.get(&p) {
                    if stamp_of(&p).as_ref() == Some(stamp) {
                        continue; // the event path already reported it
                    }
                }
                track(&mut pending, p, FileEventKind::Modified, EventSource::Poll);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        if last_tick.elapsed() < opts.tick {
            continue;
        }
        last_tick = Instant::now();
        if last_prune.elapsed() >= Duration::from_secs(60) {
            last_prune = Instant::now();
            emitted.retain(|_, (_, at)| at.elapsed() < EMITTED_TTL);
        }

        let mut ready = Vec::new();
        pending.retain(|path, t| {
            let Ok(meta) = std::fs::metadata(path) else {
                return false; // gone (renamed away / removed / share offline)
            };
            if !meta.is_file() {
                return false;
            }
            let mtime = meta.modified().ok();
            if meta.len() != t.size || mtime != t.mtime {
                t.size = meta.len();
                t.mtime = mtime;
                t.since = Instant::now();
                return true;
            }
            if t.since.elapsed() >= opts.stabilize {
                ready.push((path.clone(), t.kind, t.source, (t.size, t.mtime)));
                return false;
            }
            true
        });
        for (path, kind, source, stamp) in ready {
            if expected.is_expected(&path) {
                continue;
            }
            // Already reported in this exact state: a late or duplicate OS
            // event (macOS FSEvents coalesces and redelivers) or the poller
            // seeing what events already handled. Requeues clear the entry.
            if emitted.get(&path).is_some_and(|(s, _)| *s == stamp) {
                continue;
            }
            emitted.insert(path.clone(), (stamp, Instant::now()));
            let ev = FileEvent {
                path,
                kind,
                source,
                at: SystemTime::now(),
            };
            if tx.send(ev).is_err() {
                return;
            }
        }
    }
}

fn stamp_of(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

fn track(
    pending: &mut HashMap<PathBuf, Tracked>,
    path: PathBuf,
    kind: FileEventKind,
    source: EventSource,
) {
    let Ok(meta) = std::fs::metadata(&path) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    let mtime = meta.modified().ok();
    let size = meta.len();
    pending
        .entry(path)
        .and_modify(|t| {
            if t.size != size || t.mtime != mtime {
                t.size = size;
                t.mtime = mtime;
                t.since = Instant::now();
            }
        })
        .or_insert(Tracked {
            kind,
            source,
            size,
            mtime,
            since: Instant::now(),
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(secs: u64) -> WatchOptions {
        WatchOptions {
            stabilize: Duration::from_secs(secs),
            ignore_globs: crate::config::Classify::default().ignore_globs,
            tick: Duration::from_millis(100),
            reconnect: Duration::from_millis(200),
            poll_override: Some(Duration::from_millis(200)),
        }
    }

    #[test]
    fn ignore_filter() {
        let f = IgnoreFilter::new(&crate::config::Classify::default().ignore_globs).unwrap();
        for name in [
            "a.tmp",
            "영상.mp4.crdownload",
            "x.PART",
            "~$보고서.docx",
            ".DS_Store",
            "thumbs.db",
            ".a.txt.tidy-tmp.0123",
        ] {
            assert!(f.is_ignored(Path::new(name)), "{name}");
        }
        for name in ["보고서.docx", "a.tmpl", "스크린샷 1.png"] {
            assert!(!f.is_ignored(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn existing_files_respects_recursion_and_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.txt"), "x").unwrap();
        std::fs::write(dir.path().join("c.tmp"), "x").unwrap();
        std::fs::create_dir(dir.path().join("하위 폴더")).unwrap();
        std::fs::write(dir.path().join("하위 폴더").join("d.txt"), "x").unwrap();
        let f = IgnoreFilter::new(&["*.tmp".into()]).unwrap();
        let mut root = WatchRoot {
            path: dir.path().to_path_buf(),
            ..Default::default()
        };
        assert_eq!(existing_files(&root, &f).len(), 1);
        root.recursive = true;
        assert_eq!(existing_files(&root, &f).len(), 2);
    }

    #[test]
    fn reports_new_file_after_it_stabilizes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("감시 폴더");
        std::fs::create_dir(&root).unwrap();
        let roots = [WatchRoot {
            path: root.clone(),
            ..Default::default()
        }];
        let expected = ExpectedMoves::new();
        let (w, rx) = Watcher::start(&roots, opts(1), expected.clone()).unwrap();
        let canon = w.roots()[0].clone();

        std::fs::write(canon.join("무시.tmp"), "x").unwrap();
        // Simulate one of our own moves arriving.
        expected.expect(&canon.join("우리가 옮김.txt"));
        std::fs::write(canon.join("우리가 옮김.txt"), "x").unwrap();
        let created = canon.join("새 문서 1.txt");
        std::fs::write(&created, "hello").unwrap();

        let ev = rx.recv_timeout(Duration::from_secs(15)).expect("event");
        assert_eq!(ev.path, created);
        assert!(rx.recv_timeout(Duration::from_millis(2500)).is_err());

        // Requeue re-reports the same file.
        w.requeue(created.clone());
        let ev = rx.recv_timeout(Duration::from_secs(15)).expect("requeued");
        assert_eq!(ev.path, created);
    }

    fn wait_for(w: &Watcher, what: &str, f: impl Fn(&RootState) -> bool) {
        let until = Instant::now() + Duration::from_secs(15);
        while Instant::now() < until {
            if f(&w.root_states()[0]) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {what}: {:?}", w.root_states());
    }

    /// Windows refuses to rename a folder while another thread briefly
    /// has it open (a listing in progress).
    fn rename_retry(from: &Path, to: &Path) {
        for _ in 0..50 {
            if std::fs::rename(from, to).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        std::fs::rename(from, to).unwrap();
    }

    fn nas_root(path: &Path, poll_only: bool) -> WatchRoot {
        WatchRoot {
            path: path.to_path_buf(),
            kind: RootKind::Network,
            recursive: true,
            poll_only,
            ..Default::default()
        }
    }

    #[test]
    fn polls_network_folder_without_events() {
        let dir = tempfile::tempdir().unwrap();
        let nas = dir.path().join("NAS 공유");
        std::fs::create_dir_all(nas.join("하위 폴더")).unwrap();
        std::fs::write(nas.join("이미 있던 파일.txt"), "x").unwrap();
        let (w, rx) =
            Watcher::start(&[nas_root(&nas, true)], opts(0), ExpectedMoves::new()).unwrap();
        let st = &w.root_states()[0];
        assert!(st.online && !st.events);
        assert_eq!(st.poll_secs, Some(0));
        wait_for(&w, "baseline", |s| s.files == Some(1));

        let canon = w.roots()[0].clone();
        let created = canon.join("하위 폴더").join("새 보고서 1.hwp");
        std::fs::write(&created, "보고서").unwrap();
        std::fs::write(canon.join("무시.tmp"), "x").unwrap();
        let ev = rx.recv_timeout(Duration::from_secs(15)).expect("polled");
        assert_eq!(ev.path, created);
        assert_eq!(ev.source, EventSource::Poll);
        // Unchanged files are not reported again by later listings.
        assert!(rx.recv_timeout(Duration::from_millis(1000)).is_err());
        // A change is.
        std::fs::write(&created, "보고서 v2").unwrap();
        let ev = rx.recv_timeout(Duration::from_secs(15)).expect("changed");
        assert_eq!(ev.path, created);
    }

    #[test]
    fn disconnect_pauses_and_reconnect_picks_up_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let nas = dir.path().join("나스");
        std::fs::create_dir(&nas).unwrap();
        let (w, rx) =
            Watcher::start(&[nas_root(&nas, true)], opts(0), ExpectedMoves::new()).unwrap();
        wait_for(&w, "baseline", |s| s.files == Some(0));
        let canon = w.roots()[0].clone();

        // "Unmount": the share disappears.
        let away = dir.path().join("나스 (끊김)");
        rename_retry(&canon, &away);
        wait_for(&w, "offline", |s| !s.online);
        // Someone adds a file on the NAS meanwhile.
        std::fs::write(away.join("끊긴 동안 추가.pdf"), "pdf").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(600)).is_err());

        // "Remount".
        rename_retry(&away, &canon);
        wait_for(&w, "online", |s| s.online);
        let ev = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("after reconnect");
        assert_eq!(ev.path, canon.join("끊긴 동안 추가.pdf"));
        assert_eq!(ev.source, EventSource::Poll);
    }

    #[test]
    fn offline_at_start_is_not_fatal_for_network_folders() {
        let dir = tempfile::tempdir().unwrap();
        let nas = dir.path().join("아직 없는 공유");
        let (w, rx) =
            Watcher::start(&[nas_root(&nas, false)], opts(0), ExpectedMoves::new()).unwrap();
        assert!(!w.root_states()[0].online);
        std::fs::create_dir(&nas).unwrap();
        wait_for(&w, "online", |s| s.online && s.files == Some(0));
        // Events are registered once the share is reachable.
        assert!(w.root_states()[0].events);
        let canon = dunce::canonicalize(&nas).unwrap();
        std::fs::write(canon.join("a.txt"), "a").unwrap();
        let ev = rx.recv_timeout(Duration::from_secs(15)).expect("event");
        assert_eq!(ev.path, canon.join("a.txt"));
        // Seen by events and by the next listing: reported once.
        assert!(rx.recv_timeout(Duration::from_millis(1500)).is_err());
    }

    #[test]
    fn missing_roots_error() {
        let roots = [WatchRoot {
            path: PathBuf::from("/definitely/not/here/없음"),
            ..Default::default()
        }];
        assert!(matches!(
            Watcher::start(&roots, opts(1), ExpectedMoves::new()),
            Err(WatchError::NoRoots)
        ));
    }
}
