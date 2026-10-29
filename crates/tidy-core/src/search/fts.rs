//! FTS5 search over the index (SPEC §5.6).
//!
//! Terms are routed by length (docs/fts-tokenizer.md): terms of three or
//! more characters hit `files_fts` (trigram: substring matching, so
//! `과금내역` finds `KT과금내역서.xlsx`), shorter terms hit `files_fts_word`
//! (unicode61) as prefix queries (`과금*`), because a trigram index cannot
//! answer a query shorter than three characters. Ranking is BM25 with the
//! file name weighted 3× the body.

use std::path::Path;

use rusqlite::types::Value;
use serde::Serialize;

use super::query::Query;
use crate::store::{FileRecord, Store, StoreError};

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    /// Markers around matched text in `snippet` / `name_highlight`.
    pub mark_start: String,
    pub mark_end: String,
    /// Approximate snippet length in tokens (FTS5 `snippet()` argument).
    pub snippet_tokens: u32,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 50,
            mark_start: "[".into(),
            mark_end: "]".into(),
            snippet_tokens: 24,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchedIn {
    Name,
    Content,
    Both,
    /// Filter-only query (no terms): nothing to highlight.
    Filter,
    /// Found by meaning only (semantic search), no word matched.
    Semantic,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub file: FileRecord,
    pub matched_in: MatchedIn,
    /// File name with matches marked.
    pub name_highlight: String,
    /// Body excerpt around the first match (empty if the body did not match).
    pub snippet: String,
    /// Lower is better (FTS5 bm25; hybrid results: minus the fused score).
    pub rank: f64,
    /// Cosine similarity to the query when semantic search ranked it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f32>,
}

/// Minimum term length (in characters) the trigram index can answer.
pub const TRIGRAM_MIN_CHARS: usize = 3;

const FILE_COLS: &str = "f.id, f.path, f.root_id, f.name, f.ext, f.mime, f.size, f.mtime, \
                         f.hash, f.text_status, f.excerpt, f.indexed_at, f.pii_flags, \
                         f.pii_counts, f.vault, f.ocr, f.vision";

pub fn search(
    store: &Store,
    q: &Query,
    opts: &SearchOptions,
) -> Result<Vec<SearchHit>, StoreError> {
    let (long, short): (Vec<&str>, Vec<&str>) = q
        .terms
        .iter()
        .map(String::as_str)
        .partition(|t| t.chars().count() >= TRIGRAM_MIN_CHARS);
    let trigram_match =
        (!long.is_empty()).then(|| long.iter().map(|t| quote(t)).collect::<Vec<_>>().join(" "));
    let word_match = {
        let parts: Vec<String> = short
            .iter()
            .filter(|t| t.chars().any(char::is_alphanumeric))
            .map(|t| format!("{}*", quote(t)))
            .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    };

    let mut params: Vec<Value> = Vec::new();
    let mut sql = String::new();

    // Which FTS table drives the query (and provides snippet/highlight).
    let primary = match (&trigram_match, &word_match) {
        (Some(m), _) => Some(("files_fts", m.clone())),
        (None, Some(m)) => Some(("files_fts_word", m.clone())),
        (None, None) => None,
    };

    match &primary {
        Some((table, m)) => {
            sql.push_str(&format!(
                "SELECT {FILE_COLS}, COALESCE(highlight({table}, 0, ?, ?), f.name) AS hl_name, \
                 COALESCE(snippet({table}, 1, ?, ?, '…', ?), '') AS snip, \
                 bm25({table}, 3.0, 1.0) AS rank \
                 FROM {table} JOIN files f ON f.id = {table}.rowid WHERE {table} MATCH ?"
            ));
            params.push(opts.mark_start.clone().into());
            params.push(opts.mark_end.clone().into());
            params.push(opts.mark_start.clone().into());
            params.push(opts.mark_end.clone().into());
            params.push(i64::from(opts.snippet_tokens).into());
            params.push(m.clone().into());
            if *table == "files_fts" {
                if let Some(w) = &word_match {
                    sql.push_str(
                        " AND f.id IN (SELECT rowid FROM files_fts_word WHERE files_fts_word MATCH ?)",
                    );
                    params.push(w.clone().into());
                }
            }
        }
        None => {
            sql.push_str(&format!(
                "SELECT {FILE_COLS}, f.name AS hl_name, '' AS snip, 0.0 AS rank FROM files f WHERE 1"
            ));
        }
    }

    push_filters(q, &mut sql, &mut params)?;

    sql.push_str(if primary.is_some() {
        " ORDER BY rank, f.mtime DESC LIMIT ?"
    } else {
        " ORDER BY f.mtime DESC LIMIT ?"
    });
    params.push((opts.limit as i64).into());

    let mut stmt = store.conn().prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        let file = crate::store::file_from_row(r)?;
        let name_highlight: String = r.get(17)?;
        let snippet: String = r.get(18)?;
        let rank: f64 = r.get(19)?;
        Ok((file, name_highlight, snippet, rank))
    })?;

    let mut hits = Vec::new();
    for row in rows {
        let (file, name_highlight, snippet, rank) = row?;
        let in_name = name_highlight.contains(opts.mark_start.as_str());
        let in_body = snippet.contains(opts.mark_start.as_str());
        let matched_in = match (primary.is_some(), in_name, in_body) {
            (false, _, _) => MatchedIn::Filter,
            (true, true, true) => MatchedIn::Both,
            (true, true, false) => MatchedIn::Name,
            (true, false, true) => MatchedIn::Content,
            // Hybrid case: the short term matched via the word table only.
            (true, false, false) => MatchedIn::Content,
        };
        hits.push(SearchHit {
            file,
            matched_in,
            name_highlight,
            snippet: if in_body { snippet } else { String::new() },
            rank,
            similarity: None,
        });
    }
    Ok(hits)
}

