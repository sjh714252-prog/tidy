//! (M16) Picture descriptions by a vision model (SPEC §14.13).
//!
//! The indexer only marks pictures (`files.vision = pending`, or fills
//! them from `vision_cache` by content hash). [`run_pass`] sends waiting
//! pictures — shrunk, newest first, at most `[vision] daily_limit` a day —
//! to the vision model of the active AI profile and appends the answer to
//! the file's body, so word search, semantic search and the AI prompts
//! (classify, curate) see what the picture shows. The classifier also
//! describes a new picture before asking where it belongs
//! ([`VisionDescriber::describe_cached`]).
//!
//! Opt-in (`[vision] enabled`, off by default) because the pictures leave
//! the PC unless the model is local; never with `ai.send_content = false`;
//! never for pictures judged personal information (M9) or vault files.
//! Every failure leaves the file waiting or marks it failed — never an
//! error for the caller (CLAUDE.md rule 4). Nothing here writes to user
//! folders.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;

use crate::ai::{
    build_provider, estimate_cost_usd, prompts, AiError, AiHealth, ImageInput, LlmProvider,
    LlmResponse, Usage,
};
use crate::config::Config;
use crate::store::{AiLogEntry, Caption, Store, StoreError, VisionCounts, VisionState};

/// Picture types sent (the same ones text recognition reads).
pub use super::ocr::IMAGE_EXTS;
/// Smaller pictures are icons and the like.
pub const MIN_IMAGE_BYTES: u64 = 10 * 1024;
/// Pictures described per pass of the background loop.
pub const VISION_PASS: usize = 8;
/// Room for a short JSON answer (and thinking where the model thinks).
const VISION_MAX_TOKENS: u32 = 1024;
/// JPEG quality of shrunk pictures.
const JPEG_QUALITY: u8 = 85;
/// A JPEG/PNG this small that already fits is sent as it is.
const PASSTHROUGH_BYTES: usize = 3 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum VisionError {
    /// The model is not reachable now (network, key, paused…): the file
    /// keeps waiting and the pass stops.
    #[error("{0}")]
    Unavailable(String),
    /// This picture could not be read or described: marked failed.
    #[error("{0}")]
    File(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl VisionError {
    fn from_ai(e: AiError) -> Self {
        match e {
            AiError::Refusal
            | AiError::BadResponse(_)
            | AiError::Truncated
            | AiError::Http {
                status: 400 | 413 | 415 | 422,
                ..
            } => VisionError::File(e.to_string()),
            other => VisionError::Unavailable(other.to_string()),
        }
    }
}

/// Should the indexer mark this file for a description?
pub fn is_target(ext: &str, size: u64, max_image_bytes: u64) -> bool {
    IMAGE_EXTS.contains(&ext) && (MIN_IMAGE_BYTES..=max_image_bytes).contains(&size)
}

/// Read a picture, turn it upright (EXIF), shrink it so the long side is
/// at most `max_side` and encode it as JPEG (transparent parts on white).
/// Small upright JPEG/PNG files that already fit are sent unchanged.
pub fn prepare_image(path: &Path, max_side: u32) -> Result<ImageInput, VisionError> {
    use base64::Engine as _;
    use image::{ImageDecoder, ImageFormat, ImageReader};

    let bytes = std::fs::read(path).map_err(|e| VisionError::File(e.to_string()))?;
    let bad = |e: image::ImageError| VisionError::File(format!("그림을 읽을 수 없음: {e}"));
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| VisionError::File(e.to_string()))?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(bad)?;
    let orientation = decoder.orientation().map_err(bad)?;
    let (w, h) = decoder.dimensions();
    let upright = orientation == image::metadata::Orientation::NoTransforms;
    let fits = w.max(h) <= max_side.max(64);
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    if upright && fits && bytes.len() <= PASSTHROUGH_BYTES {
        match format {
            Some(ImageFormat::Jpeg) => {
                return Ok(ImageInput {
                    media_type: "image/jpeg",
                    base64: b64(&bytes),
                })
            }
            Some(ImageFormat::Png) => {
                return Ok(ImageInput {
                    media_type: "image/png",
                    base64: b64(&bytes),
                })
            }
            _ => {}
        }
    }
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(bad)?;
    img.apply_orientation(orientation);
    if !fits {
        let side = max_side.max(64);
        img = img.resize(side, side, image::imageops::FilterType::Triangle);
    }
    let rgb = if img.color().has_alpha() {
        // JPEG has no transparency: put it on white, not black.
        let rgba = img.to_rgba8();
        let mut out = image::RgbImage::new(rgba.width(), rgba.height());
        for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
            let a = u16::from(p[3]);
            for c in 0..3 {
                o[c] = ((u16::from(p[c]) * a + 255 * (255 - a)) / 255) as u8;
            }
        }
        out
    } else {
        img.to_rgb8()
    };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(bad)?;
    Ok(ImageInput {
        media_type: "image/jpeg",
        base64: b64(&out),
    })
}

