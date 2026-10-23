//! M8 (SPEC §14.1): first-run wizard, visual rule editor, folder tree and
//! "AI로 규칙 만들기". Nothing here moves a file. The wizard writes
//! `config.toml` / `rules.toml` only when the user presses "감시 시작";
//! the folder tree creates folders through `tidy_core::mover::create_dir`
//! (CLAUDE.md rule 1).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tidy_core::classify::ai_rules::{draft_rules, RuleDrafts};
use tidy_core::classify::preview::{collect_files, match_files, FileSample, PreviewReport};
use tidy_core::classify::starter::{starter_cards, starters, Starter};
use tidy_core::classify::{
    append_cards, apply_cards, parse_cards, AiClassifier, RuleCard, RuleSet,
};
use tidy_core::config::{write_settings, SettingsPatch, WatchRoot};
use tidy_core::watcher::IgnoreFilter;

use crate::commands::{
    blocking, status, watches_from_dtos, CmdResult, StatusDto, WatchDto, RULES_HEADER,
};
use crate::state::AppState;
use crate::{autostart, semantic, watch, windows};

fn s(p: &Path) -> String {
    p.display().to_string()
}

// ── file sample cache ───────────────────────────────────────────────────

/// Listing the watch folders for every keystroke in the editor would be
/// slow on a big desktop: the listing is reused for a few seconds.
static SAMPLE: Mutex<Option<(String, Instant, FileSample)>> = Mutex::new(None);
const SAMPLE_TTL: Duration = Duration::from_secs(10);

fn sample_for(watches: &[WatchRoot], globs: &[String]) -> FileSample {
    // Watch `exclude` folders are part of `watches`, so the cache key
    // covers them too.
    let filter = IgnoreFilter::new(globs)
        .unwrap_or_else(|_| IgnoreFilter::new(&[]).unwrap())
        .with_dirs(
            watches
                .iter()
                .flat_map(|w| w.exclude.iter().cloned())
                .collect(),
        );
    let key = format!("{watches:?}|{globs:?}");
    let mut guard = SAMPLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, at, sample)) = guard.as_ref() {
        if *k == key && at.elapsed() < SAMPLE_TTL {
            return sample.clone();
        }
    }
    let sample = collect_files(watches, &filter);
    *guard = Some((key, Instant::now(), sample.clone()));
    sample
}

/// Forget the cached listing (after the wizard moved nothing but wrote
/// a new config, or when the user asks for fresh numbers).
fn drop_sample() {
    *SAMPLE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ── wizard ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedFolder {
    pub label: String,
    pub path: String,
    pub exists: bool,
    /// Checked by default (desktop, downloads).
    pub checked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardInfo {
    pub onboarded: bool,
    /// Folders already configured (re-run), else the usual suspects.
    pub watches: Vec<WatchDto>,
    pub suggested: Vec<SuggestedFolder>,
    pub starters: Vec<Starter>,
    /// Names of the rules already in rules.toml (re-run: they are kept).
    pub existing_rules: Vec<String>,
}

fn suggested_folders() -> Vec<SuggestedFolder> {
    let mut out = Vec::new();
    let mut push = |label: &str, p: Option<PathBuf>, checked: bool| {
        if let Some(p) = p {
            if !out.iter().any(|f: &SuggestedFolder| f.path == s(&p)) {
                out.push(SuggestedFolder {
                    label: label.into(),
                    exists: p.is_dir(),
                    path: s(&p),
                    checked: checked && p.is_dir(),
                });
            }
        }
    };
    push("바탕 화면", dirs::desktop_dir(), true);
    push("다운로드", dirs::download_dir(), true);
    push("문서", dirs::document_dir(), false);
    out
}

#[tauri::command]
pub async fn wizard_info(app: AppHandle) -> CmdResult<WizardInfo> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let existing_rules = std::fs::read_to_string(&cfg.classify.rules)
            .ok()
            .and_then(|t| parse_cards(&t).ok())
            .map(|c| c.into_iter().map(|c| c.name).collect())
            .unwrap_or_default();
        Ok(WizardInfo {
            onboarded: cfg.onboarded(),
            watches: cfg.watches.iter().map(WatchDto::from).collect(),
            suggested: suggested_folders(),
            starters: starters(),
            existing_rules,
        })
    })
    .await
}

