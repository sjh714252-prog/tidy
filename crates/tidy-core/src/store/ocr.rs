//! (M15) `files.ocr` and `ocr_cache` (SPEC §14.11): which images and
//! scanned PDFs still wait for text recognition, and the results.

use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use super::db::{Store, StoreError};
use super::models::OcrState;

/// A file waiting for recognition.
#[derive(Debug, Clone)]
pub struct OcrCandidate {
    pub id: i64,
    pub path: PathBuf,
    pub name: String,
    pub ext: String,
    pub hash: Option<String>,
    /// Under a network root (reads take a NAS IO slot).
    pub network: bool,
}

/// Counts for `tidy ocr --status` and the settings card.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct OcrCounts {
    pub pending: u64,
    pub ok: u64,
    pub empty: u64,
    pub failed: u64,
}

impl Store {
    /// Up to `limit` pending files, oldest rows first (`pdf = false`:
    /// images only, for an engine that cannot read PDFs).
    pub fn ocr_candidates(&self, limit: usize, pdf: bool) -> Result<Vec<OcrCandidate>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT f.id, f.path, f.name, COALESCE(f.ext, ''), f.hash, r.kind = 'network'
             FROM files f JOIN roots r ON r.id = f.root_id
             WHERE f.ocr = 'pending' AND f.vault = 0 AND (?2 OR COALESCE(f.ext, '') != 'pdf')
             ORDER BY f.id LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64, pdf], |r| {
            Ok(OcrCandidate {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                name: r.get(2)?,
                ext: r.get(3)?,
                hash: r.get(4)?,
                network: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn ocr_counts(&self) -> Result<OcrCounts, StoreError> {
        let mut c = OcrCounts::default();
        let mut stmt = self
            .conn()
            .prepare("SELECT ocr, COUNT(*) FROM files WHERE ocr IS NOT NULL GROUP BY ocr")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (state, n) = row?;
            let n = n as u64;
            match OcrState::parse(&state) {
                Some(OcrState::Pending) => c.pending = n,
                Some(OcrState::Ok) => c.ok = n,
                Some(OcrState::Empty) => c.empty = n,
                Some(OcrState::Failed) => c.failed = n,
                None => {}
            }
        }
        Ok(c)
    }

    /// Store the recognized text of row `id` as its body (`text` empty =
    /// nothing found). `indexed_at` moves forward so the semantic-search
    /// vector is computed again from the new text.
    pub fn set_ocr_text(
        &self,
        id: i64,
        text: &str,
        pii: Option<&crate::pii::PiiFinding>,
    ) -> Result<(), StoreError> {
        let now = super::db::now();
        if text.trim().is_empty() {
            self.conn()
                .execute("UPDATE files SET ocr = 'empty' WHERE id = ?1", [id])?;
            return Ok(());
        }
        let excerpt: String = text.chars().take(500).collect();
        self.conn().execute(
            "UPDATE files SET ocr = 'ok', text_status = 'ok', content = ?1, excerpt = ?2,
                              indexed_at = MAX(indexed_at + 1, ?3)
             WHERE id = ?4",
            params![text, excerpt, now, id],
        )?;
        if let Some(p) = pii {
            self.set_file_pii(id, p)?;
        }
        Ok(())
    }

    pub fn set_ocr_failed(&self, id: i64) -> Result<(), StoreError> {
        self.conn()
            .execute("UPDATE files SET ocr = 'failed' WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Failed files wait again (`tidy ocr --retry`). Returns how many.
    pub fn ocr_retry_failed(&self) -> Result<usize, StoreError> {
        Ok(self
            .conn()
            .execute("UPDATE files SET ocr = 'pending' WHERE ocr = 'failed'", [])?)
    }

    pub fn ocr_cache_get(&self, hash: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT text FROM ocr_cache WHERE hash = ?1", [hash], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn ocr_cache_put(&self, hash: &str, text: &str, engine: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO ocr_cache (hash, text, engine, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(hash) DO UPDATE SET text = excluded.text, engine = excluded.engine,
                                             created_at = excluded.created_at",
            params![hash, text, engine, super::db::now()],
        )?;
        Ok(())
    }
}
