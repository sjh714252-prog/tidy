use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, Row};
use thiserror::Error;

use super::models::{DecidedBy, MoveLogEntry, NewMove, NewPending, PendingEntry, PendingSource};

/// Hint appended to SQLite disk I/O errors. SQLite needs real file locks,
/// which SMB/NFS shares and some mounted folders do not provide, so a
/// `store.path` on a network share fails with `disk I/O error` (PR #1).
pub const NETWORK_PATH_HINT: &str = "store.path가 네트워크 경로(NAS/공유 폴더/마운트)가 \
     아닌지 확인하세요. DB는 반드시 로컬 디스크에 두어야 합니다";

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {source}{}", io_hint(source))]
    Sqlite {
        #[from]
        source: rusqlite::Error,
    },
    #[error("cannot open database {path}: {source} — {NETWORK_PATH_HINT}")]
    DiskIo {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("cannot create database directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("path is not valid UTF-8 and cannot be logged: {0}")]
    NonUtf8Path(PathBuf),
    #[error("not an indexed root: {0}")]
    UnknownRoot(PathBuf),
}

fn is_disk_io(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::SystemIoFailure
    )
}

fn io_hint(e: &rusqlite::Error) -> String {
    if is_disk_io(e) {
        format!(" — {NETWORK_PATH_HINT}")
    } else {
        String::new()
    }
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
    // v2 (M2): index roots, indexed files, full-text search.
    //
    // `files.content` holds the extracted text and is the *external content*
    // of both FTS tables (SPEC §6): FTS5 then only stores the index, and
    // `snippet()`/`highlight()` read the text back from `files`. Two
    // tokenizers (docs/fts-tokenizer.md): `trigram` is the primary one
    // (substring matching inside Korean compounds, no spaces needed) and
    // the `unicode61` table only serves 1–2 character terms, which trigram
    // cannot match at all.
    r#"
CREATE TABLE roots (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  watch INTEGER NOT NULL,
  index_content INTEGER NOT NULL,
  poll_interval_s INTEGER
);

CREATE TABLE files (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  ext TEXT,
  mime TEXT,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  ctime INTEGER,
  hash TEXT,
  text_status TEXT NOT NULL DEFAULT 'none',
  excerpt TEXT,
  content TEXT,
  indexed_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL
);
CREATE INDEX idx_files_hash ON files(hash);
CREATE INDEX idx_files_ext ON files(ext);
CREATE INDEX idx_files_mtime ON files(mtime);
CREATE INDEX idx_files_root_seen ON files(root_id, last_seen_at);

CREATE VIRTUAL TABLE files_fts USING fts5(
  name, content,
  content='files', content_rowid='id',
  tokenize='trigram'
);
CREATE VIRTUAL TABLE files_fts_word USING fts5(
  name, content,
  content='files', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER files_ai AFTER INSERT ON files BEGIN
  INSERT INTO files_fts(rowid, name, content) VALUES (new.id, new.name, new.content);
  INSERT INTO files_fts_word(rowid, name, content) VALUES (new.id, new.name, new.content);
END;
CREATE TRIGGER files_ad AFTER DELETE ON files BEGIN
  INSERT INTO files_fts(files_fts, rowid, name, content)
    VALUES ('delete', old.id, old.name, old.content);
  INSERT INTO files_fts_word(files_fts_word, rowid, name, content)
    VALUES ('delete', old.id, old.name, old.content);
END;
CREATE TRIGGER files_au AFTER UPDATE OF name, content ON files BEGIN
  INSERT INTO files_fts(files_fts, rowid, name, content)
    VALUES ('delete', old.id, old.name, old.content);
  INSERT INTO files_fts_word(files_fts_word, rowid, name, content)
    VALUES ('delete', old.id, old.name, old.content);
  INSERT INTO files_fts(rowid, name, content) VALUES (new.id, new.name, new.content);
  INSERT INTO files_fts_word(rowid, name, content) VALUES (new.id, new.name, new.content);
END;
"#,
    // v3 (M4): AI suggestions in the queue, user decisions (learner), AI call log.
    r#"
ALTER TABLE pending ADD COLUMN source TEXT NOT NULL DEFAULT 'rule';
ALTER TABLE pending ADD COLUMN suggested_name TEXT;

CREATE TABLE decisions (
  id INTEGER PRIMARY KEY,
  file_name TEXT NOT NULL,
  ext TEXT,
  src_dir TEXT NOT NULL,
  suggested_dest TEXT,
  chosen_dest TEXT,
  action TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'rule',
  decided_at INTEGER NOT NULL
);
CREATE INDEX idx_decisions_ext ON decisions(ext);

CREATE TABLE ai_log (
  id INTEGER PRIMARY KEY,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  purpose TEXT NOT NULL,
  input_tokens INTEGER,
  output_tokens INTEGER,
  cost_usd REAL,
  latency_ms INTEGER,
  ok INTEGER NOT NULL,
  error TEXT,
  result TEXT,
  called_at INTEGER NOT NULL
);

CREATE TABLE learn_dismissed (
  key TEXT PRIMARY KEY,
  dismissed_at INTEGER NOT NULL
);
"#,
    // v4 (M5): restructure plans (proposal → user approval → one batch).
    r#"
CREATE TABLE restructure_plans (
  id INTEGER PRIMARY KEY,
  root TEXT NOT NULL,
  source TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  plan_json TEXT NOT NULL,
  applied_batch TEXT,
  applied_at INTEGER
);
"#,
    // v5 (M6): moves are written as 'moving' and marked 'done' once the
    // file is in place, so a copy interrupted by a crash can be recovered.
    r#"
ALTER TABLE move_log ADD COLUMN state TEXT NOT NULL DEFAULT 'done';
CREATE INDEX idx_move_dst ON move_log(dst_path);
CREATE INDEX idx_move_state ON move_log(state);
"#,
    // v6 (M7): embeddings for semantic search. Vectors are stored once per
    // (model, text) — `text_hash` of what was embedded — and files point
    // at them, so a moved or copied file (same name and text) reuses its
    // vector instead of paying for another API call. `model` is the
    // embedder key (provider/model[#variant]); vectors of different keys
    // are never compared. `vec` is little-endian f32, L2-normalized.
    // `file_vectors.id` only grows (AUTOINCREMENT) so the in-memory search
    // cache can load just the rows added since it was built.
    r#"
CREATE TABLE vectors (
  id INTEGER PRIMARY KEY,
  model TEXT NOT NULL,
  text_hash TEXT NOT NULL,
  dim INTEGER NOT NULL,
  vec BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE (model, text_hash)
);
CREATE TABLE file_vectors (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  file_id INTEGER NOT NULL UNIQUE REFERENCES files(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  text_hash TEXT NOT NULL,
  src_indexed_at INTEGER NOT NULL
);
CREATE INDEX idx_file_vectors_model ON file_vectors(model, id);
CREATE INDEX idx_file_vectors_hash ON file_vectors(model, text_hash);
"#,
];

pub struct Store {
    conn: Connection,
    /// On-disk location (`None` for in-memory stores).
    path: Option<PathBuf>,
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
        let disk_io = |e: rusqlite::Error| {
            if is_disk_io(&e) {
                StoreError::DiskIo {
                    path: path.to_path_buf(),
                    source: e,
                }
            } else {
                StoreError::Sqlite { source: e }
            }
        };
        let conn = Connection::open(path).map_err(disk_io)?;
        // WAL is the first thing that fails on a network share.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(disk_io)?;
        Self::init(conn, Some(path.to_path_buf())).map_err(|e| match e {
            StoreError::Sqlite { source: e } => disk_io(e),
            other => other,
        })
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, path: Option<PathBuf>) -> Result<Self, StoreError> {
        // `watch` and `undo` may hold the DB from different processes.
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { conn, path };
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

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Database file (so the indexer never indexes its own DB/WAL files).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// True for the database file itself and its `-wal` / `-shm` / `-journal`
    /// side files.
    pub fn is_own_file(&self, p: &Path) -> bool {
        let Some(db) = &self.path else {
            return false;
        };
        let (Some(dir), Some(name)) = (db.parent(), db.file_name()) else {
            return false;
        };
        p.parent() == Some(dir)
            && p.file_name().is_some_and(|n| {
                let n = n.to_string_lossy();
                let name = name.to_string_lossy();
                n == name
                    || n.strip_prefix(&*name)
                        .is_some_and(|rest| rest.starts_with('-'))
            })
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?)
    }

    // ── move_log ────────────────────────────────────────────────────────

    /// Record a finished move.
    pub fn insert_move(&self, m: &NewMove) -> Result<i64, StoreError> {
        self.insert_move_state(m, "done")
    }

    /// Record a move that is about to happen (the mover writes the row
    /// first, then moves, then [`Store::complete_move`]s it).
    pub(crate) fn begin_move(&self, m: &NewMove) -> Result<i64, StoreError> {
        self.insert_move_state(m, "moving")
    }

    fn insert_move_state(&self, m: &NewMove, state: &str) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO move_log (batch_id, src_path, dst_path, hash, size, decided_by,
                                   rule_name, ai_confidence, moved_at, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                state,
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

    /// The file of row `id` is in place: the move is now in effect.
    pub(crate) fn complete_move(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("UPDATE move_log SET state = 'done' WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Rows still marked in progress that were written before `before`
    /// (unix seconds): moves interrupted by a crash or a lost connection.
    pub fn interrupted_moves(&self, before: i64) -> Result<Vec<MoveLogEntry>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_MOVE} WHERE state = 'moving' AND undone_at IS NULL AND moved_at <= ?1
             ORDER BY id"
        ))?;
        let rows = stmt.query_map([before], move_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Hash of the file Tidy last put at `dst` (active, finished move).
    /// The engine leaves such files alone, so a polled NAS folder does not
    /// re-classify what Tidy itself moved into it — across processes too.
    pub fn placed_hash(&self, dst: &Path) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT hash FROM move_log
                 WHERE dst_path = ?1 AND undone_at IS NULL AND state = 'done'
                 ORDER BY id DESC LIMIT 1",
                [path_str(dst)?],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
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

    /// Queue a file for confirmation by a rule with `confirm = true`.
    pub fn upsert_pending(
        &self,
        path: &Path,
        suggested_dest: Option<&Path>,
        reason: Option<&str>,
    ) -> Result<(), StoreError> {
        self.upsert_pending_entry(&NewPending {
            path,
            suggested_dest,
            reason,
            source: PendingSource::Rule,
            confidence: None,
            suggested_name: None,
        })
    }

    pub fn upsert_pending_entry(&self, p: &NewPending<'_>) -> Result<(), StoreError> {
        let dest = p.suggested_dest.map(path_str).transpose()?;
        self.conn.execute(
            "INSERT INTO pending (path, suggested_dest, reason, source, confidence,
                                  suggested_name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET suggested_dest = excluded.suggested_dest,
                                             reason = excluded.reason,
                                             source = excluded.source,
                                             confidence = excluded.confidence,
                                             suggested_name = excluded.suggested_name",
            params![
                path_str(p.path)?,
                dest,
                p.reason,
                p.source.as_str(),
                p.confidence,
                p.suggested_name,
                now()
            ],
        )?;
        Ok(())
    }

    pub fn get_pending_by_path(&self, path: &Path) -> Result<Option<PendingEntry>, StoreError> {
        Ok(self
            .conn
            .query_row(
                &format!("{SELECT_PENDING} WHERE path = ?1"),
                [path_str(path)?],
                pending_from_row,
            )
            .optional()?)
    }

    pub fn list_pending(&self) -> Result<Vec<PendingEntry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_PENDING} ORDER BY id"))?;
        let rows = stmt.query_map([], pending_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_pending(&self, id: i64) -> Result<Option<PendingEntry>, StoreError> {
        Ok(self
            .conn
            .query_row(
                &format!("{SELECT_PENDING} WHERE id = ?1"),
                [id],
                pending_from_row,
            )
            .optional()?)
    }

    pub fn pending_count(&self) -> Result<u64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM pending", [], |r| r.get::<_, i64>(0))?
            as u64)
    }

    pub fn remove_pending_id(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM pending WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn remove_pending(&self, path: &Path) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM pending WHERE path = ?1", [path_str(path)?])?;
        Ok(())
    }

    // ── ignored ─────────────────────────────────────────────────────────

    /// Never organise this path again (confirm popup "무시").
    pub fn add_ignored(&self, path: &Path) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO ignored (path, added_at) VALUES (?1, ?2)
             ON CONFLICT(path) DO NOTHING",
            params![path_str(path)?, now()],
        )?;
        Ok(())
    }

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

