//! Store (SPEC §6). SQLite in WAL mode so `tidy watch` and `tidy undo`
//! can run as separate processes against the same database.

mod db;
mod models;

pub use db::{Store, StoreError};
pub use models::{DecidedBy, MoveLogEntry, NewMove, PendingEntry};
