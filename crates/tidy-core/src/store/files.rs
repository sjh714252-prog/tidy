//! `roots` / `files` rows (SPEC §6). The FTS tables are kept in sync by
//! triggers, so nothing here touches them directly.

use std::path::Path;

use rusqlite::{params, OptionalExtension, Row};

use super::db::{path_str, Store, StoreError};
use super::models::{FileRecord, FileStamp, IndexStats, NewFile, TextStatus};
use crate::config::RootKind;

impl Store {
    // ── roots ───────────────────────────────────────────────────────────

    /// Register (or update) an index root and return its id.
    pub fn upsert_root(
        &self,
        path: &Path,
        kind: RootKind,
        watch: bool,
        index_content: bool,
    ) -> Result<i64, StoreError> {
        let kind = match kind {
            RootKind::Local => "local",
            RootKind::Network => "network",
        };
        self.conn().execute(
            "INSERT INTO roots (path, kind, watch, index_content)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET kind = excluded.kind,
                                             watch = excluded.watch,
                                             index_content = excluded.index_content",
            params![path_str(path)?, kind, watch, index_content],
        )?;
        Ok(self.conn().query_row(
            "SELECT id FROM roots WHERE path = ?1",
            [path_str(path)?],
            |r| r.get(0),
        )?)
    }

    pub fn root_id(&self, path: &Path) -> Result<Option<i64>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM roots WHERE path = ?1",
                [path_str(path)?],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The deepest registered root containing `path` (roots may nest).
    pub fn root_for_path(&self, path: &Path) -> Result<Option<i64>, StoreError> {
        let mut stmt = self
            .conn()
            .prepare("SELECT id, path FROM roots ORDER BY length(path) DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, root) = row?;
            if path.starts_with(Path::new(&root)) {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Largest `last_seen_at` under a root (0 if none).
    pub fn max_last_seen(&self, root_id: i64) -> Result<i64, StoreError> {
        Ok(self.conn().query_row(
            "SELECT COALESCE(MAX(last_seen_at), 0) FROM files WHERE root_id = ?1",
            [root_id],
            |r| r.get(0),
        )?)
    }

    /// Forget a root and (via `ON DELETE CASCADE`) every file under it.
    pub fn remove_root(&self, path: &Path) -> Result<(), StoreError> {
        let n = self
            .conn()
            .execute("DELETE FROM roots WHERE path = ?1", [path_str(path)?])?;
        if n == 0 {
            return Err(StoreError::UnknownRoot(path.to_path_buf()));
        }
        Ok(())
    }

    // ── files ───────────────────────────────────────────────────────────

    /// Size/mtime of an already indexed file (the incremental check).
    pub fn file_stamp(&self, path: &Path) -> Result<Option<FileStamp>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, size, mtime, text_status FROM files WHERE path = ?1",
                [path_str(path)?],
                |r| {
                    Ok(FileStamp {
                        id: r.get(0)?,
                        size: r.get::<_, i64>(1)? as u64,
                        mtime: r.get(2)?,
                        text_none: r.get::<_, String>(3)? == "none",
                    })
                },
            )
            .optional()?)
    }

    /// Mark an unchanged file as seen in the current run.
    pub fn touch_file(&self, id: i64, seen_at: i64) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE files SET last_seen_at = ?1 WHERE id = ?2",
            params![seen_at, id],
        )?;
        Ok(())
    }

    /// Insert or replace (by path) one file row. Returns the row id.
    pub fn upsert_file(&self, f: &NewFile, seen_at: i64) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO files (path, root_id, name, ext, mime, size, mtime, ctime, hash,
                                text_status, excerpt, content, indexed_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
             ON CONFLICT(path) DO UPDATE SET
                root_id = excluded.root_id, name = excluded.name, ext = excluded.ext,
                mime = excluded.mime, size = excluded.size, mtime = excluded.mtime,
                ctime = excluded.ctime, hash = excluded.hash,
                text_status = excluded.text_status, excerpt = excluded.excerpt,
                content = excluded.content, indexed_at = excluded.indexed_at,
                last_seen_at = excluded.last_seen_at",
            params![
                path_str(&f.path)?,
                f.root_id,
                f.name,
                f.ext,
                f.mime,
                f.size as i64,
                f.mtime,
                f.ctime,
                f.hash,
                f.text_status.as_str(),
                f.excerpt,
                f.content,
                seen_at,
            ],
        )?;
        Ok(self.conn().query_row(
            "SELECT id FROM files WHERE path = ?1",
            [path_str(&f.path)?],
            |r| r.get(0),
        )?)
    }

    /// Drop rows under `root_id` that the last walk did not see (files that
    /// were moved away or deleted). Only index rows go; files are untouched.
    pub fn remove_unseen(&self, root_id: i64, seen_at: i64) -> Result<usize, StoreError> {
        Ok(self.conn().execute(
            "DELETE FROM files WHERE root_id = ?1 AND last_seen_at < ?2",
            params![root_id, seen_at],
        )?)
    }

    pub fn remove_file(&self, path: &Path) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .execute("DELETE FROM files WHERE path = ?1", [path_str(path)?])?
            > 0)
    }

    pub fn get_file(&self, path: &Path) -> Result<Option<FileRecord>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                &format!("{SELECT_FILE} WHERE path = ?1"),
                [path_str(path)?],
                file_from_row,
            )
            .optional()?)
    }

    /// Stored body text of a file (what the FTS index was built from).
    pub fn file_content(&self, id: i64) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT content FROM files WHERE id = ?1", [id], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten())
    }

    pub fn index_stats(&self) -> Result<IndexStats, StoreError> {
        let roots: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0))?;
        let (files, with_text, bytes): (i64, i64, i64) = self.conn().query_row(
            "SELECT COUNT(*), COALESCE(SUM(text_status = 'ok'), 0), COALESCE(SUM(size), 0)
             FROM files",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(IndexStats {
            roots: roots as u64,
            files: files as u64,
            with_text: with_text as u64,
            total_bytes: bytes as u64,
        })
    }

    /// Run `f` inside one transaction (the indexer batches a whole folder).
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&Store) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let tx = self.conn().unchecked_transaction()?;
        let out = f(self)?;
        tx.commit()?;
        Ok(out)
    }

    /// Rebuild both FTS indexes from `files` (after a crash or a schema
    /// change in a tokenizer).
    pub fn rebuild_fts(&self) -> Result<(), StoreError> {
        self.conn().execute_batch(
            "INSERT INTO files_fts(files_fts) VALUES ('rebuild');
             INSERT INTO files_fts_word(files_fts_word) VALUES ('rebuild');",
        )?;
        Ok(())
    }

    pub fn clear_index(&self) -> Result<(), StoreError> {
        self.conn()
            .execute_batch("DELETE FROM files; DELETE FROM roots;")?;
        Ok(())
    }
}

