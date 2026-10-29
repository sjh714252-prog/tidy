//! Semantic search (SPEC §5.6, M7): embeddings of name + start of the
//! body, brute-force cosine similarity over an in-memory int8 copy of the
//! vectors, merged with the FTS results by reciprocal rank fusion.
//!
//! Always optional (CLAUDE.md rule 4): without an embedding model, with the
//! AI off, or when the model fails or is slow, [`hybrid_search`] returns
//! the plain FTS results and says why in [`SearchOutcome::note`].
//!
//! Storage decision (SPEC §13): vectors live as f32 BLOBs in SQLite (no
//! sqlite-vec extension to ship per platform). Queries scan an int8 copy
//! (¼ of the memory; ~25 ms for 100k × 768 in release builds, see the
//! ignored `bench_100k` test), loaded once and then refreshed
//! incrementally.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::fts::{filtered_ids, search as fts_search, MatchedIn, SearchHit, SearchOptions};
use super::query::Query;
use crate::ai::{build_embedder, estimate_cost_usd, AiError, AiHealth, EmbeddingProvider};
use crate::config::{Config, SearchCfg};
use crate::store::{AiLogEntry, EmbedCandidate, Store, StoreError};

/// Reciprocal rank fusion constant (the usual 60).
const RRF_K: f64 = 60.0;
/// Weight of the vector list for keyword-like / question-like queries.
const VECTOR_WEIGHT_KEYWORDS: f64 = 0.7;
const VECTOR_WEIGHT_QUESTION: f64 = 1.5;
/// Unreferenced vectors are kept this long so moved files reuse them.
const ORPHAN_VECTOR_TTL_SECS: i64 = 7 * 24 * 3600;
/// Query vectors remembered (typing the same query again is free).
const QUERY_CACHE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Words only (no model, AI off, or the model failed: see `note`).
    Fts,
    /// Words and meaning, fused.
    Hybrid,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub mode: SearchMode,
    /// The query read like a question / sentence (meaning weighted up).
    pub natural: bool,
    /// Why semantic search did not take part (shown under the results).
    pub note: Option<String>,
}

/// Result of one embedding pass over the index.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EmbedReport {
    /// Files that got a freshly computed vector.
    pub embedded: usize,
    /// Files that reused a stored vector (moved / copied / unchanged text).
    pub reused: usize,
    /// Files still waiting (pass limit reached or stopped by an error).
    pub remaining: u64,
    /// Provider error that stopped the pass (AI is never required).
    pub error: Option<String>,
}

pub struct SemanticSearch {
    provider: Arc<dyn EmbeddingProvider>,
    key: String,
    cfg: SearchCfg,
    send_content: bool,
    embed_timeout: Duration,
    health: AiHealth,
    rt: tokio::runtime::Runtime,
    cache: Mutex<Option<VectorCache>>,
    queries: Mutex<VecDeque<(String, Arc<Vec<f32>>)>>,
}

