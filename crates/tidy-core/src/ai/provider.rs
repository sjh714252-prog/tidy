use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::config::{ProviderConfig, ProviderKind};

#[derive(Debug, Error)]
pub enum AiError {
    #[error("AI is disabled in the config")]
    Disabled,
    #[error("AI profile \"{0}\" not found or has no classify model")]
    NoProfile(String),
    #[error("AI profile \"{0}\" has no embed model (semantic search is off)")]
    NoEmbedModel(String),
    #[error("AI paused after repeated failures; retrying later ({0})")]
    Paused(String),
    #[error("no API key stored in the OS keychain for \"{0}\" (run `tidy ai key set {0}`)")]
    MissingKey(String),
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("{provider} returned HTTP {status}: {message}")]
    Http {
        provider: &'static str,
        status: u16,
        message: String,
    },
    #[error("network error talking to {provider}: {message}")]
    Network {
        provider: &'static str,
        message: String,
    },
    #[error("AI request timed out after {0} s")]
    Timeout(u64),
    #[error("the model declined the request (refusal)")]
    Refusal,
    #[error("unusable AI response: {0}")]
    BadResponse(String),
    /// The model hit `max_tokens` before finishing the JSON. Callers that
    /// batch files split the batch and retry (curate); others report it.
    #[error("AI 응답이 너무 길어 잘렸습니다 (max_tokens) — 파일을 나눠 다시 시도하세요")]
    Truncated,
    #[error("could not start the AI runtime: {0}")]
    Runtime(String),
    #[error("not supported: {0}")]
    Unsupported(String),
}

/// Token usage reported by the provider (missing fields stay `None`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LlmResponse {
    /// Parsed JSON answer (already checked to be an object).
    pub value: Value,
    pub usage: Usage,
    /// Model that answered (as reported by the provider when available).
    pub model: String,
}

/// A chat model that answers with one JSON object matching `schema`.
/// `max_tokens` caps the answer (thinking included where the model thinks);
/// providers without such a knob ignore it.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
        max_tokens: u32,
    ) -> Result<LlmResponse, AiError>;
    /// "anthropic" | "openai" | "ollama" | "mock".
    fn name(&self) -> &str;
    fn model(&self) -> &str;
}

/// Embedding model (semantic search, SPEC §5.6). Implementations:
/// OpenAI-compatible, Ollama and the test mock (see `ai::embed`). The
/// dimension is whatever the model returns; vectors of one model are only
/// ever compared with each other.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// One vector per text, in input order.
    async fn embed(&self, texts: &[String]) -> Result<super::Embeddings, AiError>;
    /// "openai" | "ollama" | "mock".
    fn name(&self) -> &str;
    fn model(&self) -> &str;
}

/// Build the provider for a profile entry. The API key (if any) is read
/// from the OS keychain here and kept only in memory.
pub fn build_provider(
    cfg: &ProviderConfig,
    timeout: Duration,
    max_retries: u32,
) -> Result<Arc<dyn LlmProvider>, AiError> {
    let key = match &cfg.key_ref {
        Some(r) => Some(super::keys::get_key(r)?.ok_or_else(|| AiError::MissingKey(r.clone()))?),
        None => None,
    };
    let http = http_client(timeout)?;
    Ok(match cfg.provider {
        ProviderKind::Anthropic => Arc::new(super::AnthropicProvider::new(
            http,
            key.ok_or_else(|| AiError::MissingKey("anthropic: key_ref is required".into()))?,
            cfg.model.clone(),
            cfg.base_url.clone(),
            cfg.effort.clone(),
            max_retries,
        )),
        ProviderKind::Openai => Arc::new(super::OpenAiProvider::new(
            http,
            key,
            cfg.model.clone(),
            cfg.base_url.clone(),
            max_retries,
        )),
        ProviderKind::Ollama => Arc::new(super::OllamaProvider::new(
            http,
            cfg.model.clone(),
            cfg.base_url.clone(),
            max_retries,
        )),
    })
}

