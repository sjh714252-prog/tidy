//! AI step of the classifier (SPEC §5.2): Rules → (no match) AI → (no AI
//! or low confidence) ask the user.
//!
//! Builds the minimal context (file facts, optional text excerpt, candidate
//! folder tree, recent folders, similar past decisions), asks the provider
//! for one JSON answer, validates it, and logs the call in `ai_log`
//! (tokens, cost, latency — never keys or file contents).
//!
//! Never blocking: one suggestion is bounded by `ai.timeout_secs` (retries
//! included) and every failure is returned as an error the engine turns
//! into "no suggestion".

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::{FileFacts, RuleSet};
use crate::ai::{
    build_provider, estimate_cost_usd, prompts, AiError, AiHealth, LlmProvider, LlmResponse, Usage,
};
use crate::config::{expand_home, Config};
use crate::index::Extractors;
use crate::paths::normalize_lexical;
use crate::store::{AiLogEntry, Store};

/// One JSON request through [`AiClassifier::call`].
#[derive(Debug, Clone, Copy)]
pub struct AiCall<'a> {
    /// `ai_log.purpose`: classify | test | restructure.
    pub purpose: &'static str,
    pub system: &'a str,
    pub user: &'a str,
    pub schema: &'a serde_json::Value,
    pub max_tokens: u32,
    pub timeout: Duration,
}

/// Validated AI answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AiSuggestion {
    pub dest_dir: PathBuf,
    /// 0.0 ..= 1.0
    pub confidence: f64,
    pub reason: String,
    /// The folder does not exist yet (always asks the user).
    pub new_folder: bool,
    /// New file name (with extension), if the model proposed one.
    pub suggested_rename: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAnswer {
    dest: String,
    confidence: f64,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    new_folder: bool,
    #[serde(default)]
    suggested_rename: Option<String>,
}

/// Settings copied out of `Config` / `RuleSet` when the classifier is built.
#[derive(Debug, Clone)]
struct Settings {
    auto_threshold: f64,
    send_content: bool,
    content_chars: usize,
    max_content_bytes: u64,
    timeout: Duration,
    tree_depth: usize,
    history_k: usize,
    tree_roots: Vec<PathBuf>,
    ai_auto_roots: Vec<PathBuf>,
}

pub struct AiClassifier {
    provider: Arc<dyn LlmProvider>,
    settings: Settings,
    health: AiHealth,
    extractors: Extractors,
    /// (M16) Describes a new picture before asking where it belongs.
    vision: Option<crate::index::vision::VisionDescriber>,
    vision_max_bytes: u64,
    /// (M16) Pictures showing personal information are not sent: their
    /// text is read locally first (M15 engine, built on first use).
    ocr_cfg: Option<crate::config::OcrCfg>,
    ocr_engine: std::sync::OnceLock<Option<Box<dyn crate::index::ocr::OcrEngine>>>,
    pii: Option<crate::pii::Detector>,
    rt: tokio::runtime::Runtime,
}

impl std::fmt::Debug for AiClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiClassifier")
            .field("provider", &self.provider.name())
            .field("model", &self.provider.model())
            .finish()
    }
}

/// Room for (adaptive) thinking plus a short JSON answer.
const CLASSIFY_MAX_TOKENS: u32 = 2048;

/// Lines of the candidate tree are capped so the prompt stays small.
const MAX_TREE_LINES: usize = 150;

impl AiClassifier {
    /// The classifier for `cfg.ai`, or why there is none:
    /// [`AiError::Disabled`], [`AiError::NoProfile`], [`AiError::MissingKey`]…
    pub fn from_config(cfg: &Config, rules: &RuleSet) -> Result<Self, AiError> {
        if !cfg.ai.enabled {
            return Err(AiError::Disabled);
        }
        let pc = cfg
            .ai
            .active()
            .and_then(|p| p.classify.as_ref())
            .ok_or_else(|| AiError::NoProfile(cfg.ai.active_profile.clone()))?;
        let timeout = Duration::from_secs(cfg.ai.timeout_secs.max(1));
        let provider = build_provider(pc, timeout, cfg.ai.max_retries)?;
        let mut ai = Self::with_provider(provider, cfg, rules)?;
        if cfg.vision.enabled && cfg.vision.classify {
            match crate::index::vision::VisionDescriber::from_config(cfg) {
                Ok(v) => ai = ai.with_vision(v),
                Err(e) => tracing::info!("pictures are classified by name only: {e}"),
            }
        }
        Ok(ai)
    }

