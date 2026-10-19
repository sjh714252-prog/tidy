//! `vectors` / `file_vectors` (SPEC §6, M7): embeddings for semantic
//! search. See the v6 migration for the layout.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use super::db::{Store, StoreError};

/// A file whose embedding is missing or out of date for a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedCandidate {
    pub file_id: i64,
    pub name: String,
    /// Start of the body text (`None` when there is none or it is not
    /// wanted).
    pub text: Option<String>,
    pub indexed_at: i64,
}

/// One row for the in-memory search cache.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorRow {
    /// `file_vectors.id` (grows monotonically).
    pub seq: i64,
    pub file_id: i64,
    pub vec: Vec<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct VectorStats {
    /// Files with a current embedding for the model.
    pub embedded: u64,
    /// Indexed files in total.
    pub files: u64,
    /// Distinct stored vectors (all models).
    pub vectors: u64,
}

pub(crate) fn encode(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub(crate) fn decode(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

impl Store {
    /// Files without a current embedding for `model`: never embedded,
    /// embedded with another model, or re-indexed since. Body text is cut
    /// at `text_chars` characters (0 = name only).
    pub fn embed_candidates(
        &self,
        model: &str,
        text_chars: usize,
        limit: usize,
    ) -> Result<Vec<EmbedCandidate>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT f.id, f.name, CASE WHEN ?2 > 0 THEN substr(f.content, 1, ?2) END, f.indexed_at
             FROM files f LEFT JOIN file_vectors fv ON fv.file_id = f.id
             WHERE fv.file_id IS NULL OR fv.model != ?1 OR fv.src_indexed_at != f.indexed_at
             ORDER BY f.id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![model, text_chars as i64, limit as i64], |r| {
            Ok(EmbedCandidate {
                file_id: r.get(0)?,
                name: r.get(1)?,
                text: r
                    .get::<_, Option<String>>(2)?
                    .filter(|t| !t.trim().is_empty()),
                indexed_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Number of files [`Store::embed_candidates`] would return without a limit.
    pub fn embed_backlog(&self, model: &str) -> Result<u64, StoreError> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*) FROM files f LEFT JOIN file_vectors fv ON fv.file_id = f.id
             WHERE fv.file_id IS NULL OR fv.model != ?1 OR fv.src_indexed_at != f.indexed_at",
            [model],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    pub fn has_vector(&self, model: &str, text_hash: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM vectors WHERE model = ?1 AND text_hash = ?2",
                params![model, text_hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Store a vector (no-op if this model already has one for the text).
    pub fn put_vector(&self, model: &str, text_hash: &str, vec: &[f32]) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT OR IGNORE INTO vectors (model, text_hash, dim, vec, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                model,
                text_hash,
                vec.len() as i64,
                encode(vec),
                super::db::now()
            ],
        )?;
        Ok(())
    }

    /// Point a file at a stored vector (replacing its previous mapping).
    pub fn map_file_vector(
        &self,
        file_id: i64,
        model: &str,
        text_hash: &str,
        src_indexed_at: i64,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT OR REPLACE INTO file_vectors (file_id, model, text_hash, src_indexed_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![file_id, model, text_hash, src_indexed_at],
        )?;
        Ok(())
    }

    /// The vector a file currently points at for `model`.
    pub fn file_vector(&self, file_id: i64, model: &str) -> Result<Option<Vec<f32>>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT v.vec FROM file_vectors fv
                 JOIN vectors v ON v.model = fv.model AND v.text_hash = fv.text_hash
                 WHERE fv.file_id = ?1 AND fv.model = ?2",
                params![file_id, model],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|b| decode(&b)))
    }

    /// `(rows, max seq)` of `model`'s file mappings: changes whenever a
    /// mapping is added, replaced or removed.
    pub fn vector_signature(&self, model: &str) -> Result<(i64, i64), StoreError> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM file_vectors WHERE model = ?1",
            [model],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// Mappings of `model` with `seq > after_seq`, with their vectors, in
    /// seq order (`after_seq = 0`: all of them).
    pub fn for_each_vector(
        &self,
        model: &str,
        after_seq: i64,
        mut f: impl FnMut(VectorRow),
    ) -> Result<(), StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT fv.id, fv.file_id, v.vec FROM file_vectors fv
             JOIN vectors v ON v.model = fv.model AND v.text_hash = fv.text_hash
             WHERE fv.model = ?1 AND fv.id > ?2 ORDER BY fv.id",
        )?;
        let mut rows = stmt.query(params![model, after_seq])?;
        while let Some(r) = rows.next()? {
            let blob = r.get_ref(2)?.as_blob().map_err(rusqlite::Error::from)?;
            f(VectorRow {
                seq: r.get(0)?,
                file_id: r.get(1)?,
                vec: decode(blob),
            });
        }
        Ok(())
    }

    pub fn vector_stats(&self, model: &str) -> Result<VectorStats, StoreError> {
        let (embedded, files, vectors): (i64, i64, i64) = self.conn().query_row(
            "SELECT (SELECT COUNT(*) FROM file_vectors fv JOIN files f ON f.id = fv.file_id
                     WHERE fv.model = ?1 AND fv.src_indexed_at = f.indexed_at),
                    (SELECT COUNT(*) FROM files),
                    (SELECT COUNT(*) FROM vectors)",
            [model],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(VectorStats {
            embedded: embedded as u64,
            files: files as u64,
            vectors: vectors as u64,
        })
    }

    /// Drop vectors no file points at any more, once older than
    /// `older_than` (unix seconds) — kept for a while so a file that is
    /// moved (removed and re-added in the index) gets its vector back.
    pub fn prune_vectors(&self, older_than: i64) -> Result<usize, StoreError> {
        Ok(self.conn().execute(
            "DELETE FROM vectors WHERE created_at < ?1 AND NOT EXISTS (
               SELECT 1 FROM file_vectors fv
               WHERE fv.model = vectors.model AND fv.text_hash = vectors.text_hash)",
            [older_than],
        )?)
    }

    /// Forget every embedding (`tidy embed --rebuild`).
    pub fn clear_vectors(&self) -> Result<(), StoreError> {
        self.conn()
            .execute_batch("DELETE FROM file_vectors; DELETE FROM vectors;")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RootKind;
    use crate::store::{NewFile, TextStatus};
    use std::path::{Path, PathBuf};

    fn add(s: &Store, root: i64, name: &str, body: Option<&str>, seen: i64) -> i64 {
        s.upsert_file(
            &NewFile {
                path: PathBuf::from("/r").join(name),
                root_id: root,
                name: name.into(),
                ext: None,
                mime: None,
                size: 1,
                mtime: 1,
                ctime: None,
                hash: None,
                text_status: TextStatus::Ok,
                excerpt: None,
                content: body.map(str::to_string),
                pii: None,
            },
            seen,
        )
        .unwrap()
    }

    #[test]
    fn candidates_mapping_reuse_and_cascade() {
        let s = Store::open_in_memory().unwrap();
        let root = s
            .upsert_root(Path::new("/r"), RootKind::Local, false, true)
            .unwrap();
        let a = add(&s, root, "a.txt", Some("가나다라마바사"), 10);
        let b = add(&s, root, "b.png", None, 10);
        let c = s.embed_candidates("m", 3, 10).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].text.as_deref(), Some("가나다"));
        assert_eq!(c[1].text, None);
        assert_eq!(s.embed_candidates("m", 0, 10).unwrap()[0].text, None);

        s.put_vector("m", "h1", &[0.6, 0.8]).unwrap();
        s.put_vector("m", "h1", &[9.0, 9.0]).unwrap(); // ignored
        s.map_file_vector(a, "m", "h1", 10).unwrap();
        s.map_file_vector(b, "m", "h1", 10).unwrap();
        assert_eq!(s.file_vector(a, "m").unwrap(), Some(vec![0.6, 0.8]));
        assert!(s.embed_candidates("m", 3, 10).unwrap().is_empty());
        // Another model sees both as missing.
        assert_eq!(s.embed_backlog("other").unwrap(), 2);
        let sig = s.vector_signature("m").unwrap();
        assert_eq!(sig.0, 2);

        // Re-indexed file → stale again.
        add(&s, root, "a.txt", Some("changed"), 20);
        assert_eq!(s.embed_backlog("m").unwrap(), 1);
        assert_eq!(s.vector_stats("m").unwrap().embedded, 1);
        s.map_file_vector(a, "m", "h1", 20).unwrap();
        let sig2 = s.vector_signature("m").unwrap();
        assert_eq!(sig2.0, 2);
        assert!(sig2.1 > sig.1, "replaced mapping gets a new seq");
        let mut seen = Vec::new();
        s.for_each_vector("m", sig.1, |r| seen.push(r.file_id))
            .unwrap();
        assert_eq!(seen, vec![a]);

        // Removing the file drops the mapping, not the vector (reuse).
        s.remove_file(&Path::new("/r").join("a.txt")).unwrap();
        s.remove_file(&Path::new("/r").join("b.png")).unwrap();
        assert_eq!(s.vector_signature("m").unwrap().0, 0);
        assert!(s.has_vector("m", "h1").unwrap());
        assert_eq!(s.prune_vectors(0).unwrap(), 0, "too young to prune");
        assert_eq!(s.prune_vectors(i64::MAX).unwrap(), 1);
        assert!(!s.has_vector("m", "h1").unwrap());
    }

    #[test]
    fn blob_round_trip() {
        let v = vec![0.0f32, -1.5, 3.25e-8, f32::MAX];
        assert_eq!(decode(&encode(&v)), v);
    }
}
