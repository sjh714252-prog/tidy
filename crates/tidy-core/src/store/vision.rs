//! (M16) `files.vision`, `files.caption` and `vision_cache` (SPEC §14.13):
//! which pictures still wait for a description, and the descriptions.
//!
//! The description is also appended to `files.content` after a marker, so
//! word search, semantic search and the curate/classify prompts see it
//! without a change to the FTS tables. Recognized text (M15) stays in
//! front of it; [`compose_body`] / [`strip_caption`] keep the two apart.

use std::path::PathBuf;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::db::{Store, StoreError};
use super::models::VisionState;

/// Starts the description inside `content`.
pub const CAPTION_MARK: &str = "[사진 설명] ";

/// `text` (recognized text, may be empty) followed by the description.
pub fn compose_body(text: &str, caption: Option<&str>) -> String {
    let text = strip_caption(text).trim_end();
    match caption.map(str::trim).filter(|c| !c.is_empty()) {
        None => text.to_string(),
        Some(c) if text.is_empty() => format!("{CAPTION_MARK}{c}"),
        Some(c) => format!("{text}\n\n{CAPTION_MARK}{c}"),
    }
}

/// `content` without the description part.
pub fn strip_caption(content: &str) -> &str {
    if content.starts_with(CAPTION_MARK) {
        return "";
    }
    match content.find(&format!("\n\n{CAPTION_MARK}")) {
        Some(i) => &content[..i],
        None => content,
    }
}

/// What the model said about one picture.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caption {
    /// One or two Korean sentences ("" = nothing to describe).
    pub caption: String,
    /// Short nouns ("영수증", "회의", "화이트보드").
    pub tags: Vec<String>,
    /// photo | screenshot | document | receipt | business_card | diagram | other
    pub kind: Option<String>,
}

impl Caption {
    pub fn is_empty(&self) -> bool {
        self.caption.trim().is_empty() && self.tags.is_empty()
    }

    /// The line stored in `files.caption` and the body: "설명 (태그: a, b)".
    pub fn line(&self) -> String {
        let c = self.caption.trim();
        if self.tags.is_empty() {
            return c.to_string();
        }
        let tags = self.tags.join(", ");
        if c.is_empty() {
            format!("태그: {tags}")
        } else {
            format!("{c} (태그: {tags})")
        }
    }
}

/// A picture waiting for its description.
#[derive(Debug, Clone)]
pub struct VisionCandidate {
    pub id: i64,
    pub path: PathBuf,
    pub name: String,
    pub hash: Option<String>,
    /// Under a network root (reads take a NAS IO slot).
    pub network: bool,
}

/// Counts for `tidy vision --status` and the settings card.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct VisionCounts {
    pub pending: u64,
    pub ok: u64,
    pub empty: u64,
    pub failed: u64,
    /// Judged personal information: never sent.
    pub skipped: u64,
}

impl Store {
    /// Pending pictures already judged personal (M9) are never sent:
    /// mark them `skipped`. Returns how many.
    pub fn vision_skip_personal(&self) -> Result<usize, StoreError> {
        Ok(self.conn().execute(
            "UPDATE files SET vision = 'skipped'
             WHERE vision = 'pending' AND COALESCE(pii_flags, 0) != 0",
            [],
        )?)
    }

