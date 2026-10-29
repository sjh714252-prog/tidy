//! (M16, SPEC §14.13) The PC side of NAS agents: connect, show their state,
//! search / queue / history / settings over HTTP. The NAS does the moving;
//! nothing here touches a file. A NAS that is off or unreachable only shows
//! up as an error string — every other part of the app keeps working.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tidy_core::config::{write_nas, NasCfg, PathMapping};
use tidy_core::remote::api::*;
use tidy_core::remote::{map, normalize_url, setup, NasClient, RemoteError};
use tidy_core::store::{BatchSummary, MoveLogEntry};

use crate::commands::{blocking, CmdResult};
use crate::state::AppState;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn configured(app: &AppHandle) -> Vec<NasCfg> {
    app.state::<AppState>().config().nas
}

fn find(app: &AppHandle, name: &str) -> CmdResult<NasCfg> {
    configured(app)
        .into_iter()
        .find(|n| n.name == name)
        .ok_or_else(|| format!("'{name}' NAS가 설정에 없습니다"))
}

/// Keychain reads may talk D-Bus: off the async thread.
async fn token_for(key_ref: String) -> CmdResult<Option<String>> {
    blocking(move || tidy_core::ai::keys::get_key(&key_ref).map_err(err)).await
}

async fn client(nas: &NasCfg) -> CmdResult<NasClient> {
    let token = token_for(nas.token_ref())
        .await?
        .ok_or_else(|| RemoteError::NoToken.to_string())?;
    NasClient::new(&nas.url, &token).map_err(err)
}

async fn client_named(app: &AppHandle, name: &str) -> CmdResult<(NasCfg, NasClient)> {
    let nas = find(app, name)?;
    let c = client(&nas).await?;
    Ok((nas, c))
}

/// Write `[[nas]]` and update the in-memory config (no full reload).
fn save_list(app: &AppHandle, list: Vec<NasCfg>) -> CmdResult<()> {
    let state = app.state::<AppState>();
    write_nas(&state.config_path, &list).map_err(err)?;
    state.lock_inner().cfg.nas = list;
    Ok(())
}

fn local(nas: &NasCfg, remote: &str) -> Option<String> {
    map::to_local(&nas.map, remote).map(|p| p.display().to_string())
}

