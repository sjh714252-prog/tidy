//! Watcher (SPEC §5.1): `notify` events → stabilization → `FileEvent`.
//!
//! A file is reported once its size and mtime have not changed for
//! `stabilize` (default 3s), so half-written downloads are not touched.
//! Temp files (`ignore_globs`), our own scratch copies and paths the mover
//! just touched ("expected moves") are ignored. Network polling (M6) will
//! feed the same stabilizer with `EventSource::Poll`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use notify::{EventKind, RecursiveMode, Watcher as _};
use thiserror::Error;

use crate::config::WatchRoot;
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
}

impl WatchOptions {
    pub fn from_config(c: &crate::config::Classify) -> Self {
        Self {
            stabilize: Duration::from_secs(c.stabilize_secs),
            ignore_globs: c.ignore_globs.clone(),
            tick: Duration::from_millis(250),
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
}

/// Running watcher. Dropping it stops watching and closes the event channel.
pub struct Watcher {
    _notify: notify::RecommendedWatcher,
    input: Sender<Input>,
    _thread: JoinHandle<()>,
    roots: Vec<PathBuf>,
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
        for r in roots {
            // Canonical roots so event paths match what the mover records
            // (macOS reports /private/var for /var; no \\?\ on Windows).
            let path = match dunce::canonicalize(&r.path) {
                Ok(p) if p.is_dir() => p,
                _ => {
                    tracing::warn!(path = %r.path.display(), "watch folder missing; skipped");
                    continue;
                }
            };
            let mode = if r.recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            w.watch(&path, mode)?;
            tracing::info!(path = %path.display(), recursive = r.recursive, "watching");
            active.push(path);
        }
        if active.is_empty() {
            return Err(WatchError::NoRoots);
        }

        let thread = std::thread::Builder::new()
            .name("tidy-stabilizer".into())
            .spawn(move || stabilizer(in_rx, out_tx, opts, filter, expected))
            .expect("spawn stabilizer thread");

        Ok((
            Self {
                _notify: w,
                input: in_tx,
                _thread: thread,
                roots: active,
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
}

struct Tracked {
    kind: FileEventKind,
    size: u64,
    mtime: Option<SystemTime>,
    since: Instant,
}

fn stabilizer(
    rx: Receiver<Input>,
    tx: Sender<FileEvent>,
    opts: WatchOptions,
    filter: IgnoreFilter,
    expected: Arc<ExpectedMoves>,
) {
    let mut pending: HashMap<PathBuf, Tracked> = HashMap::new();
    let mut last_tick = Instant::now();
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
                    track(&mut pending, p, kind);
                }
            }
            Ok(Input::Notify(Err(e))) => tracing::warn!("watch error: {e}"),
            Ok(Input::Requeue(p)) => {
                pending.remove(&p);
                track(&mut pending, p, FileEventKind::Modified);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        if last_tick.elapsed() < opts.tick {
            continue;
        }
        last_tick = Instant::now();

        let mut ready = Vec::new();
        pending.retain(|path, t| {
            let Ok(meta) = std::fs::metadata(path) else {
                return false; // gone (renamed away / removed)
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
                ready.push((path.clone(), t.kind));
                return false;
            }
            true
        });
        for (path, kind) in ready {
            if expected.is_expected(&path) {
                continue;
            }
            let ev = FileEvent {
                path,
                kind,
                source: EventSource::Watch,
                at: SystemTime::now(),
            };
            if tx.send(ev).is_err() {
                return;
            }
        }
    }
}

fn track(pending: &mut HashMap<PathBuf, Tracked>, path: PathBuf, kind: FileEventKind) {
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
