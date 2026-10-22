//! Deterministic provider for tests and demos (CLAUDE.md: AI tests use
//! `MockProvider` only). Answers are queued; each call pops one (the last
//! one repeats). Records every prompt it was given.

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use super::provider::{AiError, LlmProvider, LlmResponse, Usage};

type Responder = Box<dyn Fn(&str, &str) -> Value + Send + Sync>;
type Predicate = Box<dyn Fn(&str) -> bool + Send + Sync>;

pub struct MockProvider {
    answers: Mutex<Vec<Result<Value, String>>>,
    prompts: Mutex<Vec<(String, String)>>,
    /// Computes the answer from `(system, user)` instead of the queue.
    responder: Option<Responder>,
    /// When this says `true` for a user prompt, the call fails with
    /// `AiError::Truncated` (simulates `max_tokens` cut-offs).
    truncate_when: Option<Predicate>,
    /// Same, failing with `AiError::Timeout`.
    timeout_when: Option<Predicate>,
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
            responder: None,
            truncate_when: None,
            timeout_when: None,
        }
    }

    /// Answer every call with `f(system, user)` (deterministic answers that
    /// depend on what was asked, e.g. per-file batches).
    pub fn responder(f: impl Fn(&str, &str) -> Value + Send + Sync + 'static) -> Self {
        Self {
            answers: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            responder: Some(Box::new(f)),
            truncate_when: None,
            timeout_when: None,
        }
    }

    /// Fail with `AiError::Timeout` whenever `pred(user_prompt)` is true.
    pub fn timeout_when(mut self, pred: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.timeout_when = Some(Box::new(pred));
        self
    }

    /// Fail with `AiError::Truncated` whenever `pred(user_prompt)` is true.
    pub fn truncate_when(mut self, pred: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.truncate_when = Some(Box::new(pred));
        self
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
        if self.truncate_when.as_ref().is_some_and(|p| p(user)) {
            return Err(AiError::Truncated);
        }
        if self.timeout_when.as_ref().is_some_and(|p| p(user)) {
            return Err(AiError::Timeout(1));
        }
        if let Some(f) = &self.responder {
            return Ok(LlmResponse {
                value: f(system, user),
                usage: Usage {
                    input_tokens: Some(100),
                    output_tokens: Some(20),
                },
                model: "mock".into(),
            });
        }
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