impl std::fmt::Debug for SemanticSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticSearch")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl SemanticSearch {
    /// The semantic search for `cfg`, or why there is none (AI off, no
    /// `embed` model, Anthropic, missing key…).
    pub fn from_config(cfg: &Config) -> Result<Self, AiError> {
        if !cfg.ai.enabled || !cfg.search.semantic {
            return Err(AiError::Disabled);
        }
        let pc = cfg
            .ai
            .active()
            .and_then(|p| p.embed.as_ref())
            .ok_or_else(|| AiError::NoEmbedModel(cfg.ai.active_profile.clone()))?;
        let timeout = Duration::from_secs(cfg.ai.timeout_secs.max(1));
        let provider = build_embedder(pc, timeout, cfg.ai.max_retries)?;
        Self::with_provider(provider, cfg)
    }

    /// Use an explicit provider (tests use [`crate::ai::MockEmbedder`]).
    pub fn with_provider(
        provider: Arc<dyn EmbeddingProvider>,
        cfg: &Config,
    ) -> Result<Self, AiError> {
        let send_content = cfg.ai.send_content();
        let key = format!(
            "{}/{}{}",
            provider.name(),
            provider.model(),
            if send_content { "" } else { "#name" }
        );
        // Multi-thread (one worker) so the UI's search threads and the
        // background embedding thread can block on it at the same time.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("tidy-embed-rt")
            .enable_all()
            .build()
            .map_err(|e| AiError::Runtime(e.to_string()))?;
        Ok(Self {
            provider,
            key,
            cfg: cfg.search.clone(),
            send_content,
            embed_timeout: Duration::from_secs(cfg.ai.timeout_secs.max(1)),
            health: AiHealth::new(cfg.ai.unavailable_after),
            rt,
            cache: Mutex::new(None),
            queries: Mutex::new(VecDeque::new()),
        })
    }

    /// One tiny embedding request ("연결 테스트" in the settings screen);
    /// logged in `ai_log` like any other call. Returns the vector size.
    pub fn test_connection(&self, store: &Store) -> Result<usize, AiError> {
        let v = self.embed_logged(
            store,
            "test",
            &["연결 테스트".to_string()],
            self.embed_timeout,
        )?;
        Ok(v.first().map(Vec::len).unwrap_or(0))
    }

    /// Embedder key stored with every vector (`provider/model[#name]`).
    pub fn model_key(&self) -> &str {
        &self.key
    }

    /// "ollama / nomic-embed-text".
    pub fn describe(&self) -> String {
        format!("{} / {}", self.provider.name(), self.provider.model())
    }

    pub fn health(&self) -> &AiHealth {
        &self.health
    }

    /// Embed up to `max_files` files whose vector is missing or stale.
    /// Files whose text already has a vector (moved, copied, re-indexed
    /// without change) reuse it without a request. Stops at the first
    /// provider error (reported, never raised: search keeps working).
    pub fn embed_pending(
        &self,
        store: &Store,
        max_files: usize,
        mut progress: impl FnMut(usize),
    ) -> Result<EmbedReport, StoreError> {
        let mut report = EmbedReport::default();
        let chars = if self.send_content {
            self.cfg.embed_chars
        } else {
            0
        };
        let batch = self.cfg.embed_batch.max(1);
        let mut done = 0usize;
        // Candidates that failed to embed in this pass are not retried in it.
        let mut skip: HashSet<i64> = HashSet::new();
        while done < max_files {
            if !self.health.is_available() && !self.health.probe_due() {
                report.error =
                    Some(AiError::Paused(self.health.last_error().unwrap_or_default()).to_string());
                break;
            }
            let want = (batch * 4).min(max_files - done) + skip.len();
            let cands: Vec<EmbedCandidate> = store
                .embed_candidates(&self.key, chars, want)?
                .into_iter()
                .filter(|c| !skip.contains(&c.file_id))
                .collect();
            if cands.is_empty() {
                break;
            }
            // Group by text: one vector per distinct text.
            let mut groups: Vec<(String, String, Vec<&EmbedCandidate>)> = Vec::new();
            let mut by_hash: HashMap<String, usize> = HashMap::new();
            for c in &cands {
                let text = embed_text(&c.name, c.text.as_deref());
                let h = text_hash(&text);
                match by_hash.get(&h) {
                    Some(&i) => groups[i].2.push(c),
                    None => {
                        by_hash.insert(h.clone(), groups.len());
                        groups.push((h, text, vec![c]));
                    }
                }
            }
            let mut need = Vec::new();
            store.transaction(|s| {
                for (h, text, files) in &groups {
                    if s.has_vector(&self.key, h)? {
                        for f in files {
                            s.map_file_vector(f.file_id, &self.key, h, f.indexed_at)?;
                        }
                        report.reused += files.len();
                        done += files.len();
                    } else {
                        need.push((h.clone(), text.clone(), files.clone()));
                    }
                }
                Ok(())
            })?;
            let mut failed = false;
            for chunk in need.chunks(batch) {
                let texts: Vec<String> = chunk.iter().map(|(_, t, _)| t.clone()).collect();
                match self.embed_logged(store, "embed", &texts, self.embed_timeout) {
                    Ok(vectors) => {
                        store.transaction(|s| {
                            for ((h, _, files), v) in chunk.iter().zip(&vectors) {
                                s.put_vector(&self.key, h, v)?;
                                for f in files {
                                    s.map_file_vector(f.file_id, &self.key, h, f.indexed_at)?;
                                }
                                report.embedded += files.len();
                                done += files.len();
                            }
                            Ok(())
                        })?;
                        progress(done);
                    }
                    Err(e) => {
                        report.error = Some(e.to_string());
                        for (_, _, files) in chunk {
                            skip.extend(files.iter().map(|f| f.file_id));
                        }
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                break;
            }
        }
        let cutoff = crate::store::now_unix() - ORPHAN_VECTOR_TTL_SECS;
        let pruned = store.prune_vectors(cutoff)?;
        if pruned > 0 {
            tracing::info!(pruned, "unreferenced vectors pruned");
        }
        report.remaining = store.embed_backlog(&self.key)?;
        Ok(report)
    }

    /// Normalized vector for a query (cached), bounded by
    /// `search.query_timeout_ms`.
    pub fn query_vector(&self, store: &Store, text: &str) -> Result<Arc<Vec<f32>>, AiError> {
        if let Some(v) = self
            .queries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(t, _)| t == text)
            .map(|(_, v)| Arc::clone(v))
        {
            return Ok(v);
        }
        if !self.health.is_available() && !self.health.probe_due() {
            return Err(AiError::Paused(
                self.health.last_error().unwrap_or_default(),
            ));
        }
        let timeout = Duration::from_millis(self.cfg.query_timeout_ms.max(100));
        let mut v = self.embed_logged(store, "search", &[text.to_string()], timeout)?;
        let v = Arc::new(v.pop().unwrap_or_default());
        let mut q = self.queries.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= QUERY_CACHE {
            q.pop_front();
        }
        q.push_back((text.to_string(), Arc::clone(&v)));
        Ok(v)
    }

    /// One provider call: bounded by `timeout`, logged in `ai_log`, health
    /// updated. Vectors come back L2-normalized.
    fn embed_logged(
        &self,
        store: &Store,
        purpose: &str,
        texts: &[String],
        timeout: Duration,
    ) -> Result<Vec<Vec<f32>>, AiError> {
        let started = Instant::now();
        let fut = self.provider.embed(texts);
        let res = self
            .rt
            .block_on(async { tokio::time::timeout(timeout, fut).await })
            .unwrap_or_else(|_| Err(AiError::Timeout(timeout.as_secs().max(1))))
            .and_then(|r| {
                if r.vectors.len() == texts.len() {
                    Ok(r)
                } else {
                    Err(AiError::BadResponse("vector count mismatch".into()))
                }
            });
        let usage = res.as_ref().map(|r| r.usage).unwrap_or_default();
        let model = self.provider.model().to_string();
        let log = AiLogEntry {
            provider: self.provider.name().to_string(),
            model: model.clone(),
            purpose: purpose.to_string(),
            input_tokens: usage.input_tokens,
            output_tokens: None,
            cost_usd: estimate_cost_usd(self.provider.name(), &model, usage),
            latency_ms: Some(started.elapsed().as_millis() as u64),
            ok: res.is_ok(),
            error: res.as_ref().err().map(|e| e.to_string()),
            result: res.as_ref().ok().map(|_| format!("{} texts", texts.len())),
            ..Default::default()
        };
        if let Err(e) = store.insert_ai_log(&log) {
            tracing::warn!("ai_log write failed: {e}");
        }
        match res {
            Ok(r) => {
                self.health.record_success();
                Ok(r.vectors.into_iter().map(normalized).collect())
            }
            Err(e) => {
                self.health.record_failure(&e.to_string());
                Err(e)
            }
        }
    }

    /// Top `k` files by cosine similarity to `qv` (≥ `min_similarity`),
    /// restricted to `allowed` when given. Best first.
    pub fn nearest(
        &self,
        store: &Store,
        qv: &[f32],
        allowed: Option<&HashSet<i64>>,
        k: usize,
    ) -> Result<Vec<(i64, f32)>, StoreError> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let cache = VectorCache::refresh(guard.take(), store, &self.key)?;
        let out = cache.nearest(qv, allowed, k, self.cfg.min_similarity);
        *guard = Some(cache);
        Ok(out)
    }
}

