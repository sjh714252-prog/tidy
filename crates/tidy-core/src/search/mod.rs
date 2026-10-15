//! Search (SPEC §5.6): query syntax + FTS5 lookup. Semantic search (M7)
//! will merge its results into the same `SearchHit` list.

mod fts;
mod query;

pub use fts::{lookup, search, MatchedIn, SearchHit, SearchOptions, TRIGRAM_MIN_CHARS};
pub use query::{type_exts, Query};
