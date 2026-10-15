use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, Row};
use thiserror::Error;

use super::models::{DecidedBy, MoveLogEntry, NewMove, PendingEntry};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cannot create database directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("path is not valid UTF-8 and cannot be logged: {0}")]
    NonUtf8Path(PathBuf),
}

/// Ordered migrations. Index + 1 = schema version after applying it.
/// Tables for later milestones (files, FTS, …) are added as new entries.
const MIGRATIONS: &[&str] = &[
    // v1 (M1): move log, pending queue, permanent ignore list.
    r#"
CREATE TABLE move_log (
  id INTEGER PRIMARY KEY,
  batch_id TEXT NOT NULL,
  src_path TEXT NOT NULL,
  dst_path TEXT NOT NULL,
  hash TEXT,
  size INTEGER,
  decided_by TEXT NOT NULL,
  rule_name TEXT,
  ai_confidence REAL,
  moved_at INTEGER NOT NULL,
  undone_at INTEGER
);
CREATE INDEX idx_move_batch ON move_log(batch_id);
CREATE INDEX idx_move_src ON move_log(src_path);

CREATE TABLE pending (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  suggested_dest TEXT,
  reason TEXT,
  confidence REAL,
  alternatives TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE ignored (
  path TEXT PRIMARY KEY,
  added_at INTEGER NOT NULL
);
"#,
];

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the database at `path` and migrate it.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        // `watch` and `undo` may hold the DB from different processes.
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")?;
        let current: i64 = tx
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| {
                r.get::<_, Option<i64>>(0)
            })?
            .unwrap_or(0);
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let version = i as i64 + 1;
            if version > current {
                tx.execute_batch(sql)?;
                tx.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [version],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?)
    }

    // ── move_log ────────────────────────────────────────────────────────

    pub fn insert_move(&self, m: &NewMove) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO move_log (batch_id, src_path, dst_path, hash, size, decided_by,
                                   rule_name, ai_confidence, moved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                m.batch_id,
                path_str(&m.src_path)?,
                path_str(&m.dst_path)?,
                m.hash,
                m.size.map(|s| s as i64),
                m.decided_by.as_str(),
                m.rule_name,
                m.ai_confidence,
                now(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Remove a log row whose move never happened (the mover writes the row
    /// first, then moves; on failure the intent is withdrawn).
    pub(crate) fn withdraw_move(&self, id: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM move_log WHERE id = ?1 AND undone_at IS NULL",
            [id],
        )?;
        Ok(())
    }

    pub(crate) fn mark_undone(&self, id: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE move_log SET undone_at = ?1 WHERE id = ?2",
            params![now(), id],
        )?;
        Ok(())
    }

    pub fn get_move(&self, id: i64) -> Result<Option<MoveLogEntry>, StoreError> {
        Ok(self
            .conn
            .query_row(&format!("{SELECT_MOVE} WHERE id = ?1"), [id], move_from_row)
            .optional()?)
    }

    /// Batch of the most recent move that has not been undone.
    pub fn last_active_batch(&self) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT batch_id FROM move_log WHERE undone_at IS NULL ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// All rows of a batch, newest first (the order undo must replay them).
    pub fn moves_in_batch(&self, batch_id: &str) -> Result<Vec<MoveLogEntry>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_MOVE} WHERE batch_id = ?1 ORDER BY id DESC"
        ))?;
        let rows = stmt.query_map([batch_id], move_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn recent_moves(&self, limit: usize) -> Result<Vec<MoveLogEntry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_MOVE} ORDER BY id DESC LIMIT ?1"))?;
        let rows = stmt.query_map([limit as i64], move_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// True if a move of exactly this content out of `src` was undone. The
    /// watcher uses it to leave files restored by `tidy undo` in place even
    /// when the undo ran in another process.
    pub fn was_undone(&self, src: &Path, hash: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM move_log
                 WHERE src_path = ?1 AND hash = ?2 AND undone_at IS NOT NULL LIMIT 1",
                params![path_str(src)?, hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Hashes of undone moves out of `src` (cheap pre-check before hashing).
    pub fn undone_hashes(&self, src: &Path) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT hash FROM move_log
             WHERE src_path = ?1 AND undone_at IS NOT NULL AND hash IS NOT NULL",
        )?;
        let rows = stmt.query_map([path_str(src)?], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Full batch id from a unique prefix (as printed by `tidy log`).
    pub fn resolve_batch(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT batch_id FROM move_log WHERE substr(batch_id, 1, length(?1)) = ?1",
        )?;
        let rows = stmt.query_map([prefix], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Drop log rows older than `retention_days` (SPEC §5.4). Only database
    /// rows are removed; files are never touched.
    pub fn prune_move_log(&self, retention_days: u32) -> Result<usize, StoreError> {
        let cutoff = now() - i64::from(retention_days) * 86_400;
        Ok(self
            .conn
            .execute("DELETE FROM move_log WHERE moved_at < ?1", [cutoff])?)
    }

    // ── pending ─────────────────────────────────────────────────────────

    pub fn upsert_pending(
        &self,
        path: &Path,
        suggested_dest: Option<&Path>,
        reason: Option<&str>,
    ) -> Result<(), StoreError> {
        let dest = suggested_dest.map(path_str).transpose()?;
        self.conn.execute(
            "INSERT INTO pending (path, suggested_dest, reason, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET suggested_dest = excluded.suggested_dest,
                                             reason = excluded.reason",
            params![path_str(path)?, dest, reason, now()],
        )?;
        Ok(())
    }

    pub fn list_pending(&self) -> Result<Vec<PendingEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, suggested_dest, reason, created_at FROM pending ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PendingEntry {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                suggested_dest: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
                reason: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn remove_pending(&self, path: &Path) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM pending WHERE path = ?1", [path_str(path)?])?;
        Ok(())
    }

    // ── ignored ─────────────────────────────────────────────────────────

    pub fn is_ignored(&self, path: &Path) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM ignored WHERE path = ?1",
                [path_str(path)?],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

const SELECT_MOVE: &str = "SELECT id, batch_id, src_path, dst_path, hash, size, decided_by, \
                           rule_name, ai_confidence, moved_at, undone_at FROM move_log";

fn move_from_row(r: &Row<'_>) -> rusqlite::Result<MoveLogEntry> {
    let decided: String = r.get(6)?;
    Ok(MoveLogEntry {
        id: r.get(0)?,
        batch_id: r.get(1)?,
        src_path: PathBuf::from(r.get::<_, String>(2)?),
        dst_path: PathBuf::from(r.get::<_, String>(3)?),
        hash: r.get(4)?,
        size: r.get::<_, Option<i64>>(5)?.map(|s| s as u64),
        decided_by: DecidedBy::parse(&decided).unwrap_or(DecidedBy::User),
        rule_name: r.get(7)?,
        ai_confidence: r.get(8)?,
        moved_at: r.get(9)?,
        undone_at: r.get(10)?,
    })
}

/// Paths are stored as UTF-8 text. A non-UTF-8 path cannot be logged
/// losslessly, so it cannot be moved either (no move without undo).
fn path_str(p: &Path) -> Result<&str, StoreError> {
    p.to_str()
        .ok_or_else(|| StoreError::NonUtf8Path(p.to_path_buf()))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(batch: &str, src: &str, dst: &str) -> NewMove {
        NewMove {
            batch_id: batch.into(),
            src_path: PathBuf::from(src),
            dst_path: PathBuf::from(dst),
            hash: Some("h".into()),
            size: Some(3),
            decided_by: DecidedBy::Rule,
            rule_name: Some("r".into()),
            ai_confidence: None,
        }
    }

    #[test]
    fn migrates_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("데이터 폴더").join("tidy.db");
        let s = Store::open(&db).unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len() as i64);
        drop(s);
        let s = Store::open(&db).unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn last_active_batch_skips_undone() {
        let s = Store::open_in_memory().unwrap();
        let a = s.insert_move(&sample("A", "/a/1", "/b/1")).unwrap();
        let b = s.insert_move(&sample("B", "/a/2", "/b/2")).unwrap();
        assert_eq!(s.last_active_batch().unwrap().as_deref(), Some("B"));
        s.mark_undone(b).unwrap();
        assert_eq!(s.last_active_batch().unwrap().as_deref(), Some("A"));
        s.mark_undone(a).unwrap();
        assert_eq!(s.last_active_batch().unwrap(), None);
        assert!(s.was_undone(Path::new("/a/1"), "h").unwrap());
        assert!(!s.was_undone(Path::new("/a/1"), "other").unwrap());
    }

    #[test]
    fn korean_paths_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let src = r"C:\Users\홍 길동\바탕 화면\보고서 최종.hwp";
        let id = s
            .insert_move(&sample("K", src, r"D:\정리\보고서 최종.hwp"))
            .unwrap();
        let e = s.get_move(id).unwrap().unwrap();
        assert_eq!(e.src_path, PathBuf::from(src));
    }
}