/// The vision model of the active AI profile, with its own failure
/// counter (a down vision model does not pause file sorting).
pub struct VisionDescriber {
    provider: Arc<dyn LlmProvider>,
    health: AiHealth,
    timeout: Duration,
    max_side: u32,
    rt: tokio::runtime::Runtime,
}

impl std::fmt::Debug for VisionDescriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VisionDescriber")
            .field("provider", &self.provider.name())
            .field("model", &self.provider.model())
            .finish()
    }
}

/// Why pictures cannot be described with this config (`None` = they can,
/// as far as the settings go). For people.
pub fn config_blocker(cfg: &Config) -> Option<String> {
    if !cfg.vision.enabled {
        return Some("사진 내용 인식이 꺼져 있습니다".into());
    }
    if !cfg.ai.enabled {
        return Some("AI가 꺼져 있습니다 (설정 › AI)".into());
    }
    if !cfg.ai.send_content() {
        return Some(
            "AI 프로필이 본문을 보내지 않도록 되어 있어(send_content = false) 사진도 보내지 않습니다"
                .into(),
        );
    }
    if cfg.ai.active().and_then(|p| p.classify.as_ref()).is_none() {
        return Some("AI 분류 모델이 설정되지 않았습니다 (설정 › AI)".into());
    }
    None
}

impl VisionDescriber {
    /// From `[vision]` + the classify provider of the active AI profile
    /// (with `[vision] model` in place of its model when set).
    pub fn from_config(cfg: &Config) -> Result<Self, AiError> {
        if let Some(why) = config_blocker(cfg) {
            return Err(if !cfg.vision.enabled || !cfg.ai.enabled {
                AiError::Disabled
            } else {
                AiError::Unsupported(why)
            });
        }
        let mut pc = cfg
            .ai
            .active()
            .and_then(|p| p.classify.clone())
            .ok_or_else(|| AiError::NoProfile(cfg.ai.active_profile.clone()))?;
        if let Some(m) = cfg.vision.model.as_deref().map(str::trim) {
            if !m.is_empty() {
                pc.model = m.to_string();
            }
        }
        let timeout = Duration::from_secs(cfg.vision.timeout_secs.max(1));
        let provider = build_provider(&pc, timeout, cfg.ai.max_retries)?;
        Self::with_provider(provider, cfg)
    }

    /// Use an explicit provider (tests use [`crate::ai::MockProvider`]).
    pub fn with_provider(provider: Arc<dyn LlmProvider>, cfg: &Config) -> Result<Self, AiError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| AiError::Runtime(e.to_string()))?;
        Ok(Self {
            provider,
            health: AiHealth::new(cfg.ai.unavailable_after),
            timeout: Duration::from_secs(cfg.vision.timeout_secs.max(1)),
            max_side: cfg.vision.max_side,
            rt,
        })
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn model(&self) -> &str {
        self.provider.model()
    }

    pub fn health(&self) -> &AiHealth {
        &self.health
    }

    /// Send one picture. Logged in `ai_log` (purpose `vision`, the kind and
    /// number of tags only — never the description). Must not be called
    /// from inside an async runtime.
    pub fn describe(&self, store: &Store, path: &Path, name: &str) -> Result<Caption, VisionError> {
        if !self.health.is_available() && !self.health.probe_due() {
            return Err(VisionError::Unavailable(
                AiError::Paused(self.health.last_error().unwrap_or_default()).to_string(),
            ));
        }
        let image = prepare_image(path, self.max_side)?;
        let user = prompts::vision_user(name);
        let schema = prompts::vision_schema();
        let started = Instant::now();
        let fut = self.provider.complete_json_image(
            prompts::VISION_SYSTEM,
            &user,
            &image,
            &schema,
            VISION_MAX_TOKENS,
        );
        let res: Result<LlmResponse, AiError> = self
            .rt
            .block_on(async { tokio::time::timeout(self.timeout, fut).await })
            .unwrap_or(Err(AiError::Timeout(self.timeout.as_secs())));
        let (usage, model) = match &res {
            Ok(r) => (r.usage, r.model.clone()),
            Err(_) => (Usage::default(), self.provider.model().to_string()),
        };
        match &res {
            Ok(_) | Err(AiError::Refusal | AiError::Truncated) => self.health.record_success(),
            Err(e) => self.health.record_failure(&e.to_string()),
        }
        let outcome = res.and_then(|r| parse_caption(r.value));
        let log = AiLogEntry {
            provider: self.provider.name().to_string(),
            model: model.clone(),
            purpose: "vision".into(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: estimate_cost_usd(self.provider.name(), &model, usage),
            latency_ms: Some(started.elapsed().as_millis() as u64),
            ok: outcome.is_ok(),
            error: outcome.as_ref().err().map(|e| e.to_string()),
            result: outcome.as_ref().ok().map(|c| {
                format!(
                    "{} · 태그 {}",
                    c.kind.as_deref().unwrap_or("other"),
                    c.tags.len()
                )
            }),
            ..Default::default()
        };
        if let Err(e) = store.insert_ai_log(&log) {
            tracing::warn!("ai_log write failed: {e}");
        }
        outcome.map_err(VisionError::from_ai)
    }

    /// [`Self::describe`] unless the same content was described before
    /// (`hash`, blake3 as in `files.hash`); new answers go to the cache.
    pub fn describe_cached(
        &self,
        store: &Store,
        path: &Path,
        name: &str,
        hash: Option<&str>,
    ) -> Result<Caption, VisionError> {
        if let Some(h) = hash {
            if let Some(c) = store.vision_cache_get(h)? {
                return Ok(c);
            }
        }
        let c = self.describe(store, path, name)?;
        if let Some(h) = hash {
            store.vision_cache_put(h, &c, self.provider.model())?;
        }
        Ok(c)
    }
}