pub(crate) const SELECT_FILE: &str =
    "SELECT id, path, root_id, name, ext, mime, size, mtime, hash, text_status, excerpt, \
     indexed_at FROM files";

pub(crate) fn file_from_row(r: &Row<'_>) -> rusqlite::Result<FileRecord> {
    let status: String = r.get(9)?;
    Ok(FileRecord {
        id: r.get(0)?,
        path: std::path::PathBuf::from(r.get::<_, String>(1)?),
        root_id: r.get(2)?,
        name: r.get(3)?,
        ext: r.get(4)?,
        mime: r.get(5)?,
        size: r.get::<_, i64>(6)? as u64,
        mtime: r.get(7)?,
        hash: r.get(8)?,
        text_status: TextStatus::parse(&status).unwrap_or(TextStatus::None),
        excerpt: r.get(10)?,
        indexed_at: r.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(root: i64, path: &str, content: Option<&str>) -> NewFile {
        let p = PathBuf::from(path);
        NewFile {
            name: p.file_name().unwrap().to_string_lossy().into_owned(),
            ext: p.extension().map(|e| e.to_string_lossy().into_owned()),
            path: p,
            root_id: root,
            mime: None,
            size: 3,
            mtime: 1_700_000_000,
            ctime: None,
            hash: None,
            text_status: if content.is_some() {
                TextStatus::Ok
            } else {
                TextStatus::Unsupported
            },
            excerpt: content.map(|c| c.chars().take(500).collect()),
            content: content.map(str::to_string),
        }
    }

    #[test]
    fn upsert_touch_and_prune() {
        let s = Store::open_in_memory().unwrap();
        let root = s
            .upsert_root(Path::new("/r"), RootKind::Local, false, true)
            .unwrap();
        assert_eq!(s.root_id(Path::new("/r")).unwrap(), Some(root));
        let a = s
            .upsert_file(&file(root, "/r/a.txt", Some("hello")), 10)
            .unwrap();
        let b = s.upsert_file(&file(root, "/r/b.txt", None), 10).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            s.file_stamp(Path::new("/r/a.txt")).unwrap(),
            Some(FileStamp {
                id: a,
                size: 3,
                mtime: 1_700_000_000,
                text_none: false,
            })
        );
        // Next run: a seen, b not → b pruned.
        s.touch_file(a, 20).unwrap();
        assert_eq!(s.remove_unseen(root, 20).unwrap(), 1);
        assert!(s.get_file(Path::new("/r/b.txt")).unwrap().is_none());
        let st = s.index_stats().unwrap();
        assert_eq!((st.roots, st.files, st.with_text), (1, 1, 1));
        // Replacing by path keeps a single row and updates the content.
        let a2 = s
            .upsert_file(&file(root, "/r/a.txt", Some("changed")), 30)
            .unwrap();
        assert_eq!(a2, a);
        assert_eq!(s.file_content(a).unwrap().as_deref(), Some("changed"));
        // Removing the root cascades.
        s.remove_root(Path::new("/r")).unwrap();
        assert_eq!(s.index_stats().unwrap().files, 0);
        assert!(matches!(
            s.remove_root(Path::new("/r")),
            Err(StoreError::UnknownRoot(_))
        ));
    }
}
