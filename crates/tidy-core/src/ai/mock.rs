//! Deterministic provider for tests and demos (CLAUDE.md: AI tests use
//! `MockProvider` only). Answers are queued; each call pops one (the last
//! one repeats). Records every prompt it was given.

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use super::provider::{AiError, LlmProvider, LlmResponse, Usage};

pub struct MockProvider {
    answers: Mutex<Vec<Result<Value, String>>>,
    prompts: Mutex<Vec<(String, String)>>,
}

impl MockProvider {
    /// Always answer with `value`.
    pub fn new(value: Value) -> Self {
        Self::sequence(vec![Ok(value)])
    }

    /// Answer in order; `Err(msg)` simulates a provider failure.
    pub fn sequence(answers: Vec<Result<Value, String>>) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().rev().collect()),
            prompts: Mutex::new(Vec::new()),
        }
    }

    /// `(system, user)` prompts received so far.
    pub fn prompts(&self) -> Vec<(String, String)> {
        self.prompts.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn complete_json(
        &self,
        system: &str,
        user: &str,
        _schema: &Value,
        _max_tokens: u32,
    ) -> Result<LlmResponse, AiError> {
        self.prompts
            .lock()
            .unwrap()
            .push((system.to_string(), user.to_string()));
        let mut answers = self.answers.lock().unwrap();
        let next = if answers.len() > 1 {
            answers.pop()
        } else {
            answers.last().cloned()
        };
        match next {
            Some(Ok(value)) => Ok(LlmResponse {
                value,
                usage: Usage {
                    input_tokens: Some(100),
                    output_tokens: Some(20),
                },
                model: "mock".into(),
            }),
            Some(Err(msg)) => Err(AiError::Network {
                provider: "mock",
                message: msg,
            }),
            None => Err(AiError::BadResponse("mock has no answers".into())),
        }
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock"
    }
}
