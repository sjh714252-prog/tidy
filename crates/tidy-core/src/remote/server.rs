//! (M16, SPEC §14.13) The NAS agent: `tidy serve`. Runs the ordinary watch
//! loop (rules → move / confirm queue, indexing) and text recognition on
//! the NAS itself, and answers the PC app over HTTP (`/api/v1`, bearer
//! token). Files are only ever moved by the [`Mover`] (CLAUDE.md rule 1).

use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use tiny_http::{Header, Method, Request, Response, Server};

use super::api::*;
use crate::classify::RuleSet;
use crate::config::{Config, ConfigError, IndexRoot, RootKind};
use crate::engine::{new_batch_id, reindex_undo};
use crate::index::{jobs_from_config, IndexOptions, Indexer};
use crate::mover::{ExpectedMoves, Mover};
use crate::pending::{resolve, Resolution};
use crate::report::restructure;
use crate::search::{search, Query, SearchOptions};
use crate::service::{run_ocr_loop, run_watch, WatchEvent, WatchRunOptions};
use crate::store::Store;
use crate::watcher::WatchOptions;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot listen on {addr}: {msg}")]
    Listen { addr: String, msg: String },
    #[error("data root {0} is not a folder")]
    DataRoot(PathBuf),
    #[error("token file {path}: {source}")]
    Token {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// File (next to the config) holding the generated connection password.
pub const TOKEN_FILE: &str = "agent-token";
/// Mark around matches in search results (same as the app).
const MARK_START: &str = "\u{E000}";
const MARK_END: &str = "\u{E001}";
/// Folders a NAS shows but nobody organizes (Synology / QNAP system dirs).
const SYSTEM_DIRS: &[&str] = &[
    "@eaDir",
    "#recycle",
    "#snapshot",
    "@Recycle",
    "@Recently-Snapshot",
    ".@__thumb",
];

#[derive(Clone)]
pub struct AgentOptions {
    pub config_path: PathBuf,
    /// Everything the PC may pick or change is inside this folder.
    pub data_root: PathBuf,
    /// `0.0.0.0:7841`
    pub listen: String,
    pub token: String,
    /// Tests: sub-second watcher timings.
    pub watch_options: Option<WatchOptions>,
    /// Re-index every folder this often (catches what events missed).
    pub reindex_every: Duration,
}

impl std::fmt::Debug for AgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentOptions")
            .field("config_path", &self.config_path)
            .field("data_root", &self.data_root)
            .field("listen", &self.listen)
            .finish_non_exhaustive()
    }
}

/// Create a minimal config at `path` when there is none: no watch folders
/// (nothing moves until the PC sets some), DB and rules next to it.
pub fn ensure_config(path: &Path) -> Result<Config, ConfigError> {
    if !path.exists() {
        let text =
            "# Tidy NAS agent (tidy serve). Watch folders and rules are set from the PC app\n\
                    # (설정 › NAS); editing here works too — restart the agent after.\n\n\
                    [general]\nonboarded = true\n\n\
                    [classify]\nrules = \"rules.toml\"\n\n\
                    [store]\npath = \"tidy.db\"\n";
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, text).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Config::load(path)
}

/// The connection password: `TIDY_TOKEN` when set, else the one in
/// `<config dir>/agent-token`, made on first start (32 random bytes, hex,
/// readable by the owner only). Never logged.
pub fn load_or_create_token(config_path: &Path, env: Option<String>) -> Result<String, AgentError> {
    if let Some(t) = env.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        return Ok(t);
    }
    let path = token_path(config_path);
    match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => return Ok(t.trim().to_string()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(AgentError::Token { path, source }),
    }
    let token = new_token();
    write_private(&path, &token).map_err(|source| AgentError::Token {
        path: path.clone(),
        source,
    })?;
    Ok(token)
}

pub fn token_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(TOKEN_FILE)
}

/// 64 hex characters from two random UUIDs (244 random bits).
pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(text.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, text)
    }
}

/// Compare without stopping at the first difference.
fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(1));
    }
    diff == 0
}

#[derive(Default)]
struct WatchInfo {
    running: bool,
    error: Option<String>,
    /// Canonical root → polled instead of events.
    polling: HashMap<PathBuf, bool>,
}

