//! (M16) The PC side: talk to a NAS agent. Every call has a short timeout
//! and only reports failures — nothing on the PC waits on the NAS.

use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

use super::api::*;
use crate::report::today::TodaySummary;
use crate::store::{BatchSummary, MoveLogEntry};

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("NAS 주소가 올바르지 않습니다: {0}")]
    Url(String),
    #[error("NAS에 연결할 수 없습니다 ({0}). 주소와 NAS가 켜져 있는지 확인하세요")]
    Connect(String),
    #[error("NAS가 제때 답하지 않았습니다")]
    Timeout,
    #[error("NAS 연결 암호가 맞지 않습니다")]
    Unauthorized,
    #[error("이 주소에서 Tidy NAS 에이전트를 찾지 못했습니다 (HTTP {0})")]
    NotAgent(u16),
    #[error("{0}")]
    Agent(String),
    #[error("NAS 응답을 읽지 못했습니다: {0}")]
    Body(String),
    #[error("NAS 연결 암호가 저장되어 있지 않습니다")]
    NoToken,
}

/// Ordinary requests. Organizing a whole folder gets [`LONG_TIMEOUT`].
pub const TIMEOUT: Duration = Duration::from_secs(8);
pub const LONG_TIMEOUT: Duration = Duration::from_secs(600);

