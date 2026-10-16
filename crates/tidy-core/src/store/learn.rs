//! `decisions`, `ai_log`, `learn_dismissed` (SPEC §5.2 learner, §5.8).

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension, Row};

use super::db::{now, path_str, Store, StoreError};
use super::models::{AiLogEntry, Decision, DecisionAction, PendingSource};

/// Input for [`Store::insert_decision`].
#[derive(Debug, Clone)]
pub struct NewDecision<'a> {
    pub file_name: &'a str,
    pub ext: Option<&'a str>,
    pub src_dir: &'a Path,
    pub suggested_dest: Option<&'a Path>,
    pub chosen_dest: Option<&'a Path>,
    pub action: DecisionAction,
    pub source: PendingSource,
}

const SELECT_DECISION: &str = "SELECT id, file_name, ext, src_dir, suggested_dest, chosen_dest, \
                               action, source, decided_at FROM decisions";

fn decision_from_row(r: &Row<'_>) -> rusqlite::Result<Decision> {
    Ok(Decision {
        id: r.get(0)?,
        file_name: r.get(1)?,
        ext: r.get(2)?,
        src_dir: PathBuf::from(r.get::<_, String>(3)?),
        suggested_dest: r.get::<_, Option<String>>(4)?.map(PathBuf::from),
        chosen_dest: r.get::<_, Option<String>>(5)?.map(PathBuf::from),
        action: DecisionAction::parse(&r.get::<_, String>(6)?),
        source: PendingSource::parse(&r.get::<_, String>(7)?),
        decided_at: r.get(8)?,
    })
}

impl Store {
    // ── decisions ───────────────────────────────────────────────────────

