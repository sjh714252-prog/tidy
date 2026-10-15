//! Indexer (SPEC §5.5): walk a root, extract text, keep `files` current.

pub mod extract;
mod indexer;

pub use extract::{ExtractError, Extractor, Extractors};
pub use indexer::{FileOutcome, IndexError, IndexOptions, IndexReport, Indexer};
