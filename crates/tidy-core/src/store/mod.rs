//! Store (SPEC §6). SQLite in WAL mode so `tidy watch` and `tidy undo`
//! can run as separate processes against the same database.

mod db;
mod files;
mod models;

pub use db::{Store, StoreError, NETWORK_PATH_HINT};
pub use models::{
    DecidedBy, FileRecord, FileStamp, IndexStats, MoveLogEntry, NewFile, NewMove, PendingEntry,
    TextStatus,
};

pub(crate) use files::file_from_row;
