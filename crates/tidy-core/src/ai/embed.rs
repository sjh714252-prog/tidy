//! Embedding providers (semantic search, SPEC §5.6 / §5.8).
//!
//! * OpenAI-compatible `POST {base}/embeddings` (OpenAI, LM Studio, vLLM,
//!   llama.cpp server, most gateways).
//! * Ollama `POST {base}/api/embed` (local, nothing leaves the machine).
//! * Anthropic has no embeddings endpoint: configuring it for `embed` is
//!   reported as [`AiError::Unsupported`] and search stays FTS-only.
//! * [`MockEmbedder`]: deterministic vectors for tests (CLAUDE.md: AI tests
//!   use mocks only).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::provider::{http_client, post_json, AiError, EmbeddingProvider, Usage};
use crate::config::{ProviderConfig, ProviderKind};

/// Vectors for each input text, in input order.
#[derive(Debug, Clone, PartialEq)]
pub struct Embeddings {
    pub vectors: Vec<Vec<f32>>,
    pub usage: Usage,
}

/// Build the embedding provider for a profile's `embed` entry. The API key
/// (if any) is read from the OS keychain here and kept only in memory.
pub fn build_embedder(
    cfg: &ProviderConfig,
    timeout: Duration,
    max_retries: u32,
) -> Result<Arc<dyn EmbeddingProvider>, AiError> {
    if cfg.provider == ProviderKind::Anthropic {
        return Err(AiError::Unsupported(
            "Anthropic API에는 임베딩 모델이 없습니다 — embed에는 openai 호환 또는 ollama를 지정하세요"
                .into(),
        ));
    }
    let key = match &cfg.key_ref {
        Some(r) => Some(super::keys::get_key(r)?.ok_or_else(|| AiError::MissingKey(r.clone()))?),
        None => None,
    };
    let http = http_client(timeout)?;
    Ok(match cfg.provider {
        ProviderKind::Openai => Arc::new(OpenAiEmbedder::new(
            http,
            key,
            cfg.model.clone(),
            cfg.base_url.clone(),
            max_retries,
        )),
        ProviderKind::Ollama => Arc::new(OllamaEmbedder::new(
            http,
            cfg.model.clone(),
            cfg.base_url.clone(),
            max_retries,
        )),
        ProviderKind::Anthropic => unreachable!("rejected above"),
    })
}

fn check_count(got: usize, want: usize) -> Result<(), AiError> {
    if got == want {
        Ok(())
    } else {
        Err(AiError::BadResponse(format!(
            "{got} vectors for {want} inputs"
        )))
    }
}

fn parse_vector(v: &Value) -> Result<Vec<f32>, AiError> {
    let arr = v
        .as_array()
        .ok_or_else(|| AiError::BadResponse("embedding is not an array".into()))?;
    let out: Vec<f32> = arr
        .iter()
        .map(|x| x.as_f64().map(|f| f as f32))
        .collect::<Option<_>>()
        .ok_or_else(|| AiError::BadResponse("embedding has non-numbers".into()))?;
    if out.is_empty() {
        return Err(AiError::BadResponse("empty embedding".into()));
    }
    Ok(out)
}

// ── OpenAI-compatible ───────────────────────────────────────────────────

pub struct OpenAiEmbedder {
    http: reqwest::Client,
    api_key: Option<String>,
    model: String,
    base_url: String,
    max_retries: u32,
}

impl std::fmt::Debug for OpenAiEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiEmbedder")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl OpenAiEmbedder {
    pub fn new(
        http: reqwest::Client,
        api_key: Option<String>,
        model: String,
        base_url: Option<String>,
        max_retries: u32,
    ) -> Self {
        Self {
            http,
            api_key,
            model,
            base_url: base_url
                .unwrap_or_else(|| "https://api.openai.com/v1".into())
                .trim_end_matches('/')
                .to_string(),
            max_retries,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Embeddings, AiError> {
        let url = format!("{}/embeddings", self.base_url);
        let body = json!({ "model": self.model, "input": texts });
        let v = post_json("openai", self.max_retries, || {
            let mut req = self.http.post(&url).json(&body);
            if let Some(k) = &self.api_key {
                req = req.bearer_auth(k);
            }
            req
        })
        .await?;
        let data = v
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| AiError::BadResponse("no data".into()))?;
        check_count(data.len(), texts.len())?;
        // `index` gives the input position; most servers keep the order
        // anyway, but the API does not promise it.
        let mut vectors = vec![Vec::new(); texts.len()];
        for (i, item) in data.iter().enumerate() {
            let idx = item
                .get("index")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(i);
            let slot = vectors
                .get_mut(idx)
                .ok_or_else(|| AiError::BadResponse(format!("index {idx} out of range")))?;
            *slot = parse_vector(item.get("embedding").unwrap_or(&Value::Null))?;
        }
        if vectors.iter().any(Vec::is_empty) {
            return Err(AiError::BadResponse("missing embedding".into()));
        }
        Ok(Embeddings {
            vectors,
            usage: Usage {
                input_tokens: v.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
                output_tokens: None,
            },
        })
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }
}