/// What is embedded for a file: its name (separators as spaces, so the
/// words are visible to the model) and the start of its text.
pub fn embed_text(name: &str, text: Option<&str>) -> String {
    let readable: String = name
        .chars()
        .map(|c| if matches!(c, '_' | '-' | '.') { ' ' } else { c })
        .collect();
    match text {
        Some(t) => format!("{}\n{}", readable.trim(), t.trim()),
        None => readable.trim().to_string(),
    }
}

fn text_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..32].to_string()
}

fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 && n.is_finite() {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// Does the query read like a sentence or question rather than keywords?
pub fn is_natural_language(terms: &[String]) -> bool {
    const CUES: &[&str] = &[
        "뭐",
        "무엇",
        "어디",
        "언제",
        "어떤",
        "어떻게",
        "왜",
        "누가",
        "관련",
        "대한",
        "같은",
        "찾아",
        "있던",
        "했던",
        "받은",
        "보낸",
        "만든",
        "있지",
        "였",
        "지난",
        "작년",
        "what",
        "where",
        "when",
        "which",
        "how",
        "why",
        "who",
        "about",
        "related",
        "the",
    ];
    if terms.iter().any(|t| t.ends_with('?')) || terms.len() >= 4 {
        return true;
    }
    terms.iter().any(|t| {
        let t = t.to_lowercase();
        CUES.iter().any(|c| t.contains(c))
    })
}

/// FTS, plus semantic ranking when `sem` is available. Never fails
/// because of the AI: any embedding problem gives the FTS results with a
/// note.
pub fn hybrid_search(
    store: &Store,
    sem: Option<&SemanticSearch>,
    q: &Query,
    opts: &SearchOptions,
) -> Result<SearchOutcome, StoreError> {
    let natural = is_natural_language(&q.terms);
    let fts_only = |hits, note: Option<String>| SearchOutcome {
        hits,
        mode: SearchMode::Fts,
        natural,
        note,
    };
    let Some(sem) = sem else {
        return Ok(fts_only(fts_search(store, q, opts)?, None));
    };
    if q.terms.is_empty() {
        // Filter-only query: nothing to mean.
        return Ok(fts_only(fts_search(store, q, opts)?, None));
    }
    let k = sem.cfg.vector_k.max(opts.limit);
    let fts_opts = SearchOptions {
        limit: k,
        ..opts.clone()
    };
    let fts = fts_search(store, q, &fts_opts)?;
    let text = q.terms.join(" ");
    let qv = match sem.query_vector(store, &text) {
        Ok(v) => v,
        Err(e) => {
            tracing::info!("semantic search skipped: {e}");
            let mut hits = fts;
            hits.truncate(opts.limit);
            return Ok(fts_only(
                hits,
                Some(format!("의미검색 사용 불가 — 단어 검색만: {e}")),
            ));
        }
    };
    let allowed = filtered_ids(store, q)?;
    let near = sem.nearest(store, &qv, allowed.as_ref(), k)?;
    let hits = fuse(store, fts, &near, natural, opts)?;
    Ok(SearchOutcome {
        hits,
        mode: SearchMode::Hybrid,
        natural,
        note: None,
    })
}

/// Reciprocal rank fusion of the FTS list and the vector list.
fn fuse(
    store: &Store,
    fts: Vec<SearchHit>,
    near: &[(i64, f32)],
    natural: bool,
    opts: &SearchOptions,
) -> Result<Vec<SearchHit>, StoreError> {
    let w_vec = if natural {
        VECTOR_WEIGHT_QUESTION
    } else {
        VECTOR_WEIGHT_KEYWORDS
    };
    let sim: HashMap<i64, f32> = near.iter().copied().collect();
    let mut scored: HashMap<i64, (f64, SearchHit)> = HashMap::new();
    for (i, mut h) in fts.into_iter().enumerate() {
        h.similarity = sim.get(&h.file.id).copied();
        scored.insert(h.file.id, (1.0 / (RRF_K + i as f64 + 1.0), h));
    }
    let missing: Vec<i64> = near
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !scored.contains_key(id))
        .collect();
    let mut records: HashMap<i64, crate::store::FileRecord> = store
        .get_files_by_ids(&missing)?
        .into_iter()
        .map(|f| (f.id, f))
        .collect();
    for (i, (id, s)) in near.iter().enumerate() {
        let add = w_vec / (RRF_K + i as f64 + 1.0);
        if let Some(e) = scored.get_mut(id) {
            e.0 += add;
            continue;
        }
        // Deleted since the cache was built: skip.
        let Some(file) = records.remove(id) else {
            continue;
        };
        let snippet = file
            .excerpt
            .as_deref()
            .map(|e| {
                let flat: String = e.split_whitespace().collect::<Vec<_>>().join(" ");
                let cut: String = flat.chars().take(120).collect();
                if flat.chars().count() > 120 {
                    format!("{cut}…")
                } else {
                    cut
                }
            })
            .unwrap_or_default();
        let hit = SearchHit {
            name_highlight: file.name.clone(),
            file,
            matched_in: MatchedIn::Semantic,
            snippet,
            rank: 0.0,
            similarity: Some(*s),
        };
        scored.insert(*id, (add, hit));
    }
    let mut out: Vec<(f64, SearchHit)> = scored.into_values().collect();
    out.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.1.file.mtime.cmp(&a.1.file.mtime))
    });
    Ok(out
        .into_iter()
        .take(opts.limit)
        .map(|(score, mut h)| {
            h.rank = -score;
            h
        })
        .collect())
}