    /// (M16) Describe pictures with `v` before classifying them.
    pub fn with_vision(mut self, v: crate::index::vision::VisionDescriber) -> Self {
        self.vision = Some(v);
        self
    }

    /// Use an explicit provider (tests use [`crate::ai::MockProvider`]).
    pub fn with_provider(
        provider: Arc<dyn LlmProvider>,
        cfg: &Config,
        rules: &RuleSet,
    ) -> Result<Self, AiError> {
        let anchors: Vec<PathBuf> = cfg
            .watches
            .iter()
            .map(|w| w.anchor().to_path_buf())
            .collect();
        let mut tree_roots = rules.dest_roots_for(&anchors);
        for p in cfg
            .index_only
            .iter()
            .map(|r| r.path.clone())
            .chain(cfg.ai.candidate_roots.iter().cloned())
        {
            let p = normalize_lexical(&p);
            if !tree_roots.contains(&p) {
                tree_roots.push(p);
            }
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| AiError::Runtime(e.to_string()))?;
        Ok(Self {
            provider,
            settings: Settings {
                auto_threshold: cfg.ai.auto_threshold,
                send_content: cfg.ai.send_content(),
                content_chars: cfg.ai.content_chars,
                max_content_bytes: cfg.index.max_content_size_mb * 1024 * 1024,
                timeout: Duration::from_secs(cfg.ai.timeout_secs.max(1)),
                tree_depth: cfg.ai.tree_depth,
                history_k: cfg.ai.history_k,
                tree_roots,
                ai_auto_roots: rules.ai_auto_roots_for(&anchors),
            },
            extractors: Extractors::default_set(),
            vision: None,
            vision_max_bytes: cfg.vision.max_image_mb * 1024 * 1024,
            ocr_cfg: cfg.ocr.enabled.then(|| cfg.ocr.clone()),
            ocr_engine: std::sync::OnceLock::new(),
            pii: cfg
                .security
                .pii
                .enabled
                .then(|| crate::pii::Detector::lenient(&cfg.security.pii)),
            health: AiHealth::new(cfg.ai.unavailable_after),
            rt,
        })
    }

    /// Confidence needed to move without asking (`ai.auto_threshold`).
    pub fn auto_threshold(&self) -> f64 {
        self.settings.auto_threshold
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn model(&self) -> &str {
        self.provider.model()
    }

    /// Consecutive-failure state (status bar "AI 사용 불가").
    pub fn health(&self) -> &AiHealth {
        &self.health
    }

    /// Ask the model where `facts` belongs. Logged in `ai_log` either way.
    /// Must not be called from inside an async runtime (it blocks on its own
    /// small runtime); the watch loop and `spawn_blocking` threads are fine.
    /// While the AI is unavailable (repeated failures) this fails at once
    /// without a request, except when a retry is due.
    pub fn suggest(&self, store: &Store, facts: &FileFacts) -> Result<AiSuggestion, AiError> {
        if !self.health.is_available() && !self.health.probe_due() {
            return Err(AiError::Paused(
                self.health.last_error().unwrap_or_default(),
            ));
        }
        let input = self.build_input(store, facts);
        let user = prompts::classify_user(&input);
        let call = AiCall {
            purpose: "classify",
            system: prompts::CLASSIFY_SYSTEM,
            user: &user,
            schema: &prompts::classify_schema(),
            max_tokens: CLASSIFY_MAX_TOKENS,
            timeout: self.settings.timeout,
        };
        self.call(store, &call, |v| {
            let s = validate(v)?;
            let summary = format!("{} ({:.0}%)", s.dest_dir.display(), s.confidence * 100.0);
            Ok((s, Some(summary)))
        })
    }

    /// (M9) Ask whether a regex hit is really a personal-information file
    /// (`security.pii.ai_check`). Sends the file name, what was found and
    /// — only with `send_content` — an excerpt with every digit and mail
    /// user name masked. Logged with purpose "pii".
    pub fn confirm_pii(
        &self,
        store: &Store,
        facts: &FileFacts,
        finding: &crate::pii::PiiFinding,
    ) -> Result<bool, AiError> {
        if !self.health.is_available() && !self.health.probe_due() {
            return Err(AiError::Paused(
                self.health.last_error().unwrap_or_default(),
            ));
        }
        let excerpt = (self.settings.send_content
            && self.settings.content_chars > 0
            && facts.size <= self.settings.max_content_bytes)
            .then(|| self.extractors.extract(&facts.path))
            .flatten()
            .and_then(Result::ok)
            .map(|t| {
                prompts::mask_pii(
                    &t.chars()
                        .take(self.settings.content_chars)
                        .collect::<String>(),
                )
            });
        let user = prompts::pii_user(&facts.name, &finding.summary(), excerpt.as_deref());
        let call = AiCall {
            purpose: "pii",
            system: prompts::PII_SYSTEM,
            user: &user,
            schema: &prompts::pii_schema(),
            max_tokens: 512,
            timeout: self.settings.timeout,
        };
        self.call(store, &call, |v| {
            let personal = v
                .get("personal")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| AiError::BadResponse("missing \"personal\"".into()))?;
            Ok((
                personal,
                Some(
                    if personal {
                        "개인정보"
                    } else {
                        "개인정보 아님"
                    }
                    .into(),
                ),
            ))
        })
    }