    pub fn insert_decision(&self, d: &NewDecision<'_>) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO decisions (file_name, ext, src_dir, suggested_dest, chosen_dest,
                                    action, source, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                d.file_name,
                d.ext,
                path_str(d.src_dir)?,
                d.suggested_dest.map(path_str).transpose()?,
                d.chosen_dest.map(path_str).transpose()?,
                d.action.as_str(),
                d.source.as_str(),
                now(),
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    /// Every "move" decision (with a destination), oldest first — the
    /// learner's input.
    pub fn move_decisions(&self) -> Result<Vec<Decision>, StoreError> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT_DECISION} WHERE action IN ('move', 'rename_move')
               AND chosen_dest IS NOT NULL ORDER BY id"
        ))?;
        let rows = stmt.query_map([], decision_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Recent decisions most similar to a file (same extension first, then
    /// same source folder, then anything), newest first. Used as examples
    /// in the AI prompt.
    pub fn similar_decisions(
        &self,
        ext: Option<&str>,
        src_dir: &Path,
        limit: usize,
    ) -> Result<Vec<Decision>, StoreError> {
        let mut stmt = self.conn().prepare(&format!(
            "{SELECT_DECISION} WHERE chosen_dest IS NOT NULL
             ORDER BY (ext IS ?1) DESC, (src_dir = ?2) DESC, id DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(
            params![ext, path_str(src_dir)?, limit as i64],
            decision_from_row,
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn recent_decisions(&self, limit: usize) -> Result<Vec<Decision>, StoreError> {
        let mut stmt = self
            .conn()
            .prepare(&format!("{SELECT_DECISION} ORDER BY id DESC LIMIT ?1"))?;
        let rows = stmt.query_map([limit as i64], decision_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Distinct destination folders of recent moves, newest first (the
    /// "recently used folders" part of the AI candidate list).
    pub fn recent_dest_dirs(&self, limit: usize) -> Result<Vec<PathBuf>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT dst_path FROM move_log WHERE undone_at IS NULL ORDER BY id DESC LIMIT 500",
        )?;
        let mut out: Vec<PathBuf> = Vec::new();
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let p = PathBuf::from(row?);
            if let Some(dir) = p.parent().map(Path::to_path_buf) {
                if !out.contains(&dir) {
                    out.push(dir);
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    // ── learner suggestions dismissed by the user ───────────────────────

    pub fn dismiss_learn(&self, key: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO learn_dismissed (key, dismissed_at) VALUES (?1, ?2)
             ON CONFLICT(key) DO NOTHING",
            params![key, now()],
        )?;
        Ok(())
    }

    pub fn is_learn_dismissed(&self, key: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM learn_dismissed WHERE key = ?1",
                [key],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    // ── ai_log ──────────────────────────────────────────────────────────

    pub fn insert_ai_log(&self, e: &AiLogEntry) -> Result<i64, StoreError> {
        self.conn().execute(
            "INSERT INTO ai_log (provider, model, purpose, input_tokens, output_tokens,
                                 cost_usd, latency_ms, ok, error, result, called_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                e.provider,
                e.model,
                e.purpose,
                e.input_tokens.map(|v| v as i64),
                e.output_tokens.map(|v| v as i64),
                e.cost_usd,
                e.latency_ms.map(|v| v as i64),
                e.ok,
                e.error,
                e.result,
                if e.called_at == 0 { now() } else { e.called_at },
            ],
        )?;
        Ok(self.conn().last_insert_rowid())
    }

    pub fn recent_ai_log(&self, limit: usize) -> Result<Vec<AiLogEntry>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, provider, model, purpose, input_tokens, output_tokens, cost_usd,
                    latency_ms, ok, error, result, called_at
             FROM ai_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(AiLogEntry {
                id: r.get(0)?,
                provider: r.get(1)?,
                model: r.get(2)?,
                purpose: r.get(3)?,
                input_tokens: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                output_tokens: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
                cost_usd: r.get(6)?,
                latency_ms: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                ok: r.get(8)?,
                error: r.get(9)?,
                result: r.get(10)?,
                called_at: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// (calls, failures, input tokens, output tokens, cost) over all time.
    pub fn ai_usage(&self) -> Result<(u64, u64, u64, u64, f64), StoreError> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*), COALESCE(SUM(ok = 0), 0), COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cost_usd), 0.0)
             FROM ai_log",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, i64>(3)? as u64,
                    r.get(4)?,
                ))
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_round_trip_and_similarity_order() {
        let s = Store::open_in_memory().unwrap();
        let desk = Path::new("/u/바탕 화면");
        for (name, ext, dest) in [
            ("a.pdf", "pdf", "/d/문서"),
            ("b.xlsx", "xlsx", "/d/표"),
            ("c.pdf", "pdf", "/d/문서2"),
        ] {
            s.insert_decision(&NewDecision {
                file_name: name,
                ext: Some(ext),
                src_dir: desk,
                suggested_dest: None,
                chosen_dest: Some(Path::new(dest)),
                action: DecisionAction::Move,
                source: PendingSource::Ai,
            })
            .unwrap();
        }
        s.insert_decision(&NewDecision {
            file_name: "skip.pdf",
            ext: Some("pdf"),
            src_dir: desk,
            suggested_dest: Some(Path::new("/x")),
            chosen_dest: None,
            action: DecisionAction::Skip,
            source: PendingSource::Rule,
        })
        .unwrap();
        assert_eq!(s.move_decisions().unwrap().len(), 3);
        let sim = s.similar_decisions(Some("pdf"), desk, 2).unwrap();
        assert_eq!(sim.len(), 2);
        assert!(sim.iter().all(|d| d.ext.as_deref() == Some("pdf")));
        assert_eq!(sim[0].file_name, "c.pdf"); // newest first
        assert_eq!(
            s.recent_decisions(10).unwrap()[0].action,
            DecisionAction::Skip
        );
    }

    #[test]
    fn ai_log_and_usage() {
        let s = Store::open_in_memory().unwrap();
        s.insert_ai_log(&AiLogEntry {
            provider: "anthropic".into(),
            model: "claude-haiku-5-5".into(),
            purpose: "classify".into(),
            input_tokens: Some(1000),
            output_tokens: Some(100),
            cost_usd: Some(0.00015),
            ok: true,
            ..Default::default()
        })
        .unwrap();
        s.insert_ai_log(&AiLogEntry {
            provider: "ollama".into(),
            model: "qwen3:8b".into(),
            purpose: "classify".into(),
            ok: false,
            error: Some("timeout".into()),
            ..Default::default()
        })
        .unwrap();
        let (calls, failed, inp, out, cost) = s.ai_usage().unwrap();
        assert_eq!((calls, failed, inp, out), (2, 1, 1000, 100));
        assert!((cost - 0.00015).abs() < 1e-9);
        assert_eq!(s.recent_ai_log(1).unwrap()[0].provider, "ollama");
    }

    #[test]
    fn dismissals() {
        let s = Store::open_in_memory().unwrap();
        assert!(!s.is_learn_dismissed("k").unwrap());
        s.dismiss_learn("k").unwrap();
        s.dismiss_learn("k").unwrap();
        assert!(s.is_learn_dismissed("k").unwrap());
    }
}