/// int8 copy of one model's vectors, refreshed from `file_vectors`.
struct VectorCache {
    key: String,
    /// `vector_signature` when last synced.
    count: i64,
    max_seq: i64,
    dim: usize,
    ids: Vec<i64>,
    slot: HashMap<i64, usize>,
    data: Vec<i8>,
    scales: Vec<f32>,
}

impl VectorCache {
    fn empty(key: &str) -> Self {
        Self {
            key: key.to_string(),
            count: 0,
            max_seq: 0,
            dim: 0,
            ids: Vec::new(),
            slot: HashMap::new(),
            data: Vec::new(),
            scales: Vec::new(),
        }
    }

    fn refresh(prev: Option<Self>, store: &Store, key: &str) -> Result<Self, StoreError> {
        let (count, max_seq) = store.vector_signature(key)?;
        let mut c = match prev {
            Some(c) if c.key == key => c,
            _ => Self::empty(key),
        };
        if (c.count, c.max_seq) == (count, max_seq) {
            return Ok(c);
        }
        // Rows only removed or many replaced: stale slots pile up → rebuild.
        let stale = c.ids.len() as i64 - count;
        if max_seq < c.max_seq || stale > (count / 10).max(1000) {
            c = Self::empty(key);
        }
        let started = Instant::now();
        let after = c.max_seq;
        store.for_each_vector(key, after, |row| c.upsert(row.file_id, &row.vec))?;
        c.count = count;
        c.max_seq = max_seq;
        tracing::debug!(
            vectors = c.ids.len(),
            ms = started.elapsed().as_millis() as u64,
            "vector cache refreshed"
        );
        Ok(c)
    }