/// Check the model's answer: caption ≤ 300 characters, ≤ 12 non-empty
/// tags of ≤ 30 characters, a known kind.
fn parse_caption(v: serde_json::Value) -> Result<Caption, AiError> {
    let caption = v
        .get("caption")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AiError::BadResponse("missing \"caption\"".into()))?;
    let caption: String = caption
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect();
    let mut tags: Vec<String> = Vec::new();
    for t in v
        .get("tags")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
    {
        let t: String = t.trim().trim_start_matches('#').chars().take(30).collect();
        if !t.is_empty() && !tags.contains(&t) && tags.len() < 12 {
            tags.push(t);
        }
    }
    let kind = v
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .map(|k| {
            if prompts::VISION_KINDS.contains(&k) {
                k
            } else {
                "other"
            }
        })
        .map(str::to_string);
    Ok(Caption {
        caption,
        tags,
        kind,
    })
}

/// Settings card / `tidy vision --status`.
#[derive(Debug, Clone, Serialize)]
pub struct VisionStatus {
    pub enabled: bool,
    /// "anthropic · claude-haiku-4-5" (the model pictures go to).
    pub model: Option<String>,
    /// The pictures stay on this PC (Ollama).
    pub local: bool,
    /// Why nothing is sent now.
    pub note: Option<String>,
    pub counts: VisionCounts,
    /// Described today / allowed per day.
    pub today: u64,
    pub daily_limit: u32,
    /// All descriptions so far and their estimated cost (USD; unknown
    /// model prices count as 0).
    pub total: u64,
    pub cost_usd: f64,
}

/// Local midnight today (unix seconds), for the daily limit.
pub fn today_start() -> i64 {
    use chrono::{Local, TimeZone};
    let midnight = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight");
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or_else(|| Local::now().timestamp() - 86_400)
}

pub fn status(cfg: &Config, store: &Store) -> Result<VisionStatus, StoreError> {
    let counts = store.vision_counts()?;
    let (today, total, cost_usd) = store.vision_usage(today_start())?;
    let pc = cfg.ai.active().and_then(|p| p.classify.as_ref());
    let model = pc.map(|pc| {
        let m = cfg
            .vision
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or(&pc.model);
        format!("{} · {m}", pc.provider.as_str())
    });
    let local = pc.is_some_and(|pc| pc.provider == crate::config::ProviderKind::Ollama);
    let note = if cfg.vision.enabled {
        config_blocker(cfg)
    } else {
        None
    };
    Ok(VisionStatus {
        enabled: cfg.vision.enabled,
        model,
        local,
        note,
        counts,
        today,
        daily_limit: cfg.vision.daily_limit,
        total,
        cost_usd,
    })
}

/// Result of one background pass.
#[derive(Debug, Clone, Default, Serialize)]
pub struct VisionReport {
    /// Pictures described (or taken from the cache).
    pub described: usize,
    /// Nothing to describe in them.
    pub empty: usize,
    pub failed: usize,
    /// Judged personal information: never sent.
    pub skipped: usize,
    /// Still waiting after the pass.
    pub remaining: u64,
    /// Today's limit is used up (the rest waits for tomorrow).
    pub limited: bool,
    /// The model stopped answering; the rest waits.
    pub stopped: Option<String>,
}

