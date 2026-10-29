//! Store (SPEC §6). SQLite in WAL mode so `tidy watch` and `tidy undo`
//! can run as separate processes against the same database.

mod curation;
mod db;
mod files;
mod learn;
mod models;
mod ocr;
mod plans;
mod vectors;
mod vision;

pub use db::{Store, StoreError, NETWORK_PATH_HINT};
pub use models::{
    AiLogEntry, DecidedBy, Decision, DecisionAction, FileRecord, FileStamp, IndexStats,
    MoveLogEntry, MoveOp, NewFile, NewMove, NewPending, OcrState, PendingEntry, PendingSource,
    TextStatus, VisionState,
};

pub(crate) use db::now as now_unix;
pub(crate) use files::file_from_row;
pub use learn::NewDecision;
pub use ocr::{OcrCandidate, OcrCounts};
pub use plans::{BatchSummary, StoredPlan};
pub use vectors::{EmbedCandidate, VectorRow, VectorStats};
pub use vision::{
    compose_body, strip_caption, Caption, VisionCandidate, VisionCounts, CAPTION_MARK,
};