const SELECT_PENDING: &str = "SELECT id, path, suggested_dest, reason, created_at, source, \
                              confidence, suggested_name FROM pending";

fn pending_from_row(r: &Row<'_>) -> rusqlite::Result<PendingEntry> {
    Ok(PendingEntry {
        id: r.get(0)?,
        path: PathBuf::from(r.get::<_, String>(1)?),
        suggested_dest: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
        reason: r.get(3)?,
        created_at: r.get(4)?,
        source: PendingSource::parse(&r.get::<_, String>(5)?),
        confidence: r.get(6)?,
        suggested_name: r.get(7)?,
    })
}

const SELECT_MOVE: &str = "SELECT id, batch_id, src_path, dst_path, hash, size, decided_by, \
                           rule_name, ai_confidence, moved_at, undone_at, state FROM move_log";

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
        in_progress: r.get::<_, String>(11)? == "moving",
    })
}

/// Paths are stored as UTF-8 text. A non-UTF-8 path cannot be logged
/// losslessly, so it cannot be moved either (no move without undo).
pub(crate) fn path_str(p: &Path) -> Result<&str, StoreError> {
    p.to_str()
        .ok_or_else(|| StoreError::NonUtf8Path(p.to_path_buf()))
}

pub(crate) fn now() -> i64 {
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
    fn disk_io_errors_carry_network_hint() {
        let e = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::SystemIoFailure,
                extended_code: 2570,
            },
            Some("disk I/O error".into()),
        );
        assert!(StoreError::Sqlite { source: e }
            .to_string()
            .contains("네트워크 경로"));
        let other = rusqlite::Error::QueryReturnedNoRows;
        assert!(!StoreError::Sqlite { source: other }
            .to_string()
            .contains("네트워크"));
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