/// `192.168.0.10` → `http://192.168.0.10:7841`; keeps an explicit scheme,
/// port or path (a reverse proxy).
pub fn normalize_url(input: &str) -> Result<String, RemoteError> {
    let t = input.trim().trim_end_matches('/');
    if t.is_empty() {
        return Err(RemoteError::Url("비어 있음".into()));
    }
    let with_scheme = if t.contains("://") {
        t.to_string()
    } else {
        format!("http://{t}")
    };
    let mut url =
        reqwest::Url::parse(&with_scheme).map_err(|e| RemoteError::Url(format!("{t}: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(RemoteError::Url(t.into()));
    }
    if url.scheme() == "http" && url.port().is_none() && url.path() == "/" {
        let _ = url.set_port(Some(DEFAULT_PORT));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

#[derive(Clone)]
pub struct NasClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for NasClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the token (CLAUDE.md rule 5).
        f.debug_struct("NasClient")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl NasClient {
    pub fn new(url: &str, token: &str) -> Result<Self, RemoteError> {
        if token.trim().is_empty() {
            return Err(RemoteError::NoToken);
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .build()
            .map_err(|e| RemoteError::Connect(e.to_string()))?;
        Ok(Self {
            base: normalize_url(url)?,
            token: token.trim().to_string(),
            http,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn call<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
        timeout: Duration,
    ) -> Result<T, RemoteError> {
        let mut req = self
            .http
            .request(method, format!("{}{API}/{path}", self.base))
            .bearer_auth(&self.token)
            .timeout(timeout);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                RemoteError::Timeout
            } else {
                RemoteError::Connect(short_error(&e))
            }
        })?;
        let status = resp.status();
        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"));
        if status == StatusCode::UNAUTHORIZED && is_json {
            return Err(RemoteError::Unauthorized);
        }
        if !is_json {
            return Err(RemoteError::NotAgent(status.as_u16()));
        }
        let bytes = resp.bytes().await.map_err(|e| {
            if e.is_timeout() {
                RemoteError::Timeout
            } else {
                RemoteError::Body(e.to_string())
            }
        })?;
        if !status.is_success() {
            let msg = serde_json::from_slice::<ErrorBody>(&bytes)
                .map(|b| b.error)
                .unwrap_or_else(|_| format!("HTTP {}", status.as_u16()));
            return Err(RemoteError::Agent(msg));
        }
        serde_json::from_slice(&bytes).map_err(|e| RemoteError::Body(e.to_string()))
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, RemoteError> {
        self.call::<(), T>(Method::GET, path, query, None, TIMEOUT)
            .await
    }

    pub async fn status(&self) -> Result<AgentStatus, RemoteError> {
        self.get("status", &[]).await
    }

    pub async fn search(&self, q: &str, limit: usize) -> Result<RemoteSearch, RemoteError> {
        self.get(
            "search",
            &[("q", q.to_string()), ("limit", limit.to_string())],
        )
        .await
    }

    pub async fn pending(&self) -> Result<Vec<RemotePending>, RemoteError> {
        self.get("pending", &[]).await
    }

    pub async fn resolve(
        &self,
        ids: Vec<i64>,
        decision: RemoteDecision,
    ) -> Result<Vec<RemoteResolution>, RemoteError> {
        let body = ResolveRequest { ids, decision };
        self.call(
            Method::POST,
            "pending/resolve",
            &[],
            Some(&body),
            LONG_TIMEOUT,
        )
        .await
    }

    pub async fn batches(&self, limit: usize) -> Result<Vec<BatchSummary>, RemoteError> {
        self.get("batches", &[("limit", limit.to_string())]).await
    }

    pub async fn batch(&self, id: &str) -> Result<Vec<MoveLogEntry>, RemoteError> {
        self.get("batch", &[("id", id.to_string())]).await
    }

    pub async fn undo(&self, batch: &str) -> Result<RemoteUndo, RemoteError> {
        let body = UndoRequest {
            batch: batch.to_string(),
        };
        self.call(Method::POST, "undo", &[], Some(&body), LONG_TIMEOUT)
            .await
    }

    pub async fn today(&self, since: i64) -> Result<TodaySummary, RemoteError> {
        self.get("today", &[("since", since.to_string())]).await
    }

    /// Sub-folders of `path` (`None` = the data root).
    pub async fn dirs(&self, path: Option<&str>) -> Result<Vec<RemoteDir>, RemoteError> {
        let q: Vec<(&str, String)> = path.map(|p| ("path", p.to_string())).into_iter().collect();
        self.get("dirs", &q).await
    }

    pub async fn settings(&self) -> Result<AgentSettings, RemoteError> {
        self.get("settings", &[]).await
    }

    pub async fn save_settings(&self, s: &AgentSettings) -> Result<AgentSettings, RemoteError> {
        self.call(Method::PUT, "settings", &[], Some(s), TIMEOUT)
            .await
    }

    pub async fn organize_preview(&self, path: &str) -> Result<OrganizePreview, RemoteError> {
        self.call::<(), _>(
            Method::GET,
            "organize",
            &[("path", path.to_string())],
            None,
            LONG_TIMEOUT,
        )
        .await
    }

    pub async fn organize(&self, path: &str) -> Result<OrganizeResult, RemoteError> {
        let body = OrganizeRequest {
            path: path.to_string(),
        };
        self.call(Method::POST, "organize", &[], Some(&body), LONG_TIMEOUT)
            .await
    }
}

/// reqwest errors carry the whole URL chain; keep the root cause only.
fn short_error(e: &reqwest::Error) -> String {
    let mut src: &dyn std::error::Error = e;
    while let Some(next) = src.source() {
        src = next;
    }
    src.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(
            normalize_url("192.168.0.10").unwrap(),
            "http://192.168.0.10:7841"
        );
        assert_eq!(
            normalize_url(" nas.local:9000/ ").unwrap(),
            "http://nas.local:9000"
        );
        assert_eq!(
            normalize_url("https://nas.example.com/tidy").unwrap(),
            "https://nas.example.com/tidy"
        );
        assert_eq!(
            normalize_url("https://nas.example.com").unwrap(),
            "https://nas.example.com"
        );
        assert!(normalize_url("").is_err());
        assert!(normalize_url("ftp://nas").is_err());
    }

    #[test]
    fn debug_hides_token() {
        let c = NasClient::new("nas", "secret-token-value").unwrap();
        assert!(!format!("{c:?}").contains("secret"));
        assert!(matches!(
            NasClient::new("nas", " "),
            Err(RemoteError::NoToken)
        ));
    }
}
