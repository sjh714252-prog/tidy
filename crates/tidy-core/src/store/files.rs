//! `roots` / `files` rows (SPEC §6). The FTS tables are kept in sync by
//! triggers, so nothing here touches them directly.

use std::path::Path;

use rusqlite::{params, OptionalExtension, Row};

use super::db::{path_str, Store, StoreError};
use super::models::{FileRecord, FileStamp, IndexStats, NewFile, OcrState, TextStatus};
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
                "SELECT id, size, mtime, text_status, vault, pii_flags IS NULL
                 FROM files WHERE path = ?1",
                [path_str(path)?],
                |r| {
                    Ok(FileStamp {
                        id: r.get(0)?,
                        size: r.get::<_, i64>(1)? as u64,
                        mtime: r.get(2)?,
                        text_none: r.get::<_, String>(3)? == "none",
                        vault: r.get::<_, i64>(4)? != 0,
                        pii_unjudged: r.get::<_, String>(3)? == "ok" && r.get::<_, bool>(5)?,
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
                                text_status, excerpt, content, indexed_at, last_seen_at,
                                pii_flags, pii_counts, vault, ocr)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14, ?15, 0, ?16)
             ON CONFLICT(path) DO UPDATE SET
                root_id = excluded.root_id, name = excluded.name, ext = excluded.ext,
                mime = excluded.mime, size = excluded.size, mtime = excluded.mtime,
                ctime = excluded.ctime, hash = excluded.hash,
                text_status = excluded.text_status, excerpt = excluded.excerpt,
                content = excluded.content, indexed_at = excluded.indexed_at,
                last_seen_at = excluded.last_seen_at, pii_flags = excluded.pii_flags,
                pii_counts = excluded.pii_counts, vault = 0, ocr = excluded.ocr",
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
                f.pii.and_then(|p| p.stored_flags()),
                f.pii
                    .filter(|p| !p.counts.is_empty())
                    .map(|p| serde_json::to_string(&p.counts).unwrap_or_default()),
                f.ocr.map(OcrState::as_str),
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

    /// Rows for `ids` (missing ids are skipped), in no particular order.
    pub fn get_files_by_ids(&self, ids: &[i64]) -> Result<Vec<FileRecord>, StoreError> {
        let mut out = Vec::with_capacity(ids.len());
        // Stay well under SQLite's bound-parameter limit.
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(", ");
            let mut stmt = self
                .conn()
                .prepare(&format!("{SELECT_FILE} WHERE id IN ({marks})"))?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), file_from_row)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
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

    /// (M9) Store a verdict for an existing row.
    pub fn set_file_pii(&self, id: i64, f: &crate::pii::PiiFinding) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE files SET pii_flags = ?1, pii_counts = ?2 WHERE id = ?3",
            params![
                f.stored_flags(),
                (!f.counts.is_empty())
                    .then(|| serde_json::to_string(&f.counts).unwrap_or_default()),
                id
            ],
        )?;
        Ok(())
    }

    /// (M9) A file was locked into `zip`: keep its row (text, FTS, vector)
    /// and point it at the zip. `root_id` is the zip's root.
    pub fn mark_vaulted(
        &self,
        from: &Path,
        zip: &Path,
        root_id: i64,
        size: u64,
        mtime: i64,
        hash: Option<&str>,
    ) -> Result<bool, StoreError> {
        // A stale row at the zip's path (an earlier zip there) gives way.
        self.conn()
            .execute("DELETE FROM files WHERE path = ?1", [path_str(zip)?])?;
        let name = zip
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(self.conn().execute(
            "UPDATE files SET path = ?1, name = ?2, ext = 'zip', mime = 'application/zip',
                              size = ?3, mtime = ?4, hash = ?5, root_id = ?6, vault = 1
             WHERE path = ?7",
            params![
                path_str(zip)?,
                name,
                size as i64,
                mtime,
                hash,
                root_id,
                path_str(from)?
            ],
        )? > 0)
    }

    pub fn set_vault_flag(&self, path: &Path) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE files SET vault = 1 WHERE path = ?1",
            [path_str(path)?],
        )?;
        Ok(())
    }

    /// (M9) Personal-information counts over the index: (suspected files,
    /// of which not encrypted). With `under`, only files below that folder.
    pub fn pii_totals(&self, under: Option<&Path>) -> Result<(u64, u64), StoreError> {
        let mut stmt = self
            .conn()
            .prepare("SELECT path, vault FROM files WHERE pii_flags > 0")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let (mut n, mut open) = (0, 0);
        for row in rows {
            let (p, vault) = row?;
            if under.is_some_and(|u| !Path::new(&p).starts_with(u)) {
                continue;
            }
            n += 1;
            if vault == 0 {
                open += 1;
            }
        }
        Ok((n, open))
    }

    /// (M9) Stored verdict for an indexed file whose size and mtime still
    /// match (so it can be reused instead of extracting the text again).
    pub fn fresh_pii(
        &self,
        path: &Path,
        size: u64,
        mtime: i64,
    ) -> Result<Option<crate::pii::PiiFinding>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT pii_flags, pii_counts, text_status FROM files
                 WHERE path = ?1 AND size = ?2 AND mtime = ?3 AND vault = 0",
                params![path_str(path)?, size as i64, mtime],
                |r| {
                    Ok((
                        r.get::<_, Option<u32>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .and_then(|(flags, counts, status)| {
                // Metadata-only rows were never read: no verdict to reuse.
                (status != "none").then(|| {
                    crate::pii::PiiFinding::from_stored(
                        flags,
                        counts.and_then(|c| serde_json::from_str(&c).ok()),
                    )
                })
            }))
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
     indexed_at, pii_flags, pii_counts, vault, ocr FROM files";

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
        pii_flags: r.get(12)?,
        pii_counts: r
            .get::<_, Option<String>>(13)?
            .and_then(|c| serde_json::from_str(&c).ok()),
        vault: r.get::<_, i64>(14)? != 0,
        ocr: r
            .get::<_, Option<String>>(15)?
            .as_deref()
            .and_then(OcrState::parse),
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
            pii: None,
            ocr: None,
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
                vault: false,
                pii_unjudged: true,
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