/// The HTTP client's own timeout is only a safety net: each call is
/// bounded by its `AiCall::timeout` (classify 20 s, curate batches and
/// restructure minutes). Using the short classify timeout here cut off
/// every long answer with "request timed out".
pub(crate) const HTTP_CEILING: Duration = Duration::from_secs(600);

pub(crate) fn http_client(timeout: Duration) -> Result<reqwest::Client, AiError> {
    reqwest::Client::builder()
        .timeout(timeout.max(HTTP_CEILING))
        .connect_timeout(Duration::from_secs(10).min(timeout))
        .user_agent(concat!("tidy/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AiError::Runtime(e.to_string()))
}

/// POST `body` as JSON, retrying 408/409/429/5xx (incl. 529 overloaded)
/// and network errors with exponential backoff (honouring `retry-after`,
/// capped at 8 s). Returns the status and parsed JSON body of the final
/// answer; non-retryable HTTP errors are returned as [`AiError::Http`].
/// `build` is called once per attempt so headers (keys) are never stored.
pub(crate) async fn post_json(
    provider: &'static str,
    max_retries: u32,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<Value, AiError> {
    let mut attempt = 0u32;
    loop {
        let res = build().send().await;
        let wait = match res {
            Ok(resp) => {
                let status = resp.status();
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<f64>().ok());
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    return serde_json::from_str(&text)
                        .map_err(|e| AiError::BadResponse(format!("invalid JSON body: {e}")));
                }
                let retryable =
                    matches!(status.as_u16(), 408 | 409 | 429) || status.is_server_error();
                let err = AiError::Http {
                    provider,
                    status: status.as_u16(),
                    message: error_message(&text),
                };
                if !retryable || attempt >= max_retries {
                    return Err(err);
                }
                tracing::info!(
                    provider,
                    status = status.as_u16(),
                    attempt,
                    "retrying AI request"
                );
                retry_after
                    .map(|s| Duration::from_secs_f64(s.clamp(0.0, 8.0)))
                    .unwrap_or_else(|| backoff(attempt))
            }
            Err(e) => {
                let err = if e.is_timeout() {
                    AiError::Network {
                        provider,
                        message: "request timed out".into(),
                    }
                } else {
                    AiError::Network {
                        provider,
                        // Without the URL: it may carry a query-string key on
                        // some OpenAI-compatible gateways.
                        message: e.without_url().to_string(),
                    }
                };
                if attempt >= max_retries {
                    return Err(err);
                }
                backoff(attempt)
            }
        };
        tokio::time::sleep(wait).await;
        attempt += 1;
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500u64.saturating_mul(1 << attempt.min(4)))
}

/// Human-readable message from the usual error body shapes:
/// Anthropic `{"error":{"message"}}`, OpenAI `{"error":{"message"}}`,
/// Ollama `{"error":"..."}`.
fn error_message(body: &str) -> String {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return body.chars().take(300).collect(),
    };
    v.pointer("/error/message")
        .or_else(|| v.get("error"))
        .or_else(|| v.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| body.chars().take(300).collect())
}

/// Parse a model's text answer as one JSON object (tolerating a fenced
/// ```json block some local models add).
pub(crate) fn parse_json_object(text: &str) -> Result<Value, AiError> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map(|r| r.trim_end().trim_end_matches("```"))
        .unwrap_or(t)
        .trim();
    let v: Value = serde_json::from_str(t)
        .map_err(|e| AiError::BadResponse(format!("answer is not JSON: {e}")))?;
    if v.is_object() {
        Ok(v)
    } else {
        Err(AiError::BadResponse("answer is not a JSON object".into()))
    }
}