/// Fill in a PC path for every watch / search folder that has none yet.
fn add_guessed_maps(nas: &mut NasCfg, data_root: &str, folders: &[String]) {
    for f in folders {
        if map::to_local(&nas.map, f).is_some() {
            continue;
        }
        if let Some(g) = map::guess(&nas.url, data_root, f) {
            if !nas.map.iter().any(|m| m.remote == g.remote) {
                nas.map.push(g);
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NasStateDto {
    pub name: String,
    pub url: String,
    pub maps: Vec<PathMapping>,
    pub has_token: bool,
    /// The agent answered; `status` holds what it said.
    pub ok: bool,
    pub error: Option<String>,
    pub status: Option<AgentStatus>,
}

async fn state_of(nas: NasCfg) -> NasStateDto {
    let token = token_for(nas.token_ref()).await.ok().flatten();
    let has_token = token.is_some();
    let res = match token {
        None => Err(RemoteError::NoToken),
        Some(t) => match NasClient::new(&nas.url, &t) {
            Ok(c) => c.status().await,
            Err(e) => Err(e),
        },
    };
    let (ok, error, status) = match res {
        Ok(s) => (true, None, Some(s)),
        Err(e) => (false, Some(e.to_string()), None),
    };
    NasStateDto {
        name: nas.name,
        url: nas.url,
        maps: nas.map,
        has_token,
        ok,
        error,
        status,
    }
}

/// Every configured NAS with its live state (asked in parallel).
#[tauri::command]
pub async fn nas_list(app: AppHandle) -> CmdResult<Vec<NasStateDto>> {
    let list = configured(&app);
    let tasks: Vec<_> = list
        .into_iter()
        .map(|n| tauri::async_runtime::spawn(state_of(n)))
        .collect();
    let mut out = Vec::new();
    for t in tasks {
        out.push(t.await.map_err(err)?);
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeDto {
    pub text: String,
    /// Where the file was saved (Downloads), if it could be.
    pub saved: Option<String>,
}

/// "설치 파일 만들기": a new password into the keychain (`tidy/nas/<name>`)
/// and a docker-compose.yml carrying it, saved to Downloads.
#[tauri::command]
pub async fn nas_compose(name: String, shares: String, volume: String) -> CmdResult<ComposeDto> {
    let name = clean_name(&name)?;
    blocking(move || {
        let token = tidy_core::remote::server::new_token();
        let key_ref = NasCfg {
            name: name.clone(),
            ..Default::default()
        }
        .token_ref();
        tidy_core::ai::keys::set_key(&key_ref, &token).map_err(err)?;
        let text = setup::compose_file(&token, &setup::parse_shares(&shares), &volume);
        let saved = dirs::download_dir().and_then(|d| {
            let p = d.join("tidy-nas-docker-compose.yml");
            std::fs::write(&p, &text)
                .ok()
                .map(|_| p.display().to_string())
        });
        Ok(ComposeDto { text, saved })
    })
    .await
}

fn clean_name(name: &str) -> CmdResult<String> {
    let n = name.trim();
    if n.is_empty() {
        return Ok("NAS".into());
    }
    if n.contains(['/', '\\']) {
        return Err("이름에 / 나 \\ 는 쓸 수 없습니다".into());
    }
    Ok(n.to_string())
}

/// "연결 확인": try `url` with `token` (or the stored one); on success
/// save the NAS (the token to the keychain only) and guess PC paths.
#[tauri::command]
pub async fn nas_connect(
    app: AppHandle,
    name: String,
    url: String,
    token: Option<String>,
) -> CmdResult<NasStateDto> {
    let name = clean_name(&name)?;
    let url = normalize_url(&url).map_err(err)?;
    let mut nas = configured(&app)
        .into_iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| NasCfg {
            name: name.clone(),
            ..Default::default()
        });
    nas.url = url.clone();
    let token = match token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        Some(t) => t,
        None => token_for(nas.token_ref())
            .await?
            .ok_or("연결 암호를 넣어 주세요 (설치 파일을 만들었다면 저절로 들어갑니다)")?,
    };
    let c = NasClient::new(&url, &token).map_err(err)?;
    let status = c.status().await.map_err(err)?;
    let key_ref = nas.token_ref();
    blocking(move || tidy_core::ai::keys::set_key(&key_ref, &token).map_err(err)).await?;
    let folders: Vec<String> = status.watches.iter().map(|w| w.path.clone()).collect();
    add_guessed_maps(&mut nas, &status.data_root, &folders);
    let mut list = configured(&app);
    match list.iter_mut().find(|n| n.name == name) {
        Some(slot) => *slot = nas.clone(),
        None => list.push(nas.clone()),
    }
    save_list(&app, list)?;
    Ok(state_of(nas).await)
}

/// "연결 끊기": forget the NAS here (the agent keeps running on the NAS).
#[tauri::command]
pub async fn nas_remove(app: AppHandle, name: String) -> CmdResult<()> {
    let list: Vec<NasCfg> = configured(&app)
        .into_iter()
        .filter(|n| n.name != name)
        .collect();
    if let Ok(nas) = find(&app, &name) {
        let key_ref = nas.token_ref();
        let _ = blocking(move || tidy_core::ai::keys::delete_key(&key_ref).map_err(err)).await;
    }
    save_list(&app, list)
}

#[tauri::command]
pub async fn nas_set_maps(app: AppHandle, name: String, maps: Vec<PathMapping>) -> CmdResult<()> {
    let mut list = configured(&app);
    let nas = list
        .iter_mut()
        .find(|n| n.name == name)
        .ok_or("NAS가 설정에 없습니다")?;
    nas.map = maps
        .into_iter()
        .filter(|m| !m.remote.trim().is_empty() && !m.local.trim().is_empty())
        .collect();
    save_list(&app, list)
}

#[tauri::command]
pub async fn nas_settings(app: AppHandle, name: String) -> CmdResult<AgentSettings> {
    let (_, c) = client_named(&app, &name).await?;
    c.settings().await.map_err(err)
}

#[tauri::command]
pub async fn nas_save_settings(
    app: AppHandle,
    name: String,
    settings: AgentSettings,
) -> CmdResult<AgentSettings> {
    let (mut nas, c) = client_named(&app, &name).await?;
    let saved = c.save_settings(&settings).await.map_err(err)?;
    let folders: Vec<String> = saved
        .watches
        .iter()
        .map(|w| w.path.clone())
        .chain(saved.index_only.iter().cloned())
        .collect();
    let before = nas.map.len();
    add_guessed_maps(&mut nas, &saved.data_root, &folders);
    if nas.map.len() != before {
        let mut list = configured(&app);
        if let Some(slot) = list.iter_mut().find(|n| n.name == name) {
            slot.map = nas.map;
        }
        save_list(&app, list)?;
    }
    Ok(saved)
}

#[tauri::command]
pub async fn nas_dirs(
    app: AppHandle,
    name: String,
    path: Option<String>,
) -> CmdResult<Vec<RemoteDir>> {
    let (_, c) = client_named(&app, &name).await?;
    c.dirs(path.as_deref()).await.map_err(err)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NasHitDto {
    pub nas: String,
    /// The same file as this PC sees it (open / reveal), if mapped.
    pub local_path: Option<String>,
    #[serde(flatten)]
    pub hit: RemoteHit,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NasSearchDto {
    pub hits: Vec<NasHitDto>,
    /// "NAS: 응답 없음" per NAS that failed.
    pub errors: Vec<String>,
}

/// Search every connected NAS (in parallel, short timeout). Called next
/// to the PC's own search, never instead of it.
#[tauri::command]
pub async fn nas_search(
    app: AppHandle,
    query: String,
    limit: Option<usize>,
) -> CmdResult<NasSearchDto> {
    let list = configured(&app);
    let limit = limit.unwrap_or(30);
    let mut tasks = Vec::new();
    for nas in list {
        let query = query.clone();
        tasks.push(tauri::async_runtime::spawn(async move {
            let res = match client(&nas).await {
                Ok(c) => c.search(&query, limit).await.map_err(err),
                Err(e) => Err(e),
            };
            (nas, res)
        }));
    }
    let mut out = NasSearchDto {
        hits: Vec::new(),
        errors: Vec::new(),
    };
    for t in tasks {
        let (nas, res) = t.await.map_err(err)?;
        match res {
            Ok(r) => out.hits.extend(r.hits.into_iter().map(|h| NasHitDto {
                nas: nas.name.clone(),
                local_path: local(&nas, &h.path),
                hit: h,
            })),
            Err(e) => out.errors.push(format!("{}: {e}", nas.name)),
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NasPendingDto {
    pub nas: String,
    pub local_path: Option<String>,
    #[serde(flatten)]
    pub item: RemotePending,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NasPendingListDto {
    pub items: Vec<NasPendingDto>,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn nas_pending(app: AppHandle) -> CmdResult<NasPendingListDto> {
    let mut out = NasPendingListDto {
        items: Vec::new(),
        errors: Vec::new(),
    };
    for nas in configured(&app) {
        let res = match client(&nas).await {
            Ok(c) => c.pending().await.map_err(err),
            Err(e) => Err(e),
        };
        match res {
            Ok(items) => out.items.extend(items.into_iter().map(|p| NasPendingDto {
                nas: nas.name.clone(),
                local_path: local(&nas, &p.path),
                item: p,
            })),
            Err(e) => out.errors.push(format!("{}: {e}", nas.name)),
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn nas_resolve(
    app: AppHandle,
    name: String,
    ids: Vec<i64>,
    decision: RemoteDecision,
) -> CmdResult<Vec<RemoteResolution>> {
    let (_, c) = client_named(&app, &name).await?;
    c.resolve(ids, decision).await.map_err(err)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NasBatchDto {
    pub nas: String,
    #[serde(flatten)]
    pub batch: BatchSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NasBatchListDto {
    pub batches: Vec<NasBatchDto>,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn nas_batches(app: AppHandle, limit: Option<usize>) -> CmdResult<NasBatchListDto> {
    let mut out = NasBatchListDto {
        batches: Vec::new(),
        errors: Vec::new(),
    };
    for nas in configured(&app) {
        let res = match client(&nas).await {
            Ok(c) => c.batches(limit.unwrap_or(50)).await.map_err(err),
            Err(e) => Err(e),
        };
        match res {
            Ok(b) => out.batches.extend(b.into_iter().map(|batch| NasBatchDto {
                nas: nas.name.clone(),
                batch,
            })),
            Err(e) => out.errors.push(format!("{}: {e}", nas.name)),
        }
    }
    out.batches
        .sort_by_key(|b| std::cmp::Reverse(b.batch.last_at));
    Ok(out)
}

#[tauri::command]
pub async fn nas_batch_moves(
    app: AppHandle,
    name: String,
    batch_id: String,
) -> CmdResult<Vec<MoveLogEntry>> {
    let (_, c) = client_named(&app, &name).await?;
    c.batch(&batch_id).await.map_err(err)
}

#[tauri::command]
pub async fn nas_undo(app: AppHandle, name: String, batch_id: String) -> CmdResult<RemoteUndo> {
    let (_, c) = client_named(&app, &name).await?;
    c.undo(&batch_id).await.map_err(err)
}

#[tauri::command]
pub async fn nas_organize_preview(
    app: AppHandle,
    name: String,
    path: String,
) -> CmdResult<OrganizePreview> {
    let (_, c) = client_named(&app, &name).await?;
    c.organize_preview(&path).await.map_err(err)
}

/// Moves files on the NAS: only from the explicit "정리" confirmation.
#[tauri::command]
pub async fn nas_organize(app: AppHandle, name: String, path: String) -> CmdResult<OrganizeResult> {
    let (_, c) = client_named(&app, &name).await?;
    c.organize(&path).await.map_err(err)
}