    fn upsert(&mut self, file_id: i64, v: &[f32]) {
        if self.dim == 0 {
            self.dim = v.len();
        }
        if v.len() != self.dim {
            return; // another dimension under the same key: ignore
        }
        let max = v.iter().fold(0f32, |m, x| m.max(x.abs()));
        let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
        let q = v
            .iter()
            .map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8);
        match self.slot.get(&file_id) {
            Some(&i) => {
                let d = self.dim;
                for (dst, src) in self.data[i * d..(i + 1) * d].iter_mut().zip(q) {
                    *dst = src;
                }
                self.scales[i] = scale;
            }
            None => {
                self.slot.insert(file_id, self.ids.len());
                self.ids.push(file_id);
                self.data.extend(q);
                self.scales.push(scale);
            }
        }
    }

    fn nearest(
        &self,
        qv: &[f32],
        allowed: Option<&HashSet<i64>>,
        k: usize,
        min_similarity: f32,
    ) -> Vec<(i64, f32)> {
        if self.dim == 0 || qv.len() != self.dim || k == 0 {
            return Vec::new();
        }
        // The query is quantized too: an i8·i8 dot product with i32 sums
        // vectorizes well (≈4× faster than f32 here), and the error stays
        // around 0.01 in cosine.
        let qmax = qv.iter().fold(0f32, |m, x| m.max(x.abs()));
        let qscale = if qmax > 0.0 { qmax / 127.0 } else { 1.0 };
        let q8: Vec<i8> = qv
            .iter()
            .map(|x| (x / qscale).round().clamp(-127.0, 127.0) as i8)
            .collect();
        let mut hits: Vec<(i64, f32)> = Vec::new();
        for (i, (&id, chunk)) in self
            .ids
            .iter()
            .zip(self.data.chunks_exact(self.dim))
            .enumerate()
        {
            if allowed.is_some_and(|a| !a.contains(&id)) {
                continue;
            }
            let dot: i32 = chunk
                .iter()
                .zip(&q8)
                .map(|(&a, &b)| i32::from(a) * i32::from(b))
                .sum();
            let s = dot as f32 * self.scales[i] * qscale;
            if s >= min_similarity {
                hits.push((id, s));
            }
        }
        if hits.len() > k {
            hits.select_nth_unstable_by(k - 1, |a, b| b.1.total_cmp(&a.1));
            hits.truncate(k);
        }
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::MockEmbedder;
    use crate::config::{AiProfile, RootKind};
    use crate::store::{NewFile, TextStatus};
    use std::path::{Path, PathBuf};

    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\r")
        } else {
            PathBuf::from("/r")
        }
    }

    fn add(s: &Store, name: &str, body: &str, mtime: i64) -> i64 {
        let root = s
            .upsert_root(&root(), RootKind::Local, false, true)
            .unwrap();
        s.upsert_file(
            &NewFile {
                path: self::root().join(name),
                root_id: root,
                name: name.into(),
                ext: Path::new(name)
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned()),
                mime: None,
                size: body.len() as u64,
                mtime,
                ctime: None,
                hash: None,
                text_status: TextStatus::Ok,
                excerpt: Some(body.chars().take(500).collect()),
                content: Some(body.into()),
                pii: None,
                ocr: None,
                vision: None,
                caption: None,
            },
            mtime,
        )
        .unwrap()
    }

    fn cfg(send_content: bool) -> Config {
        let mut c = Config::default();
        c.ai.enabled = true;
        c.ai.send_content = send_content;
        c.ai.profiles.insert("default".into(), AiProfile::default());
        c.search.min_similarity = 0.2;
        c
    }

    fn mock() -> Arc<MockEmbedder> {
        Arc::new(MockEmbedder::new(256).with_synonyms(&[
            &["청구서", "invoice", "요금", "고지서"],
            &["여행", "trip", "travel", "항공권"],
        ]))
    }

    fn corpus(s: &Store) {
        add(
            s,
            "KT_청구서_2026-09.pdf",
            "9월 인터넷 이용 요금 33,000원",
            100,
        );
        add(s, "오사카_항공권.pdf", "인천 → 간사이 왕복 e-ticket", 200);
        add(s, "회의록.docx", "분기 실적 보고와 다음 분기 목표", 300);
        add(s, "invoice_march.xlsx", "Monthly invoice for hosting", 400);
    }

    fn names(o: &SearchOutcome) -> Vec<&str> {
        o.hits.iter().map(|h| h.file.name.as_str()).collect()
    }

    #[test]
    fn embeds_once_reuses_and_finds_by_meaning() {
        let s = Store::open_in_memory().unwrap();
        corpus(&s);
        let m = mock();
        let sem = SemanticSearch::with_provider(m.clone(), &cfg(true)).unwrap();
        let r = sem.embed_pending(&s, 1000, |_| {}).unwrap();
        assert_eq!((r.embedded, r.reused, r.remaining), (4, 0, 0));
        assert!(r.error.is_none());
        // Nothing left: no further calls.
        let calls = m.calls();
        let r = sem.embed_pending(&s, 1000, |_| {}).unwrap();
        assert_eq!((r.embedded, r.reused), (0, 0));
        assert_eq!(m.calls(), calls);

        // "고지서" appears in no file, but means the same as 청구서/invoice.
        let q = Query::parse("통신 고지서 어디 있지?");
        let fts = hybrid_search(&s, None, &q, &SearchOptions::default()).unwrap();
        assert!(fts.hits.is_empty());
        assert_eq!(fts.mode, SearchMode::Fts);
        let o = hybrid_search(&s, Some(&sem), &q, &SearchOptions::default()).unwrap();
        assert_eq!(o.mode, SearchMode::Hybrid);
        assert!(o.natural);
        let n = names(&o);
        assert!(n.len() >= 2, "{n:?}");
        assert!(n[..2].contains(&"KT_청구서_2026-09.pdf"), "{n:?}");
        assert!(n[..2].contains(&"invoice_march.xlsx"), "{n:?}");
        assert!(
            !n.contains(&"회의록.docx"),
            "{:?}",
            o.hits
                .iter()
                .map(|h| (&h.file.name, h.similarity))
                .collect::<Vec<_>>()
        );
        assert_eq!(o.hits[0].matched_in, MatchedIn::Semantic);
        assert!(o.hits[0].similarity.unwrap() > 0.2);

        // Keyword query: the FTS hit stays first, meaning adds related files.
        let o = hybrid_search(
            &s,
            Some(&sem),
            &Query::parse("항공권"),
            &SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(names(&o)[0], "오사카_항공권.pdf");
        assert_eq!(o.hits[0].matched_in, MatchedIn::Name);

        // Filters apply to vector hits too.
        let o = hybrid_search(
            &s,
            Some(&sem),
            &Query::parse("요금 고지서 찾아줘 ext:xlsx"),
            &SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(names(&o), ["invoice_march.xlsx"]);

        // A moved file (same name and text, new row) reuses its vector.
        s.remove_file(&root().join("오사카_항공권.pdf")).unwrap();
        let calls = m.texts_embedded();
        add(&s, "오사카_항공권.pdf", "인천 → 간사이 왕복 e-ticket", 200);
        let r = sem.embed_pending(&s, 1000, |_| {}).unwrap();
        assert_eq!((r.embedded, r.reused), (0, 1));
        assert_eq!(m.texts_embedded(), calls);
        // …and the cache picks the new mapping up incrementally.
        let o = hybrid_search(
            &s,
            Some(&sem),
            &Query::parse("travel 계획"),
            &SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(names(&o)[0], "오사카_항공권.pdf");
    }

    #[test]
    fn test_connection_reports_vector_size_and_errors() {
        let s = Store::open_in_memory().unwrap();
        let m = mock();
        let sem = SemanticSearch::with_provider(m.clone(), &cfg(true)).unwrap();
        assert!(sem.test_connection(&s).unwrap() > 0);
        m.set_failing(true);
        assert!(sem.test_connection(&s).is_err());
    }

    #[test]
    fn provider_failure_falls_back_to_fts_and_marks_health() {
        let s = Store::open_in_memory().unwrap();
        corpus(&s);
        let m = mock();
        let mut c = cfg(true);
        c.ai.unavailable_after = 2;
        let sem = SemanticSearch::with_provider(m.clone(), &c).unwrap();
        m.set_failing(true);
        let r = sem.embed_pending(&s, 1000, |_| {}).unwrap();
        assert_eq!(r.embedded, 0);
        assert_eq!(r.remaining, 4);
        assert!(r.error.is_some());
        // Search still answers with words only and says why.
        let o = hybrid_search(
            &s,
            Some(&sem),
            &Query::parse("요금"),
            &SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(o.mode, SearchMode::Fts);
        assert_eq!(names(&o), ["KT_청구서_2026-09.pdf"]);
        assert!(o.note.is_some());
        // Two failures in a row → unavailable; further queries do not
        // even call the provider until a probe is due.
        assert!(!sem.health().is_available());
        let calls = m.calls();
        let o = hybrid_search(
            &s,
            Some(&sem),
            &Query::parse("다른 질의"),
            &SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(o.mode, SearchMode::Fts);
        assert_eq!(m.calls(), calls);
        let failed = s
            .recent_ai_log(10)
            .unwrap()
            .iter()
            .filter(|e| !e.ok)
            .count();
        assert_eq!(failed, 2);
    }

    #[test]
    fn send_content_false_embeds_names_only_under_its_own_key() {
        let s = Store::open_in_memory().unwrap();
        add(&s, "a.txt", "secret body", 1);
        let sem_full = SemanticSearch::with_provider(mock(), &cfg(true)).unwrap();
        let sem_name = SemanticSearch::with_provider(mock(), &cfg(false)).unwrap();
        assert_ne!(sem_full.model_key(), sem_name.model_key());
        assert!(sem_name.model_key().ends_with("#name"));
        assert_eq!(s.embed_backlog(sem_name.model_key()).unwrap(), 1);
        let c = s.embed_candidates(sem_name.model_key(), 0, 10).unwrap();
        assert_eq!(embed_text(&c[0].name, c[0].text.as_deref()), "a txt");
    }

    #[test]
    fn natural_language_detection() {
        let t = |s: &str| is_natural_language(&Query::parse(s).terms);
        assert!(t("작년에 받은 KT 청구서"));
        assert!(t("invoice?"));
        assert!(t("여행 관련 서류"));
        assert!(!t("보고서"));
        assert!(!t("KT 청구서"));
    }

    #[test]
    fn int8_cache_ranks_like_exact_cosine() {
        let mut c = VectorCache::empty("k");
        let vs = [
            normalized(vec![1.0, 0.0, 0.0, 0.1]),
            normalized(vec![0.7, 0.7, 0.0, 0.0]),
            normalized(vec![0.0, 0.0, 1.0, 0.0]),
        ];
        for (i, v) in vs.iter().enumerate() {
            c.upsert(i as i64 + 1, v);
        }
        let q = normalized(vec![1.0, 0.2, 0.0, 0.0]);
        let got = c.nearest(&q, None, 2, 0.0);
        assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), vec![1, 2]);
        let exact: f32 = vs[0].iter().zip(&q).map(|(a, b)| a * b).sum();
        assert!((got[0].1 - exact).abs() < 0.01);
        let only3: HashSet<i64> = [3].into();
        assert_eq!(c.nearest(&q, Some(&only3), 5, -1.0)[0].0, 3);
        // Replacing a vector keeps one slot.
        c.upsert(1, &vs[2]);
        assert_eq!(c.ids.len(), 3);
        assert_eq!(c.nearest(&q, None, 1, 0.0)[0].0, 2);
    }

    /// `cargo test -p tidy-core --release bench_100k -- --ignored --nocapture`
    /// (numbers recorded in SPEC §13).
    #[test]
    #[ignore]
    fn bench_100k() {
        let dim = 768;
        let n = 100_000;
        let mut c = VectorCache::empty("k");
        let mut seed = 1u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 2000) as f32 / 1000.0 - 1.0
        };
        let t = Instant::now();
        for i in 0..n {
            let v: Vec<f32> = (0..dim).map(|_| rnd()).collect();
            c.upsert(i, &normalized(v));
        }
        println!(
            "load {n}×{dim}: {:?}, {} MB",
            t.elapsed(),
            c.data.len() / (1 << 20)
        );
        let q = normalized((0..dim).map(|_| rnd()).collect());
        for _ in 0..3 {
            let t = Instant::now();
            let r = c.nearest(&q, None, 50, -1.0);
            println!("query: {:?} ({} hits)", t.elapsed(), r.len());
        }
    }
}