/// Per-rule matches of `cards` over the files in `watches` (or the
/// configured folders when `watches` is `None`).
#[tauri::command]
pub async fn preview_rules(
    app: AppHandle,
    watches: Option<Vec<WatchDto>>,
    cards: Vec<RuleCard>,
    exclusive: bool,
) -> CmdResult<PreviewReport> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let watches = match watches {
            Some(w) => watches_from_dtos(&w)?,
            None => cfg.watches.clone(),
        };
        let text = apply_cards("", &cards).map_err(|e| e.to_string())?;
        let rules = RuleSet::from_toml(&text).map_err(|e| e.to_string())?;
        let sample = sample_for(&watches, &cfg.classify.ignore_globs);
        Ok(match_files(&rules, &sample, exclusive))
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishArgs {
    pub watches: Vec<WatchDto>,
    /// Starter ids the user ticked.
    pub starters: Vec<String>,
    /// "기타 직접 추가" rows.
    #[serde(default)]
    pub custom: Vec<RuleCard>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishResult {
    pub status: StatusDto,
    /// Files already in the watch folders / how many a rule would take.
    pub existing_files: usize,
    pub matching_files: usize,
    /// Folders the "미리보기" offer is for (report tab, rules plan).
    pub folders: Vec<String>,
}

/// "감시 시작": the only place the wizard writes anything. Existing rules
/// are kept; ticked starters (and custom rows) whose name is not there yet
/// are appended. Then setup counts as done and watching starts.
#[tauri::command]
pub async fn finish_onboarding(app: AppHandle, args: FinishArgs) -> CmdResult<FinishResult> {
    blocking(move || {
        let watches = watches_from_dtos(&args.watches)?;
        if watches.is_empty() {
            return Err("감시할 폴더를 하나 이상 고르세요".into());
        }
        let state = app.state::<AppState>();
        let cfg = state.config();
        let mut cards = starter_cards(&args.starters);
        for c in args.custom {
            if c.name.trim().is_empty() || c.dest.trim().is_empty() {
                continue;
            }
            cards.push(c);
        }
        let old = std::fs::read_to_string(&cfg.classify.rules).unwrap_or_default();
        let base = if old.trim().is_empty() {
            RULES_HEADER.to_string()
        } else {
            old
        };
        let text = append_cards(&base, &cards).map_err(|e| format!("규칙 오류: {e}"))?;
        let patch = SettingsPatch {
            watches: watches.clone(),
            store_path: cfg.store.path.clone(),
            quiet_mode: cfg.general.quiet_mode,
            hotkey: cfg.general.hotkey.clone(),
            start_on_login: cfg.general.start_on_login,
            onboarded: Some(true),
            check_updates: cfg.general.check_updates,
        };
        write_settings(&state.config_path, &patch).map_err(|e| e.to_string())?;
        state.reload();
        let cfg = state.config();
        RuleSet::save_text(&cfg.classify.rules, &text).map_err(|e| e.to_string())?;
        state.clear_error();
        let auto = autostart::sync(&app, cfg.general.start_on_login);
        if let Some(e) = auto.error {
            state.set_error(e);
        }
        drop_sample();
        let rules = RuleSet::from_toml(&text).map_err(|e| e.to_string())?;
        let sample = sample_for(&cfg.watches, &cfg.classify.ignore_globs);
        let report = match_files(&rules, &sample, true);
        watch::restart(&app);
        watch::spawn_index(&app);
        watch::pending_changed(&app, false);
        semantic::restart_embed(&app);
        Ok(FinishResult {
            status: status(&app),
            existing_files: report.scanned,
            matching_files: report.scanned - report.unmatched,
            folders: cfg.watches.iter().map(|w| s(&w.path)).collect(),
        })
    })
    .await
}

// ── rule editor ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorState {
    pub text: String,
    pub ok: bool,
    /// Compile / parse error (cards are read-only while there is one).
    pub error: Option<String>,
    pub cards: Option<Vec<RuleCard>>,
    /// First-match counts over the configured watch folders.
    pub preview: Option<PreviewReport>,
}

fn editor_state(app: &AppHandle, text: String, preview: bool) -> EditorState {
    let cards = parse_cards(&text);
    let (ok, error, cards) = match cards {
        Ok(c) => (true, None, Some(c)),
        Err(e) => (false, Some(e.to_string()), None),
    };
    let preview = if ok && preview {
        let cfg = app.state::<AppState>().config();
        RuleSet::from_toml(&text).ok().map(|rules| {
            let sample = sample_for(&cfg.watches, &cfg.classify.ignore_globs);
            match_files(&rules, &sample, true)
        })
    } else {
        None
    };
    EditorState {
        text,
        ok,
        error,
        cards,
        preview,
    }
}

/// TOML → cards (+ match counts). The "고급" editor calls this as the user
/// types; a file that does not parse leaves the cards read-only.
#[tauri::command]
pub async fn rules_editor(app: AppHandle, text: String, preview: bool) -> CmdResult<EditorState> {
    blocking(move || Ok(editor_state(&app, text, preview))).await
}