// ── Ollama ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct OllamaEmbedder {
    http: reqwest::Client,
    model: String,
    base_url: String,
    max_retries: u32,
}

impl OllamaEmbedder {
    pub fn new(
        http: reqwest::Client,
        model: String,
        base_url: Option<String>,
        max_retries: u32,
    ) -> Self {
        Self {
            http,
            model,
            base_url: base_url
                .unwrap_or_else(|| "http://localhost:11434".into())
                .trim_end_matches('/')
                .to_string(),
            max_retries,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Embeddings, AiError> {
        let url = format!("{}/api/embed", self.base_url);
        // `truncate` (default true) cuts inputs at the model's context.
        let body = json!({ "model": self.model, "input": texts });
        let v = post_json("ollama", self.max_retries, || {
            self.http.post(&url).json(&body)
        })
        .await?;
        let list = v
            .get("embeddings")
            .and_then(Value::as_array)
            .ok_or_else(|| AiError::BadResponse("no embeddings".into()))?;
        check_count(list.len(), texts.len())?;
        Ok(Embeddings {
            vectors: list.iter().map(parse_vector).collect::<Result<_, _>>()?,
            usage: Usage {
                input_tokens: v.get("prompt_eval_count").and_then(Value::as_u64),
                output_tokens: None,
            },
        })
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }
}

// ── Mock ────────────────────────────────────────────────────────────────

/// Deterministic bag-of-features embedder: words and character trigrams
/// are hashed into `dim` buckets. Words of one synonym group share a
/// reserved bucket, which is how tests stand in for "meaning" (e.g. `청구서` ≈
/// `invoice`). Can be switched into a failing state.
pub struct MockEmbedder {
    dim: usize,
    synonyms: Vec<Vec<String>>,
    failing: AtomicBool,
    calls: AtomicUsize,
    texts: AtomicUsize,
}

impl MockEmbedder {
    pub fn new(dim: usize) -> Self {
        Self {
            dim: dim.max(8),
            synonyms: Vec::new(),
            failing: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            texts: AtomicUsize::new(0),
        }
    }

    pub fn with_synonyms(mut self, groups: &[&[&str]]) -> Self {
        self.synonyms = groups
            .iter()
            .map(|g| g.iter().map(|w| w.to_lowercase()).collect())
            .collect();
        self
    }

    /// While `true`, every call fails with a network error.
    pub fn set_failing(&self, on: bool) {
        self.failing.store(on, Ordering::SeqCst);
    }

    /// Number of `embed` calls / texts embedded so far.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn texts_embedded(&self) -> usize {
        self.texts.load(Ordering::SeqCst)
    }

    pub fn vector(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        let lower = text.to_lowercase();
        for word in lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            // A synonym group puts all of its words (and any word that
            // contains one, e.g. `청구서를`) on the same strong feature.
            if let Some(g) = self
                .synonyms
                .iter()
                .position(|g| g.iter().any(|s| word.contains(s.as_str())))
            {
                v[g % self.dim] += 4.0;
                continue;
            }
            v[self.bucket(&format!("w:{word}"))] += 1.0;
            let chars: Vec<char> = word.chars().collect();
            for t in chars.windows(3) {
                let t: String = t.iter().collect();
                v[self.bucket(&format!("t:{t}"))] += 0.5;
            }
        }
        v
    }