    /// One tiny request to check model, key and network ("연결 테스트").
    /// Logged in `ai_log` with purpose "test". Returns the answering model.
    pub fn test_connection(&self, store: &Store) -> Result<String, AiError> {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"],
            "additionalProperties": false
        });
        let call = AiCall {
            purpose: "test",
            system: "Connectivity check. Answer with the JSON object {\"ok\": true}.",
            user: "ping",
            schema: &schema,
            max_tokens: 256,
            timeout: self.settings.timeout,
        };
        let model = self.provider.model().to_string();
        self.call(store, &call, |_| Ok((model.clone(), None)))
    }

    /// Any JSON request on this provider (used by the restructure report):
    /// bounded by `call.timeout`, logged in `ai_log` under `call.purpose`.
    /// `check` validates the answer and may return a short result summary
    /// for the log (never file contents).
    pub fn call<T>(
        &self,
        store: &Store,
        call: &AiCall<'_>,
        check: impl FnOnce(serde_json::Value) -> Result<(T, Option<String>), AiError>,
    ) -> Result<T, AiError> {
        let started = Instant::now();
        let fut = self
            .provider
            .complete_json(call.system, call.user, call.schema, call.max_tokens);
        let res: Result<LlmResponse, AiError> = self
            .rt
            .block_on(async { tokio::time::timeout(call.timeout, fut).await })
            .unwrap_or(Err(AiError::Timeout(call.timeout.as_secs())));
        let (usage, model) = match &res {
            Ok(r) => (r.usage, r.model.clone()),
            Err(_) => (Usage::default(), self.provider.model().to_string()),
        };
        // Only transport-level results count for availability: a refusal or
        // an answer that fails validation still means the AI is reachable.
        match &res {
            Ok(_) | Err(AiError::Refusal | AiError::Truncated) => self.health.record_success(),
            Err(e) => self.health.record_failure(&e.to_string()),
        }
        let outcome = res.and_then(|r| check(r.value));
        let log = AiLogEntry {
            provider: self.provider.name().to_string(),
            model: model.clone(),
            purpose: call.purpose.to_string(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: estimate_cost_usd(self.provider.name(), &model, usage),
            latency_ms: Some(started.elapsed().as_millis() as u64),
            ok: outcome.is_ok(),
            error: outcome.as_ref().err().map(|e| e.to_string()),
            result: outcome.as_ref().ok().and_then(|(_, s)| s.clone()),
            ..Default::default()
        };
        if let Err(e) = store.insert_ai_log(&log) {
            tracing::warn!("ai_log write failed: {e}");
        }
        outcome.map(|(t, _)| t)
    }

    /// May this suggestion move the file without asking? (SPEC §5.2:
    /// confidence ≥ threshold, an existing folder, inside an `ai_auto` rule
    /// folder.)
    pub fn auto_allowed(&self, s: &AiSuggestion) -> bool {
        s.confidence >= self.settings.auto_threshold
            && !s.new_folder
            && s.dest_dir.is_dir()
            && self
                .settings
                .ai_auto_roots
                .iter()
                .any(|root| s.dest_dir.starts_with(root))
    }

    fn build_input(&self, store: &Store, facts: &FileFacts) -> prompts::ClassifyInput {
        let src_dir = facts.path.parent().unwrap_or_else(|| Path::new(""));
        let modified: DateTime<Local> = facts.modified.into();
        let excerpt = (self.settings.send_content
            && self.settings.content_chars > 0
            && facts.size <= self.settings.max_content_bytes)
            .then(|| self.extractors.extract(&facts.path))
            .flatten()
            .and_then(Result::ok)
            .map(|t| t.chars().take(self.settings.content_chars).collect())
            .or_else(|| self.picture_excerpt(store, facts));
        let recent_dirs = store
            .recent_dest_dirs(10)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.display().to_string())
            .collect();
        let ext = (!facts.ext.is_empty()).then_some(facts.ext.as_str());
        let history = store
            .similar_decisions(ext, src_dir, self.settings.history_k)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|d| Some((d.file_name, d.chosen_dest?.display().to_string())))
            .collect();
        prompts::ClassifyInput {
            file_name: facts.name.clone(),
            ext: facts.ext.clone(),
            size: facts.size,
            modified: modified.format("%Y-%m-%d %H:%M").to_string(),
            source_dir: src_dir.display().to_string(),
            excerpt,
            tree: folder_tree(&self.settings.tree_roots, self.settings.tree_depth),
            recent_dirs,
            history,
        }
    }
}