/// Cards → TOML (round-trip on top of `text`, comments and unknown keys
/// kept) + match counts. Nothing is saved.
#[tauri::command]
pub async fn rules_from_cards(
    app: AppHandle,
    text: String,
    cards: Vec<RuleCard>,
    preview: bool,
) -> CmdResult<EditorState> {
    blocking(move || {
        let base = if text.trim().is_empty() {
            RULES_HEADER.to_string()
        } else {
            text
        };
        let out = apply_cards(&base, &cards).map_err(|e| e.to_string())?;
        Ok(editor_state(&app, out, preview))
    })
    .await
}

/// "AI로 규칙 만들기": drafts for the configured watch folders. Nothing is
/// written; the editor shows them as cards to accept or drop.
#[tauri::command]
pub async fn ai_rule_drafts(app: AppHandle) -> CmdResult<RuleDrafts> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let rules = RuleSet::load(&cfg.classify.rules).unwrap_or_default();
        let ai = AiClassifier::from_config(&cfg, &rules).map_err(|e| e.to_string())?;
        // Own connection: never hold the UI one during a network call.
        let store =
            tidy_core::store::Store::open(&cfg.store.db_path()).map_err(|e| e.to_string())?;
        draft_rules(
            &ai,
            &store,
            &cfg.watches,
            Duration::from_secs(cfg.ai.restructure_timeout_secs),
        )
        .map_err(|e| e.to_string())
    })
    .await
}

// ── folder tree ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsRoot {
    pub label: String,
    pub path: String,
    /// drive | home | favorite | watch | anchor
    pub kind: &'static str,
}

/// Where the folder tree starts: drives, home and the usual folders, the
/// watch folders and their collect roots.
#[tauri::command]
pub async fn fs_roots(app: AppHandle) -> CmdResult<Vec<FsRoot>> {
    blocking(move || {
        let cfg = app.state::<AppState>().config();
        let mut out: Vec<FsRoot> = Vec::new();
        let mut push = |label: String, p: PathBuf, kind: &'static str| {
            if !out.iter().any(|r| r.path == s(&p)) {
                out.push(FsRoot {
                    label,
                    path: s(&p),
                    kind,
                });
            }
        };
        for w in &cfg.watches {
            let name = w
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| s(&w.path));
            push(format!("감시: {name}"), w.path.clone(), "watch");
            if let Some(r) = &w.organize_root {
                push(format!("기준: {}", s(r)), r.clone(), "anchor");
            }
        }
        if let Some(h) = dirs::home_dir() {
            push("홈".into(), h, "home");
        }
        for (label, p) in [
            ("바탕 화면", dirs::desktop_dir()),
            ("문서", dirs::document_dir()),
            ("다운로드", dirs::download_dir()),
            ("사진", dirs::picture_dir()),
        ] {
            if let Some(p) = p.filter(|p| p.is_dir()) {
                push(label.into(), p, "favorite");
            }
        }
        for d in drives() {
            push(s(&d), d, "drive");
        }
        Ok(out)
    })
    .await
}

#[cfg(windows)]
fn drives() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|c| PathBuf::from(format!("{}:\\", c as char)))
        .filter(|p| p.exists())
        .collect()
}

#[cfg(not(windows))]
fn drives() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
}

/// Sub-folders of `path` (no files, no hidden folders), read only when a
/// node is expanded. A share that does not answer within 5 s gives an
/// error instead of hanging the tree.
#[tauri::command]
pub async fn list_dirs(path: String) -> CmdResult<Vec<DirEntryDto>> {
    blocking(move || {
        let dir = tidy_core::config::expand_home(Path::new(&path));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res = (|| -> std::io::Result<Vec<DirEntryDto>> {
                let mut out = Vec::new();
                for e in std::fs::read_dir(&dir)? {
                    let Ok(e) = e else { continue };
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') || name.starts_with('$') {
                        continue;
                    }
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        out.push(DirEntryDto {
                            path: s(&e.path()),
                            name,
                        });
                    }
                }
                out.sort_by_key(|e| e.name.to_lowercase());
                Ok(out)
            })();
            let _ = tx.send(res);
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(format!("폴더를 읽을 수 없습니다: {e}")),
            Err(_) => Err("응답 없음 (네트워크 폴더가 느리거나 끊겼습니다)".into()),
        }
    })
    .await
}

/// "새 폴더": `parent/name` through the mover (the only module that changes
/// the filesystem). Returns the new folder's path.
#[tauri::command]
pub async fn make_dir(parent: String, name: String) -> CmdResult<String> {
    blocking(move || {
        tidy_core::mover::create_dir(&tidy_core::config::expand_home(Path::new(&parent)), &name)
            .map(|p| s(&p))
            .map_err(|e| e.to_string())
    })
    .await
}

/// "마법사 다시 실행".
#[tauri::command]
pub async fn open_wizard(app: AppHandle) -> CmdResult<()> {
    windows::show_wizard(&app);
    Ok(())
}
