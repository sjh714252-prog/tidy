//! Ollama (local) `POST {base}/api/chat` with `format` = JSON schema.
//! No key, no data leaves the machine.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::provider::{parse_json_object, post_json, AiError, LlmProvider, LlmResponse, Usage};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

#[derive(Debug)]
pub struct OllamaProvider {
    http: reqwest::Client,
    model: String,
    base_url: String,
    max_retries: u32,
}

impl OllamaProvider {
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
                .unwrap_or_else(|| DEFAULT_BASE_URL.into())
                .trim_end_matches('/')
                .to_string(),
            max_retries,
        }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
        max_tokens: u32,
    ) -> Result<LlmResponse, AiError> {
        let body = json!({
            "model": self.model,
            "stream": false,
            "format": schema,
            "options": { "temperature": 0, "num_predict": max_tokens },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ]
        });
        let url = format!("{}/api/chat", self.base_url);
        let v = post_json("ollama", self.max_retries, || {
            self.http.post(&url).json(&body)
        })
        .await?;
        let text = v
            .pointer("/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(LlmResponse {
            value: parse_json_object(text)?,
            usage: Usage {
                input_tokens: v.get("prompt_eval_count").and_then(Value::as_u64),
                output_tokens: v.get("eval_count").and_then(Value::as_u64),
            },
            model: self.model.clone(),
        })
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::super::provider::{http_client, test_server};
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn chat_with_schema_format() {
        let (base, seen) = test_server::serve(vec![test_server::Canned {
            status: 200,
            headers: vec![],
            body:
                r#"{"model":"qwen3:8b","message":{"role":"assistant","content":"{\"dest\":\"/b\"}"},
                      "done":true,"prompt_eval_count":300,"eval_count":20}"#
                    .into(),
        }])
        .await;
        let p = OllamaProvider::new(
            http_client(Duration::from_secs(5)).unwrap(),
            "qwen3:8b".into(),
            Some(base),
            0,
        );
        let r = p
            .complete_json(
                "s",
                "u",
                &json!({"type":"object","properties":{"dest":{"type":"string"}}}),
                256,
            )
            .await
            .unwrap();
        assert_eq!(r.value["dest"], "/b");
        assert_eq!(r.usage.input_tokens, Some(300));
        let req = &seen.lock().unwrap()[0];
        assert!(req.starts_with("POST /api/chat"));
        assert!(req.contains("\"format\":{"));
    }

    #[tokio::test]
    async fn unreachable_server_is_a_network_error() {
        // Port 9 (discard) on localhost: nothing listens in CI.
        let p = OllamaProvider::new(
            http_client(Duration::from_secs(2)).unwrap(),
            "m".into(),
            Some("http://127.0.0.1:9".into()),
            0,
        );
        let e = p
            .complete_json("s", "u", &json!({}), 256)
            .await
            .unwrap_err();
        assert!(matches!(e, AiError::Network { .. }), "{e}");
    }
}