/// Describe up to `limit` waiting pictures (within today's limit).
/// `wait_for_ocr`: pictures still waiting for text recognition go later.
/// `progress` gets each path.
pub fn run_pass(
    cfg: &Config,
    store: &Store,
    describer: &VisionDescriber,
    limit: usize,
    wait_for_ocr: bool,
    mut progress: impl FnMut(&Path),
) -> Result<VisionReport, StoreError> {
    let mut report = VisionReport {
        skipped: store.vision_skip_personal()?,
        ..Default::default()
    };
    let (today, _, _) = store.vision_usage(today_start())?;
    let budget = u64::from(cfg.vision.daily_limit).saturating_sub(today) as usize;
    let mut sent = 0usize;
    for c in store.vision_candidates(limit, wait_for_ocr)? {
        let cached = match &c.hash {
            Some(h) => store.vision_cache_get(h)?,
            None => None,
        };
        let result = match cached {
            Some(cap) => Ok(cap),
            None => {
                if sent >= budget {
                    report.limited = true;
                    break;
                }
                progress(&c.path);
                sent += 1;
                let _slot = c.network.then(|| crate::netio::network_io().acquire());
                describer.describe_cached(store, &c.path, &c.name, c.hash.as_deref())
            }
        };
        match result {
            Ok(cap) if cap.is_empty() => {
                report.empty += 1;
                store.set_vision_state(c.id, VisionState::Empty)?;
            }
            Ok(cap) => {
                report.described += 1;
                store.set_vision_caption(c.id, &cap.line())?;
            }
            Err(VisionError::File(e)) => {
                tracing::debug!(path = %c.path.display(), "vision failed: {e}");
                report.failed += 1;
                store.set_vision_state(c.id, VisionState::Failed)?;
            }
            Err(VisionError::Unavailable(e)) => {
                report.stopped = Some(e);
                break;
            }
            Err(VisionError::Store(e)) => return Err(e),
        }
    }
    report.remaining = store.vision_counts()?.pending;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        let max = 20 << 20;
        assert!(is_target("jpg", 50_000, max));
        assert!(!is_target("jpg", 5_000, max));
        assert!(!is_target("jpg", 30 << 20, max));
        assert!(!is_target("pdf", 50_000, max));
    }

    #[test]
    fn answers_are_checked() {
        let c = parse_caption(serde_json::json!({
            "caption": "  카페   영수증 ",
            "tags": ["#영수증", "카페", "카페", "", "커피"],
            "kind": "nonsense"
        }))
        .unwrap();
        assert_eq!(c.caption, "카페 영수증");
        assert_eq!(c.tags, vec!["영수증", "카페", "커피"]);
        assert_eq!(c.kind.as_deref(), Some("other"));
        assert!(parse_caption(serde_json::json!({"tags": []})).is_err());
    }

    #[test]
    fn errors_split_into_file_and_unavailable() {
        assert!(matches!(
            VisionError::from_ai(AiError::Refusal),
            VisionError::File(_)
        ));
        assert!(matches!(
            VisionError::from_ai(AiError::Http {
                provider: "x",
                status: 400,
                message: String::new()
            }),
            VisionError::File(_)
        ));
        assert!(matches!(
            VisionError::from_ai(AiError::Timeout(5)),
            VisionError::Unavailable(_)
        ));
        assert!(matches!(
            VisionError::from_ai(AiError::Http {
                provider: "x",
                status: 529,
                message: String::new()
            }),
            VisionError::Unavailable(_)
        ));
    }

    #[test]
    fn big_pictures_are_shrunk_to_jpeg() {
        use base64::Engine as _;
        let tmp = tempfile::tempdir().unwrap();
        // 3000×1000 RGBA PNG with a transparent half.
        let mut img = image::RgbaImage::new(3000, 1000);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 1500 {
                image::Rgba([10, 120, 200, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            };
        }
        let p = tmp.path().join("큰 사진.png");
        img.save(&p).unwrap();
        let out = prepare_image(&p, 1000).unwrap();
        assert_eq!(out.media_type, "image/jpeg");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&out.base64)
            .unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!((back.width(), back.height()), (1000, 333));
        // Transparent → white, not black.
        assert!(
            back.get_pixel(900, 100)[0] > 240,
            "{:?}",
            back.get_pixel(900, 100)
        );

        // A small PNG goes as it is.
        let small = tmp.path().join("small.png");
        image::RgbImage::from_pixel(200, 100, image::Rgb([1, 2, 3]))
            .save(&small)
            .unwrap();
        let out = prepare_image(&small, 1568).unwrap();
        assert_eq!(out.media_type, "image/png");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&out.base64)
                .unwrap(),
            std::fs::read(&small).unwrap()
        );
        // Not a picture.
        let junk = tmp.path().join("junk.jpg");
        std::fs::write(&junk, b"not an image at all").unwrap();
        assert!(matches!(
            prepare_image(&junk, 1568),
            Err(VisionError::File(_))
        ));
    }
}