struct WatchRun {
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

struct Shared {
    opts: AgentOptions,
    data_root: PathBuf,
    cfg: Mutex<Config>,
    expected: Arc<ExpectedMoves>,
    watch: Mutex<Option<WatchRun>>,
    info: Mutex<WatchInfo>,
    /// One moving action (resolve / undo / organize / settings) at a time.
    act: Mutex<()>,
    indexing: AtomicBool,
    stop: Arc<AtomicBool>,
    started_at: i64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A running agent (server + watch loop + background work).
pub struct RunningAgent {
    shared: Arc<Shared>,
    server: Arc<Server>,
    addr: SocketAddr,
    http: Option<JoinHandle<()>>,
}

impl RunningAgent {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Serve until the process ends.
    pub fn wait(mut self) {
        if let Some(h) = self.http.take() {
            let _ = h.join();
        }
    }

    /// Stop serving and watching (tests).
    pub fn stop(mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.server.unblock();
        if let Some(h) = self.http.take() {
            let _ = h.join();
        }
        stop_watch(&self.shared);
    }
}

/// Start the agent: load (or create) the config, start watching, indexing
/// and text recognition, and serve the API.
pub fn start(opts: AgentOptions) -> Result<RunningAgent, AgentError> {
    let cfg = ensure_config(&opts.config_path)?;
    let data_root = dunce::canonicalize(&opts.data_root)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| AgentError::DataRoot(opts.data_root.clone()))?;
    let server = Server::http(opts.listen.as_str()).map_err(|e| AgentError::Listen {
        addr: opts.listen.clone(),
        msg: e.to_string(),
    })?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| AgentError::Listen {
            addr: opts.listen.clone(),
            msg: "not an IP address".into(),
        })?;
    let shared = Arc::new(Shared {
        opts,
        data_root,
        cfg: Mutex::new(cfg),
        expected: ExpectedMoves::new(),
        watch: Mutex::new(None),
        info: Mutex::new(WatchInfo::default()),
        act: Mutex::new(()),
        indexing: AtomicBool::new(false),
        stop: Arc::new(AtomicBool::new(false)),
        started_at: chrono::Utc::now().timestamp(),
    });
    start_watch(&shared);
    spawn_index(&shared);
    spawn_ocr(&shared);
    spawn_reindex_timer(&shared);

    let server = Arc::new(server);
    let http = {
        let shared = Arc::clone(&shared);
        let server = Arc::clone(&server);
        std::thread::Builder::new()
            .name("tidy-agent-http".into())
            .spawn(move || {
                for req in server.incoming_requests() {
                    if shared.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let shared = Arc::clone(&shared);
                    let _ = std::thread::Builder::new()
                        .name("tidy-agent-req".into())
                        .spawn(move || handle(&shared, req));
                }
            })
            .expect("spawn agent http thread")
    };
    tracing::info!(%addr, root = %shared.data_root.display(), "NAS agent listening");
    Ok(RunningAgent {
        shared,
        server,
        addr,
        http: Some(http),
    })
}

// ── background work ─────────────────────────────────────────────────────

