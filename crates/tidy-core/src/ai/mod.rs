//! AI layer (SPEC §5.8): provider traits, three HTTP providers
//! (Anthropic, OpenAI-compatible, Ollama), a deterministic `MockProvider`,
//! prompts, and API-key access through the OS keychain.
//!
//! AI is always optional: every caller treats an [`AiError`] as "no
//! suggestion" and falls back to rules / asking the user (CLAUDE.md rule 4).

mod anthropic;
pub mod keys;
mod mock;
mod ollama;
mod openai;
pub mod prompts;
mod provider;

pub use anthropic::AnthropicProvider;
pub use mock::MockProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use provider::{
    build_provider, estimate_cost_usd, AiError, EmbeddingProvider, LlmProvider, LlmResponse, Usage,
};
