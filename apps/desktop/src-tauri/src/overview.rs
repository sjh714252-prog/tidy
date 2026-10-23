//! (M11, SPEC §14.6) Watch overview for the home tab: one row per watch
//! folder (organize mode, state, excluded folders) and the backlog of files
//! that were there before watching started. Counting walks the folders, so
//! it runs on a background thread, is cached, and announces itself with
//! `tidy://organize-status`. Nothing here moves a file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tidy_core::classify::RuleSet;
use tidy_core::config::{OrganizeMode, RootKind, WatchRoot};
use tidy_core::curate::ContextDict;
use tidy_core::report::backlog::{backlog, organize_roots, Backlog, BACKLOG_CAP};

use crate::commands::{blocking, CmdResult};
use crate::state::AppState;

pub const EV_ORGANIZE: &str = "tidy://organize-status";
/// Counts older than this are recounted on the next request.
const TTL: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Cache {
    /// Watches + rules the counts were made for.
    key: String,
    at: Option<Instant>,
    counts: HashMap<PathBuf, Backlog>,
    computing: bool,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// Forget the counts (settings saved, watching restarted, files moved).
pub fn invalidate() {
    if let Some(c) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        c.at = None;
    }
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BacklogDto {
    pub to_move: usize,
    pub no_rule: usize,
    pub total: usize,
    pub capped: bool,
}

impl From<&Backlog> for BacklogDto {
    fn from(b: &Backlog) -> Self {
        Self {
            to_move: b.to_move,
            no_rule: b.no_rule,
            total: b.total(),
            capped: b.capped,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub path: String,
    /// "in_place" | "collect"
    pub mode: &'static str,
    /// collect: where results go.
    pub organize_root: Option<String>,
    pub network: bool,
    pub recursive: bool,
    /// "ok" | "offline" | "paused" | "idle" (not watching: wizard / no loop)
    pub state: &'static str,
    pub exclude: usize,
    /// `None` while counting, or for network folders (not counted).
    pub backlog: Option<BacklogDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewDto {
    pub folders: Vec<FolderDto>,
    pub computing: bool,
    /// Sum over counted folders (to_move + no_rule).
    pub total: usize,
    pub capped: bool,
    /// AI on and at least one customer in the context dictionary: offer
    /// "업무 맥락으로 정리" next to the rules preview.
    pub can_curate: bool,
    pub cap: usize,
}

/// Distinct folders results go to: `(in_place count, collect roots)`.
pub fn organize_summary(watches: &[WatchRoot]) -> (usize, Vec<String>) {
    let in_place = watches
        .iter()
        .filter(|w| w.mode == OrganizeMode::InPlace || w.anchor() == w.path.as_path())
        .count();
    let mut roots: Vec<String> = organize_roots(watches).iter().map(|p| s(p)).collect();
    roots.sort();
    roots.dedup();
    (in_place, roots)
}

fn key_for(watches: &[WatchRoot], rules_text: &str) -> String {
    format!("{watches:?}|{rules_text}")
}

/// Rows for the overview; counts from the cache (fresh or not).
#[tauri::command]
pub async fn watch_overview(app: AppHandle, refresh: bool) -> CmdResult<OverviewDto> {
    blocking(move || Ok(overview(&app, refresh))).await
}

pub fn overview(app: &AppHandle, refresh: bool) -> OverviewDto {
    let state = app.state::<AppState>();
    let cfg = state.config();
    let running = state
        .watch
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();
    let paused = state.is_paused();
    let offline: Vec<PathBuf> = state
        .offline
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let rules_text = std::fs::read_to_string(&cfg.classify.rules).unwrap_or_default();
    let key = key_for(&cfg.watches, &rules_text);

    let (counts, computing) = {
        let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let c = guard.get_or_insert_with(Cache::default);
        let stale = c.key != key || c.at.is_none_or(|t| t.elapsed() > TTL) || refresh;
        if c.key != key {
            c.counts.clear();
        }
        if stale && !c.computing {
            c.computing = true;
            spawn_count(
                app,
                key.clone(),
                cfg.watches.clone(),
                rules_text,
                cfg.clone(),
            );
        }
        (c.counts.clone(), c.computing)
    };

    let folders: Vec<FolderDto> = cfg
        .watches
        .iter()
        .map(|w| {
            let network = w.kind == RootKind::Network;
            FolderDto {
                path: s(&w.path),
                mode: if w.anchor() == w.path.as_path() {
                    "in_place"
                } else {
                    "collect"
                },
                organize_root: (w.anchor() != w.path.as_path()).then(|| s(w.anchor())),
                network,
                recursive: w.recursive,
                state: if !running {
                    if paused {
                        "paused"
                    } else {
                        "idle"
                    }
                } else if offline.iter().any(|o| o == &w.path) {
                    "offline"
                } else {
                    "ok"
                },
                exclude: w.exclude.len(),
                backlog: if network {
                    None
                } else {
                    counts.get(&w.path).map(BacklogDto::from)
                },
            }
        })
        .collect();
    let total = folders
        .iter()
        .filter_map(|f| f.backlog.as_ref())
        .map(|b| b.total)
        .sum();
    let capped = folders
        .iter()
        .filter_map(|f| f.backlog.as_ref())
        .any(|b| b.capped);
    let ai_on = matches!(
        &*state.ai_state.lock().unwrap_or_else(|e| e.into_inner()),
        Some(Ok(_))
    );
    let can_curate = ai_on
        && ContextDict::load(&cfg.curate.context)
            .map(|d| !d.entities.is_empty())
            .unwrap_or(false);
    OverviewDto {
        folders,
        computing,
        total,
        capped,
        can_curate,
        cap: BACKLOG_CAP,
    }
}

fn spawn_count(
    app: &AppHandle,
    key: String,
    watches: Vec<WatchRoot>,
    rules_text: String,
    cfg: tidy_core::Config,
) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("tidy-backlog".into())
        .spawn(move || {
            let rules = RuleSet::from_toml(&rules_text).unwrap_or_default();
            let filter = cfg
                .ignore_filter()
                .unwrap_or_else(|_| tidy_core::watcher::IgnoreFilter::new(&[]).unwrap());
            let skip = organize_roots(&watches);
            let mut counts = HashMap::new();
            for w in watches.iter().filter(|w| w.kind == RootKind::Local) {
                counts.insert(
                    w.path.clone(),
                    backlog(w, &rules, &filter, &skip, BACKLOG_CAP),
                );
            }
            {
                let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
                let c = guard.get_or_insert_with(Cache::default);
                c.computing = false;
                c.key = key;
                c.at = Some(Instant::now());
                c.counts = counts;
            }
            let _ = app.emit(EV_ORGANIZE, ());
        });
}
