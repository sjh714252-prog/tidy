//! (M10) `curation` rows: the cached work-context analysis per file content.

use rusqlite::{params, OptionalExtension};

use super::db::{now, path_str, Store, StoreError};
use crate::curate::{Curation, CurationSource};

impl Store {
    pub fn get_curation(&self, key: &str) -> Result<Option<Curation>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT entity, issue, issue_date, doc_kind, topic, confidence, reason, source
                 FROM curation WHERE key = ?1",
                [key],
                |r| {
                    Ok(Curation {
                        entity: r.get(0)?,
                        issue: r.get(1)?,
                        date: r
                            .get::<_, Option<String>>(2)?
                            .and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()),
                        doc_kind: r.get(3)?,
                        topic: r.get(4)?,
                        confidence: r.get(5)?,
                        reason: r.get(6)?,
                        source: if r.get::<_, String>(7)? == "user" {
                            CurationSource::User
                        } else {
                            CurationSource::Ai
                        },
                    })
                },
            )
            .optional()?)
    }

    pub fn put_curation(
        &self,
        key: &str,
        path: &std::path::Path,
        c: &Curation,
        model: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO curation (key, path, entity, issue, issue_date, doc_kind, topic,
                                   confidence, reason, source, model, analyzed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(key) DO UPDATE SET
               path = excluded.path, entity = excluded.entity, issue = excluded.issue,
               issue_date = excluded.issue_date, doc_kind = excluded.doc_kind,
               topic = excluded.topic, confidence = excluded.confidence,
               reason = excluded.reason, source = excluded.source, model = excluded.model,
               analyzed_at = excluded.analyzed_at",
            params![
                key,
                path_str(path)?,
                c.entity,
                c.issue,
                c.date.map(|d| d.format("%Y-%m-%d").to_string()),
                c.doc_kind,
                c.topic,
                c.confidence,
                c.reason,
                c.source.as_str(),
                model,
                now()
            ],
        )?;
        Ok(())
    }

    /// Dates already used for this customer + issue (other files analysed
    /// before), so new files join the same dated folder.
    pub fn curation_dates(
        &self,
        entity: &str,
        issue: &str,
    ) -> Result<Vec<chrono::NaiveDate>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT issue_date FROM curation
             WHERE entity = ?1 AND issue = ?2 AND issue_date IS NOT NULL",
        )?;
        let rows = stmt.query_map([entity, issue], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(&r?, "%Y-%m-%d") {
                out.push(d);
            }
        }
        out.sort();
        Ok(out)
    }

    /// (entity, issue) pairs seen so far (drop-down choices in the preview).
    pub fn curation_groups(&self) -> Result<Vec<(String, String)>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT entity, issue FROM curation WHERE entity IS NOT NULL ORDER BY entity, issue",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn curation_count(&self) -> Result<u64, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT COUNT(*) FROM curation", [], |r| r.get::<_, i64>(0))?
            as u64)
    }
}
