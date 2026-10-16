//! Process-wide app state. The watch loop runs on its own thread with its
//! own database connection; UI commands use `Inner::mover` (a second
//! connection) and share `expected` so moves made from the confirm popup
//! are not picked up by the watcher as new files.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use tidy_core::config::Config;
use tidy_core::mover::{ExpectedMoves, Mover};
use tidy_core::store::Store;

pub struct AppState {
    pub config_path: PathBuf,
    pub inner: Mutex<Inner>,
    pub expected: Arc<ExpectedMoves>,
    pub watch: Mutex<Option<WatchHandle>>,
    pub paused: AtomicBool,
    pub indexing: AtomicBool,
    pub last_error: Mutex<Option<String>>,
    /// Hotkey registration failure; kept apart from `last_error` because the
    /// watch loop clears that one on every (re)start.
    pub hotkey_error: Mutex<Option<String>>,
    /// AI step as reported by the running watch loop: Ok("provider / model")
    /// or why it is off. `None` = AI not enabled in the config.
    pub ai_state: Mutex<Option<Result<String, String>>>,
    /// Watch folders currently unreachable (network shares), as reported
    /// by the watch loop.
    pub offline: Mutex<Vec<PathBuf>>,
}

pub struct Inner {
    pub cfg: Config,
    /// `None` when the database could not be opened (see `last_error`).
    pub mover: Option<Mover>,
    /// Hotkey currently registered (to unregister on change).
    pub hotkey: Option<String>,
}

pub struct WatchHandle {
    pub stop: Arc<AtomicBool>,
    pub thread: JoinHandle<()>,
}

impl AppState {
    pub fn new(config_path: PathBuf) -> Self {
        let expected = ExpectedMoves::new();
        let state = Self {
            config_path,
            inner: Mutex::new(Inner {
                cfg: Config::default(),
                mover: None,
                hotkey: None,
            }),
            expected,
            watch: Mutex::new(None),
            paused: AtomicBool::new(false),
            indexing: AtomicBool::new(false),
            last_error: Mutex::new(None),
            hotkey_error: Mutex::new(None),
            ai_state: Mutex::new(None),
            offline: Mutex::new(Vec::new()),
        };
        state.reload();
        state
    }

    /// (Re)load `config.toml` and reopen the UI database connection.
    pub fn reload(&self) {
        let cfg = load_config(&self.config_path);
        let mover = match &cfg {
            Ok(c) => match Store::open(&c.store.db_path()) {
                Ok(store) => Some(
                    Mover::new(store, c.mover.duplicate_policy)
                        .with_expected(Arc::clone(&self.expected))
                        .with_progress(crate::watch::progress_fn()),
                ),
                Err(e) => {
                    self.set_error(format!("DB를 열 수 없습니다: {e}"));
                    None
                }
            },
            Err(e) => {
                self.set_error(format!("설정 파일 오류: {e}"));
                None
            }
        };
        if let Ok(c) = &cfg {
            tidy_core::netio::network_io().set_limit(usize::from(c.mover.nas_concurrency));
        }
        let mut inner = self.lock_inner();
        inner.cfg = cfg.unwrap_or_else(|_| default_config(&self.config_path));
        inner.mover = mover;
    }

    pub fn lock_inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn config(&self) -> Config {
        self.lock_inner().cfg.clone()
    }

    pub fn set_error(&self, msg: String) {
        tracing::warn!("{msg}");
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg);
    }

    pub fn clear_error(&self) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn set_hotkey_error(&self, msg: Option<String>) {
        if let Some(m) = &msg {
            tracing::warn!("{m}");
        }
        *self.hotkey_error.lock().unwrap_or_else(|e| e.into_inner()) = msg;
    }

    /// What the status bar shows: the watch/DB error, else the hotkey error.
    pub fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .or_else(|| {
                self.hotkey_error
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            })
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// `--config <path>` argument, then `TIDY_CONFIG`, then the platform
/// default (`%APPDATA%\tidy\config.toml` on Windows).
pub fn config_path_from_env() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--config" {
            if let Some(p) = args.next() {
                return PathBuf::from(p);
            }
        } else if let Some(p) = a.strip_prefix("--config=") {
            return PathBuf::from(p);
        }
    }
    if let Some(p) = std::env::var_os("TIDY_CONFIG") {
        return PathBuf::from(p);
    }
    Config::default_path().unwrap_or_else(|| PathBuf::from("config.toml"))
}

fn load_config(path: &Path) -> Result<Config, tidy_core::config::ConfigError> {
    if path.exists() {
        Config::load(path)
    } else {
        Ok(default_config(path))
    }
}

/// Defaults with relative paths (rules.toml) resolved next to `path`.
/// A missing config means no watch folders: nothing moves until the user
/// configures folders in Settings.
fn default_config(path: &Path) -> Config {
    let mut c = Config::default();
    c.resolve_relative(path.parent().unwrap_or_else(|| Path::new("")));
    c
}