    /// Hashed feature bucket; the first buckets are reserved for the
    /// synonym groups so words never collide with a "meaning".
    fn bucket(&self, feature: &str) -> usize {
        // FNV-1a: stable across runs and platforms.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in feature.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let reserved = self.synonyms.len().min(self.dim / 2);
        reserved + (h % (self.dim - reserved) as u64) as usize
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Embeddings, AiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return Err(AiError::Network {
                provider: "mock",
                message: "mock embedder is offline".into(),
            });
        }
        self.texts.fetch_add(texts.len(), Ordering::SeqCst);
        Ok(Embeddings {
            vectors: texts.iter().map(|t| self.vector(t)).collect(),
            usage: Usage::default(),
        })
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-embed"
    }
}

#[cfg(test)]
mod tests {
    use super::super::provider::test_server;
    use super::*;

    #[tokio::test]
    async fn openai_embeddings_in_input_order() {
        let (base, seen) = test_server::serve(vec![test_server::Canned {
            status: 200,
            headers: vec![],
            // Out of order on purpose.
            body: r#"{"data":[{"index":1,"embedding":[0.0,1.0]},{"index":0,"embedding":[1.0,0.0]}],
                     "usage":{"prompt_tokens":7}}"#
                .into(),
        }])
        .await;
        let p = OpenAiEmbedder::new(
            http_client(Duration::from_secs(5)).unwrap(),
            Some("sk-not-real".into()),
            "text-embedding-3-small".into(),
            Some(format!("{base}/v1")),
            0,
        );
        let r = p.embed(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(r.vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(r.usage.input_tokens, Some(7));
        let req = &seen.lock().unwrap()[0];
        assert!(req.starts_with("POST /v1/embeddings"));
        assert!(req.contains("\"input\":[\"a\",\"b\"]"));
        assert!(format!("{p:?}").contains("<redacted>"));
    }

    #[tokio::test]
    async fn ollama_embed_and_count_mismatch() {
        let (base, seen) = test_server::serve(vec![
            test_server::Canned {
                status: 200,
                headers: vec![],
                body: r#"{"model":"nomic-embed-text","embeddings":[[0.5,0.5,0.0]],"prompt_eval_count":3}"#
                    .into(),
            },
            test_server::Canned {
                status: 200,
                headers: vec![],
                body: r#"{"embeddings":[]}"#.into(),
            },
        ])
        .await;
        let p = OllamaEmbedder::new(
            http_client(Duration::from_secs(5)).unwrap(),
            "nomic-embed-text".into(),
            Some(base),
            0,
        );
        let r = p.embed(&["x".into()]).await.unwrap();
        assert_eq!(r.vectors, vec![vec![0.5, 0.5, 0.0]]);
        assert!(seen.lock().unwrap()[0].starts_with("POST /api/embed"));
        let e = p.embed(&["x".into()]).await.unwrap_err();
        assert!(matches!(e, AiError::BadResponse(_)), "{e}");
    }

    #[test]
    fn anthropic_is_unsupported_for_embeddings() {
        let cfg = ProviderConfig {
            provider: ProviderKind::Anthropic,
            model: "claude-haiku-5-5".into(),
            key_ref: None,
            base_url: None,
            effort: None,
        };
        let e = build_embedder(&cfg, Duration::from_secs(1), 0)
            .err()
            .unwrap();
        assert!(matches!(e, AiError::Unsupported(_)));
    }

    #[tokio::test]
    async fn mock_is_deterministic_and_can_fail() {
        let m = MockEmbedder::new(64).with_synonyms(&[&["청구서", "invoice"]]);
        let a = m.vector("KT 청구서");
        assert_eq!(a, m.vector("KT 청구서"));
        let cos = |x: &[f32], y: &[f32]| {
            let d: f32 = x.iter().zip(y).map(|(a, b)| a * b).sum();
            d / (x.iter().map(|a| a * a).sum::<f32>().sqrt()
                * y.iter().map(|a| a * a).sum::<f32>().sqrt())
        };
        assert!(cos(&a, &m.vector("kt invoice")) > 0.9);
        m.set_failing(true);
        assert!(m.embed(&["x".into()]).await.is_err());
        m.set_failing(false);
        assert_eq!(m.embed(&["x".into()]).await.unwrap().vectors.len(), 1);
        assert_eq!(m.calls(), 2);
    }
}
