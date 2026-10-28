//! Indexer (SPEC §5.5): walk a root, extract text, keep `files` current.

pub mod extract;
mod indexer;
pub mod ocr;

pub use extract::{hwp_testgen, ExtractError, Extractor, Extractors};
pub use indexer::{
    jobs_from_config, FileOutcome, IndexError, IndexOptions, IndexReport, Indexer, NetworkRoot,
};