/// USD per million tokens (input, output) for known hosted models.
/// Local (Ollama) models cost nothing; unknown models return `None`.
pub fn estimate_cost_usd(provider: &str, model: &str, usage: Usage) -> Option<f64> {
    let (inp, out) = match provider {
        "ollama" | "mock" => return Some(0.0),
        "anthropic" => match model {
            "claude-haiku-5-5" => (0.10, 0.50),
            "claude-haiku-4-5" => (1.0, 5.0),
            "claude-sonnet-5-5" | "claude-sonnet-5" => (2.0, 10.0),
            "claude-sonnet-4-6" => (3.0, 15.0),
            "claude-opus-5-5" => (4.0, 20.0),
            "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
                (5.0, 25.0)
            }
            "claude-fable-5-1" | "claude-fable-5" => (10.0, 50.0),
            _ => return None,
        },
        "openai" => match model {
            "text-embedding-3-small" => (0.02, 0.0),
            "text-embedding-3-large" => (0.13, 0.0),
            _ => return None,
        },
        _ => return None,
    };
    let i = usage.input_tokens.unwrap_or(0) as f64;
    let o = usage.output_tokens.unwrap_or(0) as f64;
    Some((i * inp + o * out) / 1_000_000.0)
}

#[cfg(test)]
pub(crate) mod test_server {
    //! A tiny HTTP server for provider tests: answers each connection with
    //! the next canned response and records the requests. No real API is
    //! ever contacted (CLAUDE.md: AI tests use mocks only).

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub struct Canned {
        pub status: u16,
        pub headers: Vec<(&'static str, &'static str)>,
        pub body: String,
    }

    pub async fn serve(responses: Vec<Canned>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            for r in responses {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                // Read headers, then Content-Length bytes of body.
                loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(end) = find(&buf, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                seen2
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).into_owned());
                let mut out = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                    r.status,
                    r.body.len()
                );
                for (k, v) in &r.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                out.push_str(&r.body);
                sock.write_all(out.as_bytes()).await.unwrap();
                let _ = sock.shutdown().await;
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn find(h: &[u8], n: &[u8]) -> Option<usize> {
        h.windows(n.len()).position(|w| w == n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slow answer (longer than the classify timeout the client was
    /// built with) must still arrive: the per-call `AiCall::timeout` bounds
    /// calls, the client's own timeout is only a 10-minute ceiling. This was
    /// the "network error talking to anthropic: request timed out" on every
    /// 30-file curate batch.
    #[tokio::test]
    async fn client_timeout_is_a_ceiling_not_the_classify_timeout() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut tmp = [0u8; 8192];
            let _ = sock.read(&mut tmp).await;
            tokio::time::sleep(Duration::from_millis(2500)).await;
            let body = r#"{"ok":true}"#;
            let out = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(out.as_bytes()).await.unwrap();
        });
        let http = http_client(Duration::from_secs(1)).unwrap();
        let v = post_json("test", 0, || http.post(&url).body("{}"))
            .await
            .expect("slow answer within the ceiling is not a timeout");
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn parses_plain_and_fenced_json() {
        assert!(parse_json_object(r#"{"a":1}"#).is_ok());
        assert!(parse_json_object("```json\n{\"a\":1}\n```").is_ok());
        assert!(parse_json_object("[1]").is_err());
        assert!(parse_json_object("sure! here").is_err());
    }

    #[test]
    fn error_messages_from_known_shapes() {
        assert_eq!(
            error_message(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
            ),
            "Overloaded"
        );
        assert_eq!(
            error_message(r#"{"error":"model not found"}"#),
            "model not found"
        );
        assert_eq!(error_message("plain"), "plain");
    }

    #[test]
    fn costs() {
        let u = Usage {
            input_tokens: Some(1_000_000),
            output_tokens: Some(1_000_000),
        };
        assert_eq!(
            estimate_cost_usd("anthropic", "claude-haiku-5-5", u),
            Some(0.6)
        );
        assert_eq!(estimate_cost_usd("ollama", "qwen3:8b", u), Some(0.0));
        assert_eq!(estimate_cost_usd("openai", "whatever", u), None);
    }
}
