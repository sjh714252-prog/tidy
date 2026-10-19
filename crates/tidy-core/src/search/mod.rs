//! Search (SPEC §5.6): query syntax + FTS5 lookup, and semantic search
//! (M7) merged into the same `SearchHit` list.

mod fts;
mod query;
mod semantic;

pub use fts::{lookup, search, MatchedIn, SearchHit, SearchOptions, TRIGRAM_MIN_CHARS};
pub use query::{type_exts, Query};
pub use semantic::{
    embed_text, hybrid_search, is_natural_language, EmbedReport, SearchMode, SearchOutcome,
    SemanticSearch,
};