impl AiClassifier {
    /// (M16) "[사진 설명] …" for a picture when a vision model is set up
    /// (cached by content hash, so indexing the moved file sends nothing
    /// again). `None` on any failure: the picture is classified by name.
    fn picture_excerpt(&self, store: &Store, facts: &FileFacts) -> Option<String> {
        let v = self.vision.as_ref()?;
        if !self.settings.send_content
            || !crate::index::vision::is_target(&facts.ext, facts.size, self.vision_max_bytes)
        {
            return None;
        }
        let hash = crate::mover::hash_file(&facts.path).ok();
        if self.shows_personal_info(store, facts, hash.as_deref()) {
            tracing::info!(path = %facts.path.display(), "picture shows personal information; not sent");
            return None;
        }
        match v.describe_cached(store, &facts.path, &facts.name, hash.as_deref()) {
            Ok(c) if !c.is_empty() => Some(format!("{}{}", crate::store::CAPTION_MARK, c.line())),
            Ok(_) => None,
            Err(e) => {
                tracing::info!(path = %facts.path.display(), "picture not described: {e}");
                None
            }
        }
    }

    /// Read the picture's text locally (cached by hash, like the indexer
    /// does) and judge it with the M9 detector. No engine or no detector:
    /// only the file name is judged.
    fn shows_personal_info(&self, store: &Store, facts: &FileFacts, hash: Option<&str>) -> bool {
        let Some(det) = &self.pii else {
            return false;
        };
        let cached = hash.and_then(|h| store.ocr_cache_get(h).ok().flatten());
        let text = cached.or_else(|| {
            let cfg = self.ocr_cfg.as_ref()?;
            let engine = self
                .ocr_engine
                .get_or_init(|| crate::index::ocr::build_engine(cfg).ok())
                .as_deref()?;
            let t = crate::index::ocr::recognize(engine, &facts.path, cfg).ok()?;
            if let Some(h) = hash {
                let _ = store.ocr_cache_put(h, &t, engine.name());
            }
            Some(t)
        });
        det.judge(&facts.name, text.as_deref()).flags != 0
    }
}