/// Ids of all files passing `q`'s filters, or `None` when it has none
/// (semantic search scores only these).
pub(crate) fn filtered_ids(
    store: &Store,
    q: &Query,
) -> Result<Option<std::collections::HashSet<i64>>, StoreError> {
    let mut sql = String::from("SELECT f.id FROM files f WHERE 1");
    let mut params = Vec::new();
    push_filters(q, &mut sql, &mut params)?;
    if params.is_empty() {
        return Ok(None);
    }
    let mut stmt = store.conn().prepare(&sql)?;
    let ids = stmt
        .query_map(rusqlite::params_from_iter(params), |r| r.get::<_, i64>(0))?
        .collect::<Result<_, _>>()?;
    Ok(Some(ids))
}

/// `dir` + separator, canonical when it exists (stored paths are).
fn dir_prefix(dir: &std::path::Path) -> Result<String, StoreError> {
    let dir = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut prefix = dir
        .to_str()
        .ok_or_else(|| StoreError::NonUtf8Path(dir.clone()))?
        .to_string();
    if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
        prefix.push(std::path::MAIN_SEPARATOR);
    }
    Ok(prefix)
}

fn push_filters(q: &Query, sql: &mut String, params: &mut Vec<Value>) -> Result<(), StoreError> {
    if !q.exts.is_empty() {
        let marks = vec!["?"; q.exts.len()].join(", ");
        sql.push_str(&format!(" AND f.ext IN ({marks})"));
        params.extend(q.exts.iter().map(|e| Value::from(e.clone())));
    }
    if let Some(dir) = &q.in_dir {
        let prefix = dir_prefix(dir)?;
        // substr() counts characters, not bytes (Korean paths!).
        sql.push_str(" AND substr(f.path, 1, ?) = ?");
        params.push((prefix.chars().count() as i64).into());
        params.push(prefix.into());
    }
    if !q.in_any.is_empty() {
        let mut parts = Vec::new();
        for dir in &q.in_any {
            let prefix = dir_prefix(dir)?;
            parts.push("substr(f.path, 1, ?) = ?");
            params.push((prefix.chars().count() as i64).into());
            params.push(prefix.into());
        }
        sql.push_str(&format!(" AND ({})", parts.join(" OR ")));
    }
    if let Some(t) = q.after {
        sql.push_str(" AND f.mtime >= ?");
        params.push(t.into());
    }
    if let Some(t) = q.before {
        sql.push_str(" AND f.mtime < ?");
        params.push(t.into());
    }
    if let Some(n) = q.min_size {
        sql.push_str(" AND f.size >= ?");
        params.push((n as i64).into());
    }
    if let Some(n) = q.max_size {
        sql.push_str(" AND f.size <= ?");
        params.push((n as i64).into());
    }
    Ok(())
}

