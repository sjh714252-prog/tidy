//! AI layer (SPEC §5.8): provider traits, three HTTP providers
//! (Anthropic, OpenAI-compatible, Ollama), embedding providers (M7), a
//! deterministic `MockProvider` / `MockEmbedder`, a failure counter,
//! prompts, and API-key access through the OS keychain.
//!
//! AI is always optional: every caller treats an [`AiError`] as "no
//! suggestion" and falls back to rules / asking the user (CLAUDE.md rule 4).

mod anthropic;
mod embed;
pub mod health;
pub mod keys;
mod mock;
mod ollama;
mod openai;
pub mod prompts;
mod provider;

pub use anthropic::AnthropicProvider;
pub use embed::{build_embedder, Embeddings, MockEmbedder, OllamaEmbedder, OpenAiEmbedder};
pub use health::{AiAvailability, AiHealth};
pub use mock::MockProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use provider::{
    build_provider, estimate_cost_usd, AiError, EmbeddingProvider, ImageInput, LlmProvider,
    LlmResponse, Usage,
};