/// Check the model's answer: an absolute destination (after `~`), a
/// confidence clamped to 0..1, and a plain file name for renames.
fn validate(v: serde_json::Value) -> Result<AiSuggestion, AiError> {
    let raw: RawAnswer = serde_json::from_value(v)
        .map_err(|e| AiError::BadResponse(format!("answer does not match the schema: {e}")))?;
    let dest = raw.dest.trim();
    if dest.is_empty() {
        return Err(AiError::BadResponse("empty dest".into()));
    }
    let dest_dir = normalize_lexical(&expand_home(Path::new(dest)));
    if !dest_dir.is_absolute() {
        return Err(AiError::BadResponse(format!(
            "dest is not an absolute folder: {dest}"
        )));
    }
    let confidence = if raw.confidence.is_finite() {
        raw.confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let suggested_rename = raw
        .suggested_rename
        .map(|n| n.trim().to_string())
        .filter(|n| is_plain_file_name(n));
    Ok(AiSuggestion {
        // A folder that does not exist is a new folder whatever the model said.
        new_folder: raw.new_folder || !dest_dir.is_dir(),
        dest_dir,
        confidence,
        reason: raw.reason.trim().chars().take(300).collect(),
        suggested_rename,
    })
}

/// A single file name: no separators, not `.`/`..`, not empty.
pub fn is_plain_file_name(n: &str) -> bool {
    !n.is_empty()
        && n != "."
        && n != ".."
        && !n.contains(['/', '\\', ':', '\0'])
        && n.chars().count() <= 200
}

/// Indented folder listing (folders only, hidden ones skipped), depth
/// limited, at most [`MAX_TREE_LINES`] lines.
fn folder_tree(roots: &[PathBuf], depth: usize) -> Vec<String> {
    let mut out = Vec::new();
    for root in roots {
        if out.len() >= MAX_TREE_LINES || !root.is_dir() {
            continue;
        }
        out.push(root.display().to_string());
        let walker = walkdir::WalkDir::new(root)
            .min_depth(1)
            .max_depth(depth.max(1))
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'));
        for e in walker.filter_map(Result::ok) {
            if out.len() >= MAX_TREE_LINES {
                out.push("  …".into());
                break;
            }
            if e.file_type().is_dir() {
                out.push(format!(
                    "{}{}",
                    "  ".repeat(e.depth()),
                    e.file_name().to_string_lossy()
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::MockProvider;
    use serde_json::json;
    use std::fs;

    fn toml_path(p: &Path) -> String {
        p.display().to_string().replace('\\', "/")
    }

    fn setup(
        answer: serde_json::Value,
    ) -> (tempfile::TempDir, AiClassifier, Arc<MockProvider>, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("문서 보관");
        fs::create_dir_all(docs.join("업무").join("KT")).unwrap();
        fs::create_dir_all(docs.join(".숨김")).unwrap();
        let rules = RuleSet::from_toml(&format!(
            "[[rule]]\nname='문서'\nmatch={{ext=['pdf']}}\ndest='{}/{{yyyy}}'\nai_auto=true\n",
            toml_path(&docs)
        ))
        .unwrap();
        let mock = Arc::new(MockProvider::new(answer));
        let mut cfg = Config::default();
        cfg.ai.enabled = true;
        let ai = AiClassifier::with_provider(mock.clone(), &cfg, &rules).unwrap();
        (tmp, ai, mock, Store::open_in_memory().unwrap())
    }

    fn facts(dir: &Path, name: &str, body: &str) -> FileFacts {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        FileFacts::from_path(&p).unwrap()
    }

    #[test]
    fn prompt_has_tree_excerpt_and_logs_the_call() {
        let (tmp, ai, mock, store) = setup(json!({
            "dest": "~/x", "confidence": 0.5, "reason": "r", "new_folder": false, "suggested_rename": null
        }));
        let f = facts(tmp.path(), "KT 과금 메모.txt", "9월 인터넷 회선 과금 내역");
        let s = ai.suggest(&store, &f).unwrap();
        assert_eq!(s.confidence, 0.5);
        let (sys, user) = mock.prompts().pop().unwrap();
        assert_eq!(sys, prompts::CLASSIFY_SYSTEM);
        assert!(user.contains("KT 과금 메모.txt"));
        assert!(user.contains("9월 인터넷 회선 과금 내역"));
        assert!(user.contains("  업무"), "{user}");
        assert!(user.contains("    KT"), "{user}");
        assert!(!user.contains(".숨김"));
        let log = store.recent_ai_log(1).unwrap();
        assert_eq!(log[0].provider, "mock");
        assert!(log[0].ok);
        assert_eq!(log[0].input_tokens, Some(100));
        // Contents never go to the log.
        assert!(!log[0]
            .result
            .clone()
            .unwrap_or_default()
            .contains("과금 내역"));
    }

    #[test]
    fn send_content_false_keeps_text_local() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProvider::new(json!({
            "dest": "~/x", "confidence": 0.5, "reason": "", "new_folder": false, "suggested_rename": null
        })));
        let mut cfg = Config::default();
        cfg.ai.enabled = true;
        cfg.ai.send_content = false;
        let ai = AiClassifier::with_provider(mock.clone(), &cfg, &RuleSet::default()).unwrap();
        let f = facts(tmp.path(), "비밀.txt", "주민번호 900101-1234567");
        ai.suggest(&Store::open_in_memory().unwrap(), &f).unwrap();
        let (_, user) = mock.prompts().pop().unwrap();
        assert!(!user.contains("900101"));
    }

    #[test]
    fn auto_only_inside_ai_auto_folder_with_high_confidence() {
        let (tmp, ai, _mock, _store) = setup(json!({}));
        let kt = tmp.path().join("문서 보관").join("업무").join("KT");
        let mk = |dest: &Path, c: f64| AiSuggestion {
            dest_dir: dest.to_path_buf(),
            confidence: c,
            reason: String::new(),
            new_folder: !dest.is_dir(),
            suggested_rename: None,
        };
        assert!(ai.auto_allowed(&mk(&kt, 0.95)));
        assert!(!ai.auto_allowed(&mk(&kt, 0.5)));
        assert!(!ai.auto_allowed(&mk(&kt.join("새 폴더"), 0.99)));
        assert!(!ai.auto_allowed(&mk(tmp.path(), 0.99)));
    }

    #[test]
    fn validation() {
        let ok = validate(json!({
            "dest": "~/문서", "confidence": 1.7, "reason": "  r  ", "new_folder": false,
            "suggested_rename": "a/b.txt"
        }))
        .unwrap();
        assert_eq!(ok.dest_dir, dirs::home_dir().unwrap().join("문서"));
        assert_eq!(ok.confidence, 1.0);
        assert_eq!(ok.reason, "r");
        assert_eq!(ok.suggested_rename, None);
        assert!(validate(json!({"dest": "상대/경로", "confidence": 0.9})).is_err());
        assert!(validate(json!({"dest": "", "confidence": 0.9})).is_err());
        assert!(validate(json!({"confidence": 0.9})).is_err());
        assert!(is_plain_file_name("2026-09 KT 과금.xlsx"));
        assert!(!is_plain_file_name(".."));
    }

    #[test]
    fn provider_failure_is_logged_and_returned() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProvider::sequence(vec![Err(
            "connection refused".into()
        )]));
        let mut cfg = Config::default();
        cfg.ai.enabled = true;
        let ai = AiClassifier::with_provider(mock, &cfg, &RuleSet::default()).unwrap();
        let store = Store::open_in_memory().unwrap();
        let f = facts(tmp.path(), "a.txt", "x");
        assert!(ai.suggest(&store, &f).is_err());
        let log = store.recent_ai_log(1).unwrap();
        assert!(!log[0].ok);
        assert!(log[0]
            .error
            .as_deref()
            .unwrap()
            .contains("connection refused"));
    }

    #[test]
    fn repeated_failures_pause_the_ai_until_it_answers_again() {
        use crate::ai::AiAvailability;
        let tmp = tempfile::tempdir().unwrap();
        let ok = json!({"dest": tmp.path(), "confidence": 0.5, "reason": "r"});
        let mock = Arc::new(MockProvider::sequence(vec![
            Err("down".into()),
            Err("down".into()),
            Err("down".into()),
            Ok(ok),
        ]));
        let mut cfg = Config::default();
        cfg.ai.enabled = true;
        cfg.ai.unavailable_after = 3;
        let ai = AiClassifier::with_provider(mock.clone(), &cfg, &RuleSet::default()).unwrap();
        let store = Store::open_in_memory().unwrap();
        let f = facts(tmp.path(), "a.txt", "x");
        for _ in 0..3 {
            assert!(ai.suggest(&store, &f).is_err());
        }
        assert_eq!(ai.health().take_change(), Some(AiAvailability::Unavailable));
        // Paused: no request until a probe is due.
        let e = ai.suggest(&store, &f).unwrap_err();
        assert!(matches!(e, AiError::Paused(_)), "{e}");
        assert_eq!(mock.prompts().len(), 3);
        // An explicit probe (connection test) brings it back.
        ai.test_connection(&store).unwrap();
        assert_eq!(ai.health().take_change(), Some(AiAvailability::Ready));
        assert!(ai.suggest(&store, &f).is_ok());
    }

    #[test]
    fn disabled_or_unconfigured() {
        let mut cfg = Config::default();
        assert!(matches!(
            AiClassifier::from_config(&cfg, &RuleSet::default()),
            Err(AiError::Disabled)
        ));
        cfg.ai.enabled = true;
        assert!(matches!(
            AiClassifier::from_config(&cfg, &RuleSet::default()),
            Err(AiError::NoProfile(_))
        ));
    }
}
