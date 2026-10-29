//! Anthropic Messages API (`POST /v1/messages`) over raw HTTP — there is no
//! official Rust SDK. JSON-only answers via structured outputs
//! (`output_config.format = json_schema`).

use async_trait::async_trait;
use serde_json::{json, Value};

use super::provider::{
    parse_json_object, post_json, AiError, ImageInput, LlmProvider, LlmResponse, Usage,
};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
    effort: Option<String>,
    max_retries: u32,
}

impl std::fmt::Debug for AnthropicProvider {
    // Never print the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl AnthropicProvider {
    pub fn new(
        http: reqwest::Client,
        api_key: String,
        model: String,
        base_url: Option<String>,
        effort: Option<String>,
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
            effort,
            max_retries,
        }
    }

    fn body(
        &self,
        system: &str,
        user: &str,
        image: Option<&ImageInput>,
        schema: &Value,
        max_tokens: u32,
    ) -> Value {
        let content = match image {
            None => json!(user),
            Some(img) => json!([
                { "type": "image", "source": {
                    "type": "base64", "media_type": img.media_type, "data": img.base64 } },
                { "type": "text", "text": user }
            ]),
        };
        let mut output_config = json!({ "format": { "type": "json_schema", "schema": schema } });
        if let Some(e) = &self.effort {
            output_config["effort"] = json!(e);
        }
        json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{ "role": "user", "content": content }],
            "output_config": output_config,
        })
    }
}

/// Text of the `text` blocks (thinking blocks are skipped).
fn parse_response(v: &Value) -> Result<(String, Usage, Option<String>), AiError> {
    match v.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => return Err(AiError::Refusal),
        Some("max_tokens") => return Err(AiError::Truncated),
        _ => {}
    }
    let text: String = v
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err(AiError::BadResponse("no text in the answer".into()));
    }
    let usage = Usage {
        input_tokens: v.pointer("/usage/input_tokens").and_then(Value::as_u64),
        output_tokens: v.pointer("/usage/output_tokens").and_then(Value::as_u64),
    };
    let model = v.get("model").and_then(Value::as_str).map(str::to_string);
    Ok((text, usage, model))
}

impl AnthropicProvider {
    async fn complete(
        &self,
        system: &str,
        user: &str,
        image: Option<&ImageInput>,
        schema: &Value,
        max_tokens: u32,
    ) -> Result<LlmResponse, AiError> {
        let body = self.body(system, user, image, schema, max_tokens);
        let url = format!("{}/v1/messages", self.base_url);
        let v = post_json("anthropic", self.max_retries, || {
            self.http
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .json(&body)
        })
        .await?;
        let (text, usage, model) = parse_response(&v)?;
        Ok(LlmResponse {
            value: parse_json_object(&text)?,
            usage,
            model: model.unwrap_or_else(|| self.model.clone()),
        })
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
        max_tokens: u32,
    ) -> Result<LlmResponse, AiError> {
        self.complete(system, user, None, schema, max_tokens).await
    }

    async fn complete_json_image(
        &self,
        system: &str,
        user: &str,
        image: &ImageInput,
        schema: &Value,
        max_tokens: u32,
    ) -> Result<LlmResponse, AiError> {
        self.complete(system, user, Some(image), schema, max_tokens)
            .await
    }

    fn name(&self) -> &str {
        "anthropic"
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

    fn provider(base: String) -> AnthropicProvider {
        AnthropicProvider::new(
            http_client(Duration::from_secs(5)).unwrap(),
            "sk-test-not-real".into(),
            "claude-haiku-5-5".into(),
            Some(base),
            Some("low".into()),
            2,
        )
    }

    const OK: &str = r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-5-5",
        "content":[{"type":"thinking","thinking":""},{"type":"text","text":"{\"dest\":\"~/문서\",\"confidence\":0.8,\"reason\":\"r\",\"new_folder\":false,\"suggested_rename\":null}"}],
        "stop_reason":"end_turn","usage":{"input_tokens":812,"output_tokens":64}}"#;

    #[test]
    fn request_shape() {
        let p = provider("http://x".into());
        let b = p.body("sys", "user", None, &json!({"type":"object"}), 2048);
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["system"], "sys");
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        assert_eq!(b["output_config"]["effort"], "low");
        assert_eq!(b["max_tokens"], 2048);
        assert!(b.get("temperature").is_none());
        assert!(!format!("{p:?}").contains("sk-test"));
    }

    #[test]
    fn picture_goes_before_the_text() {
        let p = provider("http://x".into());
        let img = ImageInput {
            media_type: "image/jpeg",
            base64: "QUJD".into(),
        };
        let b = p.body("sys", "user", Some(&img), &json!({}), 1024);
        let c = &b["messages"][0]["content"];
        assert_eq!(c[0]["type"], "image");
        assert_eq!(c[0]["source"]["type"], "base64");
        assert_eq!(c[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(c[0]["source"]["data"], "QUJD");
        assert_eq!(c[1]["text"], "user");
        assert!(!format!("{img:?}").contains("QUJD"));
    }

    #[tokio::test]
    async fn retries_overloaded_then_parses() {
        let (base, seen) = test_server::serve(vec![
            test_server::Canned {
                status: 529,
                headers: vec![("retry-after", "0")],
                body:
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
                        .into(),
            },
            test_server::Canned {
                status: 200,
                headers: vec![],
                body: OK.into(),
            },
        ])
        .await;
        let r = provider(base)
            .complete_json("s", "u", &json!({"type":"object"}), 256)
            .await
            .unwrap();
        assert_eq!(r.value["confidence"], 0.8);
        assert_eq!(r.usage.input_tokens, Some(812));
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert!(reqs[1].starts_with("POST /v1/messages"));
        assert!(reqs[1]
            .to_lowercase()
            .contains("anthropic-version: 2023-06-01"));
    }

    #[tokio::test]
    async fn client_errors_and_refusals_are_not_retried() {
        let (base, seen) = test_server::serve(vec![test_server::Canned {
            status: 400,
            headers: vec![],
            body:
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#
                    .into(),
        }])
        .await;
        let e = provider(base)
            .complete_json("s", "u", &json!({}), 256)
            .await
            .unwrap_err();
        assert!(matches!(e, AiError::Http { status: 400, .. }), "{e}");
        assert!(e.to_string().contains("bad model"));
        assert_eq!(seen.lock().unwrap().len(), 1);

        let refusal = json!({"stop_reason":"refusal","content":[]});
        assert!(matches!(parse_response(&refusal), Err(AiError::Refusal)));
    }
}
