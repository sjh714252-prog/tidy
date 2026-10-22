//! OpenAI-compatible `POST {base}/chat/completions` (OpenAI and the many
//! vendors / local servers that speak the same API). Asks for a strict
//! `json_schema` response; servers that reject that get one retry with the
//! older `json_object` mode and the schema in the prompt.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::provider::{parse_json_object, post_json, AiError, LlmProvider, LlmResponse, Usage};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiProvider {
    http: reqwest::Client,
    api_key: Option<String>,
    model: String,
    base_url: String,
    max_retries: u32,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl OpenAiProvider {
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
                .unwrap_or_else(|| DEFAULT_BASE_URL.into())
                .trim_end_matches('/')
                .to_string(),
            max_retries,
        }
    }

    fn body(&self, system: &str, user: &str, schema: &Value, strict: bool) -> Value {
        if strict {
            json!({
                "model": self.model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user }
                ],
                "response_format": {
                    "type": "json_schema",
                    "json_schema": { "name": "tidy_answer", "strict": true, "schema": schema }
                }
            })
        } else {
            json!({
                "model": self.model,
                "messages": [
                    { "role": "system", "content": format!(
                        "{system}\n\nRespond with one JSON object matching this JSON Schema:\n{schema}") },
                    { "role": "user", "content": user }
                ],
                "response_format": { "type": "json_object" }
            })
        }
    }

    async fn send(&self, body: &Value) -> Result<Value, AiError> {
        let url = format!("{}/chat/completions", self.base_url);
        post_json("openai", self.max_retries, || {
            let mut req = self.http.post(&url).json(body);
            if let Some(k) = &self.api_key {
                req = req.bearer_auth(k);
            }
            req
        })
        .await
    }
}

fn parse_response(v: &Value) -> Result<(String, Usage, Option<String>), AiError> {
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| AiError::BadResponse("no choices".into()))?;
    if choice
        .pointer("/message/refusal")
        .and_then(Value::as_str)
        .is_some()
    {
        return Err(AiError::Refusal);
    }
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        return Err(AiError::Truncated);
    }
    let text = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let usage = Usage {
        input_tokens: v.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
        output_tokens: v
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64),
    };
    Ok((
        text,
        usage,
        v.get("model").and_then(Value::as_str).map(str::to_string),
    ))
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
        max_tokens: u32,
    ) -> Result<LlmResponse, AiError> {
        // Not forwarded: OpenAI-compatible servers disagree on the field
        // name (max_tokens / max_completion_tokens) and reject unknown ones.
        let _ = max_tokens;
        let v = match self.send(&self.body(system, user, schema, true)).await {
            // Server without json_schema support: fall back once.
            Err(AiError::Http {
                status: 400 | 422, ..
            }) => self.send(&self.body(system, user, schema, false)).await?,
            other => other?,
        };
        let (text, usage, model) = parse_response(&v)?;
        Ok(LlmResponse {
            value: parse_json_object(&text)?,
            usage,
            model: model.unwrap_or_else(|| self.model.clone()),
        })
    }

    fn name(&self) -> &str {
        "openai"
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
    async fn falls_back_to_json_object_mode() {
        let ok = r#"{"model":"local","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"{\"dest\":\"/a\"}"}}],
                    "usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let (base, seen) = test_server::serve(vec![
            test_server::Canned {
                status: 400,
                headers: vec![],
                body: r#"{"error":{"message":"response_format json_schema not supported"}}"#.into(),
            },
            test_server::Canned {
                status: 200,
                headers: vec![],
                body: ok.into(),
            },
        ])
        .await;
        let p = OpenAiProvider::new(
            http_client(Duration::from_secs(5)).unwrap(),
            Some("sk-not-real".into()),
            "local".into(),
            Some(format!("{base}/v1")),
            0,
        );
        let r = p
            .complete_json("s", "u", &json!({"type":"object"}), 256)
            .await
            .unwrap();
        assert_eq!(r.value["dest"], "/a");
        assert_eq!(r.usage.output_tokens, Some(5));
        let reqs = seen.lock().unwrap();
        assert!(reqs[0].contains("json_schema"));
        assert!(reqs[1].contains("json_object"));
        assert!(reqs[1].starts_with("POST /v1/chat/completions"));
        assert!(reqs[1]
            .to_lowercase()
            .contains("authorization: bearer sk-not-real"));
    }
}
