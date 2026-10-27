//! `restructure_plans` and the move-history view of `move_log` (M5).

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use super::db::{now, path_str, Store, StoreError};

/// A stored restructure plan (the plan itself is opaque JSON here; the
/// report module owns its shape).
#[derive(Debug, Clone, Serialize)]
pub struct StoredPlan {
    pub id: i64,
    pub root: PathBuf,
    pub source: String,
    pub created_at: i64,
    pub plan_json: String,
    pub applied_batch: Option<String>,
    pub applied_at: Option<i64>,
}

/// One batch of moves, for the history timeline (SPEC §9.6).
#[derive(Debug, Clone, Serialize)]
pub struct BatchSummary {
    pub batch_id: String,
    pub first_at: i64,
    pub last_at: i64,
    pub moves: u64,
    /// Rows already undone.
    pub undone: u64,
    /// rule | ai | user | restructure (most frequent in the batch).
    pub decided_by: String,
    /// A rule / plan label from the batch, if any.
    pub label: Option<String>,
    /// Example source path.
    pub example: PathBuf,
    /// (M14) Rows that sent a duplicate to the trash (`op = trash`).
    pub trashed: u64,
}

impl Store {
    pub fn save_plan(&self, root: &Path, source: &str, plan_json: &str) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO restructure_plans (root, source, created_at, plan_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![path_str(root)?, source, now(), plan_json],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    pub fn update_plan_json(&self, id: i64, plan_json: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE restructure_plans SET plan_json = ?1 WHERE id = ?2",
            params![plan_json, id],
        )?;
        Ok(())
    }

    pub fn get_plan(&self, id: i64) -> Result<Option<StoredPlan>, StoreError> {
        Ok(self
            .conn()
            .query_row(&format!("{SELECT_PLAN} WHERE id = ?1"), [id], plan_from_row)
            .optional()?)
    }

    pub fn recent_plans(&self, limit: usize) -> Result<Vec<StoredPlan>, StoreError> {
        let mut stmt = self
            .conn()
            .prepare(&format!("{SELECT_PLAN} ORDER BY id DESC LIMIT ?1"))?;
        let rows = stmt.query_map([limit as i64], plan_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn mark_plan_applied(&self, id: i64, batch_id: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE restructure_plans SET applied_batch = ?1, applied_at = ?2 WHERE id = ?3",
            params![batch_id, now(), id],
        )?;
        Ok(())
    }

    /// Newest batches first.
    pub fn batches(&self, limit: usize) -> Result<Vec<BatchSummary>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT batch_id, MIN(moved_at), MAX(moved_at), COUNT(*),
                    SUM(undone_at IS NOT NULL),
                    (SELECT decided_by FROM move_log m2 WHERE m2.batch_id = m.batch_id
                       GROUP BY decided_by ORDER BY COUNT(*) DESC LIMIT 1),
                    MAX(rule_name), MIN(src_path), SUM(op = 'trash')
             FROM move_log m GROUP BY batch_id ORDER BY MAX(id) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(BatchSummary {
                batch_id: r.get(0)?,
                first_at: r.get(1)?,
                last_at: r.get(2)?,
                moves: r.get::<_, i64>(3)? as u64,
                undone: r.get::<_, i64>(4)? as u64,
                decided_by: r.get(5)?,
                label: r.get(6)?,
                example: PathBuf::from(r.get::<_, String>(7)?),
                trashed: r.get::<_, i64>(8)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

const SELECT_PLAN: &str = "SELECT id, root, source, created_at, plan_json, applied_batch, \
                           applied_at FROM restructure_plans";

fn plan_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredPlan> {
    Ok(StoredPlan {
        id: r.get(0)?,
        root: PathBuf::from(r.get::<_, String>(1)?),
        source: r.get(2)?,
        created_at: r.get(3)?,
        plan_json: r.get(4)?,
        applied_batch: r.get(5)?,
        applied_at: r.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DecidedBy, NewMove};

    #[test]
    fn plans_and_batches() {
        let s = Store::open_in_memory().unwrap();
        let id = s.save_plan(Path::new("/r/바탕 화면"), "ai", "{}").unwrap();
        assert!(s.get_plan(id).unwrap().unwrap().applied_batch.is_none());
        s.mark_plan_applied(id, "B1").unwrap();
        assert_eq!(
            s.get_plan(id).unwrap().unwrap().applied_batch.as_deref(),
            Some("B1")
        );
        assert_eq!(s.recent_plans(5).unwrap().len(), 1);

        for (b, src) in [("B1", "/a/1"), ("B1", "/a/2"), ("B2", "/a/3")] {
            s.insert_move(&NewMove {
                batch_id: b.into(),
                src_path: PathBuf::from(src),
                dst_path: PathBuf::from("/d").join(src.rsplit('/').next().unwrap()),
                hash: None,
                size: None,
                decided_by: DecidedBy::Restructure,
                rule_name: Some("재구성 #1".into()),
                ai_confidence: None,
                op: crate::store::MoveOp::Move,
            })
            .unwrap();
        }
        let b = s.batches(10).unwrap();
        assert_eq!(b[0].batch_id, "B2");
        assert_eq!(b[1].moves, 2);
        assert_eq!(b[1].decided_by, "restructure");
        assert_eq!(b[1].label.as_deref(), Some("재구성 #1"));
    }
}