fn start_watch(shared: &Arc<Shared>) {
    let cfg = lock(&shared.cfg).clone();
    {
        let mut info = lock(&shared.info);
        *info = WatchInfo::default();
    }
    if cfg.watches.is_empty() {
        tracing::info!("no watch folders yet — set them from the PC app (설정 › NAS)");
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let join = {
        let shared = Arc::clone(shared);
        let stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("tidy-agent-watch".into())
            .spawn(move || watch_loop(&shared, &cfg, &stop))
            .expect("spawn agent watch thread")
    };
    *lock(&shared.watch) = Some(WatchRun { stop, join });
}

fn stop_watch(shared: &Shared) {
    let run = lock(&shared.watch).take();
    if let Some(run) = run {
        run.stop.store(true, Ordering::SeqCst);
        let _ = run.join.join();
    }
}

fn watch_loop(shared: &Arc<Shared>, cfg: &Config, stop: &AtomicBool) {
    let opts = WatchRunOptions {
        index: true,
        ai: false, // no keychain in a container: rules + search (rule 4)
        watch_options: shared.opts.watch_options.clone(),
        ..Default::default()
    };
    while !stop.load(Ordering::SeqCst) {
        let res = run_watch(
            cfg,
            &opts,
            Arc::clone(&shared.expected),
            stop,
            |ev| match ev {
                WatchEvent::Started(roots) => {
                    let mut info = lock(&shared.info);
                    info.running = true;
                    info.error = None;
                    tracing::info!("watching {} folders", roots.len());
                }
                WatchEvent::Root(r) => {
                    lock(&shared.info).polling.insert(r.path.clone(), !r.events);
                    tracing::info!(path = %r.path.display(), online = r.online, "folder state");
                }
                WatchEvent::Warning(w) => tracing::warn!("{w}"),
                WatchEvent::Handled(p, Ok(a)) => tracing::info!(path = %p.display(), "{a:?}"),
                WatchEvent::Handled(p, Err(e)) => {
                    tracing::warn!(path = %p.display(), "not organized: {e}")
                }
                _ => {}
            },
        );
        lock(&shared.info).running = false;
        if let Err(e) = &res {
            tracing::warn!("watching stopped: {e}");
            lock(&shared.info).error = Some(e.to_string());
        }
        // Stopped on purpose, or the watcher died / a folder was missing:
        // try again in a minute.
        let until = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Index every folder once (startup, settings saved, hourly).
fn spawn_index(shared: &Arc<Shared>) {
    if shared.indexing.swap(true, Ordering::SeqCst) {
        return;
    }
    let shared = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("tidy-agent-index".into())
        .spawn(move || {
            let cfg = lock(&shared.cfg).clone();
            match Store::open(&cfg.store.db_path()) {
                Ok(store) => {
                    let indexer = Indexer::new(&store);
                    for (path, mut o) in jobs_from_config(&cfg, false) {
                        o.batch_size = 50;
                        if !path.is_dir() || shared.stop.load(Ordering::SeqCst) {
                            continue;
                        }
                        match indexer.index_root(&path, &o, |_, _| {}) {
                            Ok(r) => tracing::info!(
                                root = %path.display(),
                                files = r.scanned,
                                changed = r.added + r.updated + r.removed,
                                "indexed"
                            ),
                            Err(e) => tracing::warn!("index {}: {e}", path.display()),
                        }
                    }
                }
                Err(e) => tracing::warn!("index: {e}"),
            }
            shared.indexing.store(false, Ordering::SeqCst);
        });
}

fn spawn_reindex_timer(shared: &Arc<Shared>) {
    let every = shared.opts.reindex_every;
    let shared = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("tidy-agent-reindex".into())
        .spawn(move || loop {
            let until = std::time::Instant::now() + every;
            while std::time::Instant::now() < until {
                if shared.stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            spawn_index(&shared);
        });
}

fn spawn_ocr(shared: &Arc<Shared>) {
    let cfg = lock(&shared.cfg).clone();
    if !cfg.ocr.enabled {
        return;
    }
    let stop = Arc::clone(&shared.stop);
    let _ = std::thread::Builder::new()
        .name("tidy-agent-ocr".into())
        .spawn(move || {
            let res = run_ocr_loop(&cfg, &stop, Duration::from_secs(60), |r| match r {
                Ok(r) => tracing::info!(
                    recognized = r.recognized,
                    remaining = r.remaining,
                    "text recognition"
                ),
                Err(e) => tracing::warn!("text recognition off: {e}"),
            });
            if let Err(e) = res {
                tracing::warn!("text recognition stopped: {e}");
            }
        });
}

// ── HTTP ────────────────────────────────────────────────────────────────

struct ApiError {
    code: u16,
    msg: String,
}

type ApiResult<T> = Result<T, ApiError>;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError {
        code: 400,
        msg: msg.into(),
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError {
        code: 500,
        msg: e.to_string(),
    }
}

fn json_header() -> Header {
    Header::from_bytes("Content-Type", "application/json; charset=utf-8").expect("header")
}

fn respond_json<T: Serialize>(req: Request, code: u16, body: &T) {
    let text = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
    let _ = req.respond(
        Response::from_string(text)
            .with_status_code(code)
            .with_header(json_header()),
    );
}

/// `%XX` and `+` decoding of one query component.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn read_json<T: DeserializeOwned>(req: &mut Request) -> ApiResult<T> {
    let mut buf = Vec::new();
    req.as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| bad(format!("요청을 읽지 못했습니다: {e}")))?;
    if buf.len() > MAX_BODY {
        return Err(ApiError {
            code: 413,
            msg: "요청이 너무 큽니다".into(),
        });
    }
    serde_json::from_slice(&buf).map_err(|e| bad(format!("요청 형식이 올바르지 않습니다: {e}")))
}

fn handle(shared: &Arc<Shared>, mut req: Request) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let Some(route) = path
        .strip_prefix(API)
        .map(|r| r.trim_matches('/').to_string())
    else {
        let _ = req.respond(Response::from_string("Tidy NAS agent\n").with_status_code(404));
        return;
    };
    let q = parse_query(query);
    let method = req.method().clone();

    // Unauthenticated: lets the PC tell "wrong address" from "wrong password".
    if method == Method::Get && route == "hello" {
        respond_json(
            req,
            200,
            &serde_json::json!({ "app": "tidy-agent", "version": env!("CARGO_PKG_VERSION") }),
        );
        return;
    }
    let authorized = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer ").map(str::to_string))
        .is_some_and(|t| same_secret(t.trim(), &shared.opts.token));
    if !authorized {
        // Slow down guessing.
        std::thread::sleep(Duration::from_secs(1));
        respond_json(
            req,
            401,
            &ErrorBody {
                error: "연결 암호가 맞지 않습니다".into(),
            },
        );
        return;
    }

    macro_rules! reply {
        ($res:expr) => {
            match $res {
                Ok(v) => respond_json(req, 200, &v),
                Err(ApiError { code, msg }) => {
                    if code >= 500 {
                        tracing::warn!(route = %route, "{msg}");
                    }
                    respond_json(req, code, &ErrorBody { error: msg })
                }
            }
        };
    }

    match (method, route.as_str()) {
        (Method::Get, "status") => reply!(status(shared)),
        (Method::Get, "search") => reply!(api_search(shared, &q)),
        (Method::Get, "pending") => reply!(api_pending(shared)),
        (Method::Post, "pending/resolve") => {
            let body = read_json::<ResolveRequest>(&mut req);
            reply!(body.and_then(|b| api_resolve(shared, b)))
        }
        (Method::Get, "batches") => reply!(api_batches(shared, &q)),
        (Method::Get, "batch") => reply!(api_batch(shared, &q)),
        (Method::Post, "undo") => {
            let body = read_json::<UndoRequest>(&mut req);
            reply!(body.and_then(|b| api_undo(shared, b)))
        }
        (Method::Get, "today") => reply!(api_today(shared, &q)),
        (Method::Get, "dirs") => reply!(api_dirs(shared, &q)),
        (Method::Get, "settings") => reply!(api_settings(shared)),
        (Method::Put, "settings") => {
            let body = read_json::<AgentSettings>(&mut req);
            reply!(body.and_then(|b| api_save_settings(shared, b)))
        }
        (Method::Get, "organize") => {
            let path = q.get("path").cloned().unwrap_or_default();
            reply!(api_organize(shared, &path, false).map(|(p, _)| p))
        }
        (Method::Post, "organize") => {
            let body = read_json::<OrganizeRequest>(&mut req);
            reply!(body.and_then(|b| api_organize(shared, &b.path, true).map(|(_, r)| r)))
        }
        _ => respond_json(
            req,
            404,
            &ErrorBody {
                error: format!("없는 요청입니다: {route}"),
            },
        ),
    }
}

// ── handlers ────────────────────────────────────────────────────────────

fn cfg(shared: &Shared) -> Config {
    lock(&shared.cfg).clone()
}

fn open_store(cfg: &Config) -> ApiResult<Store> {
    Store::open(&cfg.store.db_path()).map_err(internal)
}

fn open_mover(shared: &Shared, cfg: &Config) -> ApiResult<Mover> {
    Ok(Mover::new(open_store(cfg)?, cfg.mover.duplicate_policy)
        .with_expected(Arc::clone(&shared.expected)))
}

fn local_midnight() -> i64 {
    use chrono::{Local, TimeZone};
    let today = Local::now().date_naive();
    today
        .and_hms_opt(0, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn status(shared: &Shared) -> ApiResult<AgentStatus> {
    let cfg = cfg(shared);
    let info = lock(&shared.info);
    let watches = cfg
        .watches
        .iter()
        .map(|w| {
            let canon = dunce::canonicalize(&w.path).unwrap_or_else(|_| w.path.clone());
            AgentWatchState {
                path: s(&w.path),
                alias: w.alias.clone(),
                online: w.path.is_dir(),
                mode: w.mode.as_str().into(),
                organize_root: w.organize_root.as_deref().map(s),
                polling: info.polling.get(&canon).copied().unwrap_or(false),
            }
        })
        .collect();
    let (running, error) = (info.running, info.error.clone());
    drop(info);
    let store = open_store(&cfg)?;
    let today = store.moves_since(local_midnight()).map_err(internal)?;
    Ok(AgentStatus {
        version: env!("CARGO_PKG_VERSION").into(),
        data_root: s(&shared.data_root),
        watches,
        watching: running,
        error,
        pending: store.pending_count().map_err(internal)?,
        files: store.index_stats().map_err(internal)?.files,
        ocr_waiting: store.ocr_counts().map_err(internal)?.pending,
        today_moved: crate::report::today::summarize(&today, local_midnight(), 0).moved as u64,
        started_at: shared.started_at,
    })
}

fn api_search(shared: &Shared, q: &HashMap<String, String>) -> ApiResult<RemoteSearch> {
    let text = q.get("q").map(String::as_str).unwrap_or("").trim();
    let limit = q
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let query = Query::parse(text);
    if query.is_empty() {
        return Ok(RemoteSearch::default());
    }
    let cfg = cfg(shared);
    let store = open_store(&cfg)?;
    let opts = SearchOptions {
        limit,
        mark_start: MARK_START.into(),
        mark_end: MARK_END.into(),
        snippet_tokens: 24,
    };
    let hits = search(&store, &query, &opts).map_err(internal)?;
    Ok(RemoteSearch {
        hits: hits.into_iter().map(RemoteHit::from).collect(),
        note: None,
    })
}

fn api_pending(shared: &Shared) -> ApiResult<Vec<RemotePending>> {
    let store = open_store(&cfg(shared))?;
    Ok(store
        .list_pending()
        .map_err(internal)?
        .into_iter()
        .map(RemotePending::from)
        .collect())
}

fn api_resolve(shared: &Shared, b: ResolveRequest) -> ApiResult<Vec<RemoteResolution>> {
    let decision: crate::pending::Decision = b.decision.into();
    if let crate::pending::Decision::Move {
        dest_dir: Some(d), ..
    } = &decision
    {
        inside(&shared.data_root, d)?;
    }
    let _act = lock(&shared.act);
    let cfg = cfg(shared);
    let mover = open_mover(shared, &cfg)?;
    let idx = IndexOptions::from_config(&cfg);
    let batch = new_batch_id();
    Ok(b.ids
        .iter()
        .map(
            |&id| match resolve(&mover, id, &decision, &batch, Some(&idx)) {
                Ok(r) => resolution(r),
                Err(e) => RemoteResolution {
                    kind: "error".into(),
                    path: String::new(),
                    dst: None,
                    batch_id: None,
                    message: Some(e.user_message()),
                },
            },
        )
        .collect())
}

fn resolution(r: Resolution) -> RemoteResolution {
    let simple = |kind: &str, p: &Path| RemoteResolution {
        kind: kind.into(),
        path: s(p),
        dst: None,
        batch_id: None,
        message: None,
    };
    match r {
        Resolution::Moved {
            src, dst, batch_id, ..
        } => RemoteResolution {
            kind: "moved".into(),
            path: s(&src),
            dst: Some(s(&dst)),
            batch_id: Some(batch_id),
            message: None,
        },
        Resolution::Duplicate { src, existing } => RemoteResolution {
            kind: "duplicate".into(),
            path: s(&src),
            dst: Some(s(&existing)),
            batch_id: None,
            message: Some(crate::mover::duplicate_message(&existing)),
        },
        Resolution::AlreadyInPlace { path } => simple("already_in_place", &path),
        Resolution::Skipped { path } => simple("skipped", &path),
        Resolution::Ignored { path } => simple("ignored", &path),
        Resolution::Gone { path } => simple("gone", &path),
    }
}

fn api_batches(
    shared: &Shared,
    q: &HashMap<String, String>,
) -> ApiResult<Vec<crate::store::BatchSummary>> {
    let limit = q
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(100usize)
        .clamp(1, 1000);
    open_store(&cfg(shared))?.batches(limit).map_err(internal)
}

fn api_batch(
    shared: &Shared,
    q: &HashMap<String, String>,
) -> ApiResult<Vec<crate::store::MoveLogEntry>> {
    let id = q.get("id").map(String::as_str).unwrap_or("").trim();
    if id.is_empty() {
        return Err(bad("id가 없습니다"));
    }
    open_store(&cfg(shared))?
        .moves_in_batch(id)
        .map_err(internal)
}

fn api_undo(shared: &Shared, b: UndoRequest) -> ApiResult<RemoteUndo> {
    let _act = lock(&shared.act);
    let cfg = cfg(shared);
    let mover = open_mover(shared, &cfg)?;
    let report = mover
        .undo_batch(b.batch.trim())
        .map_err(|e| bad(e.to_string()))?;
    reindex_undo(mover.store(), &report, &IndexOptions::from_config(&cfg));
    Ok(RemoteUndo::from(&report))
}

fn api_today(
    shared: &Shared,
    q: &HashMap<String, String>,
) -> ApiResult<crate::report::today::TodaySummary> {
    let since = q
        .get("since")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(local_midnight);
    let entries = open_store(&cfg(shared))?
        .moves_since(since)
        .map_err(internal)?;
    Ok(crate::report::today::summarize(&entries, since, 8))
}

/// `p` must be an absolute path inside the data root, without `..`;
/// existing paths are also checked after resolving links.
fn inside(root: &Path, p: &Path) -> ApiResult<PathBuf> {
    if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(bad(format!(
            "{}: NAS 폴더 안의 절대 경로여야 합니다",
            p.display()
        )));
    }
    let resolved = match dunce::canonicalize(p) {
        Ok(c) => c,
        Err(_) => p.to_path_buf(),
    };
    if !resolved.starts_with(root) {
        return Err(bad(format!(
            "{}: {} 밖의 폴더는 쓸 수 없습니다",
            p.display(),
            root.display()
        )));
    }
    Ok(resolved)
}

fn api_dirs(shared: &Shared, q: &HashMap<String, String>) -> ApiResult<Vec<RemoteDir>> {
    let dir = match q.get("path").map(|p| p.trim()).filter(|p| !p.is_empty()) {
        Some(p) => inside(&shared.data_root, Path::new(p))?,
        None => shared.data_root.clone(),
    };
    let read = std::fs::read_dir(&dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
    let visible = |name: &str| !name.starts_with('.') && !SYSTEM_DIRS.contains(&name);
    let mut out: Vec<RemoteDir> = read
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !visible(&name) {
                return None;
            }
            let path = e.path();
            let has_children = std::fs::read_dir(&path)
                .map(|rd| {
                    rd.filter_map(Result::ok).any(|c| {
                        c.file_type().map(|t| t.is_dir()).unwrap_or(false)
                            && visible(&c.file_name().to_string_lossy())
                    })
                })
                .unwrap_or(false);
            Some(RemoteDir {
                name,
                path: s(&path),
                has_children,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(1000);
    Ok(out)
}

fn api_settings(shared: &Shared) -> ApiResult<AgentSettings> {
    let cfg = cfg(shared);
    Ok(AgentSettings {
        data_root: s(&shared.data_root),
        watches: cfg.watches.iter().map(AgentWatch::from).collect(),
        index_only: cfg.index_only.iter().map(|r| s(&r.path)).collect(),
        rules: std::fs::read_to_string(&cfg.classify.rules).unwrap_or_default(),
    })
}

fn api_save_settings(shared: &Arc<Shared>, b: AgentSettings) -> ApiResult<AgentSettings> {
    let root = &shared.data_root;
    let mut watches = Vec::new();
    for w in &b.watches {
        let mut r = w.to_root();
        r.kind = RootKind::Local;
        r.index_content = true;
        let p = inside(root, &r.path)?;
        if !p.is_dir() {
            return Err(bad(format!("{}: 폴더가 없습니다", r.path.display())));
        }
        if let Some(o) = &r.organize_root {
            inside(root, o)?;
        }
        for e in &r.exclude {
            inside(root, e)?;
        }
        watches.push(r);
    }
    let mut index_only = Vec::new();
    for p in b
        .index_only
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
    {
        let p = inside(root, Path::new(p))?;
        if !p.is_dir() {
            return Err(bad(format!("{}: 폴더가 없습니다", p.display())));
        }
        index_only.push(IndexRoot {
            path: p,
            recursive: true,
        });
    }
    RuleSet::from_toml(&b.rules).map_err(|e| bad(format!("규칙 오류: {e}")))?;

    let _act = lock(&shared.act);
    let old = cfg(shared);
    if let Some(parent) = old.classify.rules.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&old.classify.rules, &b.rules).map_err(internal)?;
    crate::config::write_folders(&shared.opts.config_path, &watches, &index_only)
        .map_err(internal)?;
    let new = Config::load(&shared.opts.config_path).map_err(internal)?;
    stop_watch(shared);
    *lock(&shared.cfg) = new;
    start_watch(shared);
    spawn_index(shared);
    tracing::info!(folders = watches.len(), "settings saved from the PC");
    api_settings(shared)
}

/// "규칙대로 지금 정리": what the rules would do with the files already in
/// watch folder `path` (preview), or do it as one batch (`apply`).
fn api_organize(
    shared: &Shared,
    path: &str,
    apply: bool,
) -> ApiResult<(OrganizePreview, OrganizeResult)> {
    let cfg = cfg(shared);
    let target = inside(&shared.data_root, Path::new(path.trim()))?;
    let w = cfg
        .watches
        .iter()
        .find(|w| dunce::canonicalize(&w.path).is_ok_and(|p| p == target))
        .ok_or_else(|| bad(format!("{path}: 감시 폴더가 아닙니다")))?;
    let rules = RuleSet::load(&cfg.classify.rules).map_err(|e| bad(e.to_string()))?;
    let filter = cfg.ignore_filter().map_err(internal)?;
    let anchors = crate::classify::Anchors::from_config(&cfg);
    let protected: Vec<PathBuf> = cfg
        .report
        .protected
        .iter()
        .map(|p| dunce::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    let plan = restructure::plan_from_rules_in(
        &target,
        &rules,
        w.recursive,
        &filter,
        &protected,
        Some(anchors.for_path(&target).unwrap_or(&target)),
    )
    .map_err(|e| bad(e.to_string()))?;
    let preview = OrganizePreview {
        path: s(&target),
        to_move: plan.moves.iter().map(|m| m.files.max(1) as usize).sum(),
        examples: plan
            .moves
            .iter()
            .take(5)
            .map(|m| (s(&m.from), s(&m.to)))
            .collect(),
    };
    if !apply {
        return Ok((preview, OrganizeResult::default()));
    }
    let _act = lock(&shared.act);
    let mover = open_mover(shared, &cfg)?;
    let plan = restructure::save(mover.store(), plan).map_err(internal)?;
    let r = restructure::apply(
        &mover,
        &plan,
        None,
        &protected,
        Some(&IndexOptions::from_config(&cfg)),
    )
    .map_err(|e| bad(e.to_string()))?;
    tracing::info!(moved = r.moved.len(), batch = %r.batch_id, "organized existing files");
    Ok((
        preview,
        OrganizeResult {
            batch_id: r.batch_id,
            moved: r.moved.len(),
            skipped: r.skipped.len(),
            errors: r
                .errors
                .iter()
                .map(|(p, e)| format!("{}: {e}", p.display()))
                .collect(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_decoding() {
        let q = parse_query("q=%EA%B2%AC%EC%A0%81+%EC%84%9C&limit=5&x");
        assert_eq!(q["q"], "견적 서");
        assert_eq!(q["limit"], "5");
        assert_eq!(q["x"], "");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%2"), "a%2");
    }

    #[test]
    fn secrets_compare() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
        assert!(!same_secret("", "a"));
    }

    #[test]
    fn paths_must_stay_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("a")).unwrap();
        assert!(inside(&root, &root.join("a")).is_ok());
        assert!(inside(&root, &root.join("new")).is_ok());
        assert!(inside(&root, &root.join("a").join("..").join("..")).is_err());
        assert!(inside(&root, Path::new("relative")).is_err());
        assert!(inside(&root, &std::env::temp_dir().join("elsewhere-xyz")).is_err());
    }

    #[test]
    fn token_file_is_made_once() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let t1 = load_or_create_token(&cfg, None).unwrap();
        assert_eq!(t1.len(), 64);
        assert_eq!(load_or_create_token(&cfg, None).unwrap(), t1);
        assert_eq!(
            load_or_create_token(&cfg, Some(" mine ".into())).unwrap(),
            "mine"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(token_path(&cfg))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