    /// Up to `limit` pending pictures, newest first. `wait_for_ocr`: leave
    /// pictures whose text is still being recognized for later (their
    /// personal-information verdict may change).
    pub fn vision_candidates(
        &self,
        limit: usize,
        wait_for_ocr: bool,
    ) -> Result<Vec<VisionCandidate>, StoreError> {
        let mut stmt = self.conn().prepare(
            "SELECT f.id, f.path, f.name, f.hash, r.kind = 'network'
             FROM files f JOIN roots r ON r.id = f.root_id
             WHERE f.vision = 'pending' AND f.vault = 0 AND COALESCE(f.pii_flags, 0) = 0
               AND NOT (?2 AND COALESCE(f.ocr, '') = 'pending')
             ORDER BY f.mtime DESC, f.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64, wait_for_ocr], |r| {
            Ok(VisionCandidate {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                name: r.get(2)?,
                hash: r.get(3)?,
                network: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn vision_counts(&self) -> Result<VisionCounts, StoreError> {
        let mut c = VisionCounts::default();
        let mut stmt = self.conn().prepare(
            "SELECT vision, COUNT(*) FROM files WHERE vision IS NOT NULL GROUP BY vision",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (state, n) = row?;
            let n = n as u64;
            match VisionState::parse(&state) {
                Some(VisionState::Pending) => c.pending = n,
                Some(VisionState::Ok) => c.ok = n,
                Some(VisionState::Empty) => c.empty = n,
                Some(VisionState::Failed) => c.failed = n,
                Some(VisionState::Skipped) => c.skipped = n,
                None => {}
            }
        }
        Ok(c)
    }

    /// Store the description of row `id` and append it to the body
    /// (`indexed_at` moves forward so the semantic vector is redone).
    pub fn set_vision_caption(&self, id: i64, caption: &str) -> Result<(), StoreError> {
        let content: Option<String> = self
            .conn()
            .query_row("SELECT content FROM files WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten();
        let body = compose_body(content.as_deref().unwrap_or(""), Some(caption));
        let excerpt: String = body.chars().take(500).collect();
        self.conn().execute(
            "UPDATE files SET vision = 'ok', caption = ?1, text_status = 'ok', content = ?2,
                              excerpt = ?3, indexed_at = MAX(indexed_at + 1, ?4)
             WHERE id = ?5",
            params![caption, body, excerpt, super::db::now(), id],
        )?;
        Ok(())
    }

    /// `empty`, `failed` or `skipped` (no description).
    pub fn set_vision_state(&self, id: i64, state: VisionState) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE files SET vision = ?1 WHERE id = ?2",
            params![state.as_str(), id],
        )?;
        Ok(())
    }

    /// The stored description of `id` (kept when OCR rewrites the body).
    pub fn file_caption(&self, id: i64) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn()
            .query_row("SELECT caption FROM files WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .flatten())
    }

    /// Failed pictures wait again (`tidy vision --retry`). Returns how many.
    pub fn vision_retry_failed(&self) -> Result<usize, StoreError> {
        Ok(self.conn().execute(
            "UPDATE files SET vision = 'pending' WHERE vision = 'failed'",
            [],
        )?)
    }

    pub fn vision_cache_get(&self, hash: &str) -> Result<Option<Caption>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT caption, tags, kind FROM vision_cache WHERE hash = ?1",
                [hash],
                |r| {
                    let tags: String = r.get(1)?;
                    Ok(Caption {
                        caption: r.get(0)?,
                        tags: serde_json::from_str(&tags).unwrap_or_default(),
                        kind: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn vision_cache_put(&self, hash: &str, c: &Caption, model: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO vision_cache (hash, caption, tags, kind, model, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(hash) DO UPDATE SET caption = excluded.caption, tags = excluded.tags,
                 kind = excluded.kind, model = excluded.model, created_at = excluded.created_at",
            params![
                hash,
                c.caption,
                serde_json::to_string(&c.tags).unwrap_or_else(|_| "[]".into()),
                c.kind,
                model,
                super::db::now()
            ],
        )?;
        Ok(())
    }

    /// Pictures sent since `since` (unix seconds) and what they cost so far
    /// in total: `(sent_since, total_sent, total_cost_usd)`. From `ai_log`
    /// (purpose `vision`, answered calls only).
    pub fn vision_usage(&self, since: i64) -> Result<(u64, u64, f64), StoreError> {
        Ok(self.conn().query_row(
            "SELECT COALESCE(SUM(called_at >= ?1), 0), COUNT(*), COALESCE(SUM(cost_usd), 0.0)
             FROM ai_log WHERE purpose = 'vision' AND ok = 1",
            [since],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, f64>(2)?,
                ))
            },
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_keeps_text_and_caption_apart() {
        assert_eq!(compose_body("", Some("영수증")), "[사진 설명] 영수증");
        let b = compose_body("합계 12,000원", Some("카페 영수증"));
        assert_eq!(b, "합계 12,000원\n\n[사진 설명] 카페 영수증");
        assert_eq!(strip_caption(&b), "합계 12,000원");
        // A new caption replaces the old one.
        assert_eq!(
            compose_body(&b, Some("식당 영수증")),
            "합계 12,000원\n\n[사진 설명] 식당 영수증"
        );
        assert_eq!(compose_body(&b, None), "합계 12,000원");
        assert_eq!(strip_caption("[사진 설명] x"), "");
    }

    #[test]
    fn caption_line() {
        let c = Caption {
            caption: "화이트보드에 그린 네트워크 구성도".into(),
            tags: vec!["회의".into(), "화이트보드".into()],
            kind: Some("diagram".into()),
        };
        assert_eq!(
            c.line(),
            "화이트보드에 그린 네트워크 구성도 (태그: 회의, 화이트보드)"
        );
        assert!(Caption::default().is_empty());
    }
}