/// FTS5 string literal: double quotes, inner quotes doubled.
fn quote(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// Convenience for callers that only have a path and want its row.
pub fn lookup(store: &Store, path: &Path) -> Result<Option<FileRecord>, StoreError> {
    store.get_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RootKind;
    use crate::store::{NewFile, TextStatus};

    /// An absolute root that looks like what the indexer stores on this OS
    /// (Windows paths carry a drive; `/r` would be rooted but driveless).
    fn root() -> std::path::PathBuf {
        if cfg!(windows) {
            std::path::PathBuf::from(r"C:\r")
        } else {
            std::path::PathBuf::from("/r")
        }
    }

    fn store_with(docs: &[(&str, &str)]) -> Store {
        let s = Store::open_in_memory().unwrap();
        let root = s
            .upsert_root(&root(), RootKind::Local, false, true)
            .unwrap();
        for (name, body) in docs {
            s.upsert_file(
                &NewFile {
                    path: self::root().join(name),
                    root_id: root,
                    name: name.to_string(),
                    ext: Path::new(name)
                        .extension()
                        .map(|e| e.to_string_lossy().into_owned()),
                    mime: None,
                    size: body.len() as u64,
                    mtime: 1_760_000_000,
                    ctime: None,
                    hash: None,
                    text_status: TextStatus::Ok,
                    excerpt: None,
                    content: (!body.is_empty()).then(|| body.to_string()),
                    pii: None,
                    ocr: None,
                    vision: None,
                    caption: None,
                },
                1,
            )
            .unwrap();
        }
        s
    }

    fn names(s: &Store, q: &str) -> Vec<String> {
        let mut v: Vec<String> = search(s, &Query::parse(q), &SearchOptions::default())
            .unwrap()
            .into_iter()
            .map(|h| h.file.name)
            .collect();
        v.sort();
        v
    }

    /// The cases from docs/fts-tokenizer.md that drove the hybrid design.
    #[test]
    fn korean_compounds_and_short_terms() {
        let s = store_with(&[
            ("KT과금내역서_2026-09.xlsx", "9월 인터넷 회선 과금 33,000원"),
            (
                "회의록_20261015.docx",
                "과금내역서는 매월 5일까지 제출한다.",
            ),
            ("사업제안서(최종).docx", "KT 과금 시스템 개선 제안"),
            ("분기 보고서 초안.pdf", "보고서를 임원진에게 공유함"),
        ]);
        // trigram: inside compounds, no spaces needed.
        assert_eq!(
            names(&s, "내역서"),
            ["KT과금내역서_2026-09.xlsx", "회의록_20261015.docx"]
        );
        assert_eq!(names(&s, "제안서"), ["사업제안서(최종).docx"]);
        // 2-char terms go to the unicode61 prefix path instead of returning nothing.
        assert_eq!(
            names(&s, "과금"),
            [
                "KT과금내역서_2026-09.xlsx",
                "사업제안서(최종).docx",
                "회의록_20261015.docx"
            ]
        );
        assert_eq!(
            names(&s, "KT"),
            ["KT과금내역서_2026-09.xlsx", "사업제안서(최종).docx"]
        );
        // Mixed lengths are ANDed across both tables.
        assert_eq!(names(&s, "과금 제안"), ["사업제안서(최종).docx"]);
        // Name matches outrank body-only matches.
        let hits = search(&s, &Query::parse("보고서"), &SearchOptions::default()).unwrap();
        assert_eq!(hits[0].file.name, "분기 보고서 초안.pdf");
        assert_eq!(hits[0].matched_in, MatchedIn::Both);
        assert!(hits[0].name_highlight.contains("[보고서]"));
        assert!(hits[0].snippet.contains("[보고서]"), "{}", hits[0].snippet);
    }

    #[test]
    fn quotes_and_punctuation_are_safe() {
        let s = store_with(&[("a\"b.txt", "has \"quotes\" and (parens) AND NOT OR")]);
        assert_eq!(names(&s, "\"quotes\""), ["a\"b.txt"]);
        assert_eq!(names(&s, "(parens)"), ["a\"b.txt"]);
        assert_eq!(names(&s, "AND"), ["a\"b.txt"]);
        assert_eq!(names(&s, "a\"b"), ["a\"b.txt"]);
        // Only punctuation in a short term: no word-table constraint, no error.
        assert_eq!(names(&s, "quotes .."), ["a\"b.txt"]);
    }

    #[test]
    fn filters_combine_with_terms() {
        let s = store_with(&[("a.pdf", "same body"), ("b.txt", "same body")]);
        assert_eq!(names(&s, "body ext:pdf"), ["a.pdf"]);
        assert_eq!(names(&s, "body type:doc"), ["a.pdf", "b.txt"]);
        assert_eq!(names(&s, "body after:2030"), Vec::<String>::new());
        assert_eq!(names(&s, "ext:txt"), ["b.txt"]);
        let inside = format!("in:\"{}\"", root().display());
        assert_eq!(names(&s, &inside), ["a.pdf", "b.txt"]);
        let other = format!("in:\"{}\"", root().with_file_name("other").display());
        assert_eq!(names(&s, &other), Vec::<String>::new());
    }
}
