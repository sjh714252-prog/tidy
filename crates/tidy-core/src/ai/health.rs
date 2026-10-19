//! AI availability from consecutive call results.
//!
//! One failed call says little (a slow answer, one bad file), so the state
//! only turns [`AiAvailability::Unavailable`] after `threshold` failures in
//! a row, and back to [`AiAvailability::Ready`] on the next success. While
//! unavailable, callers skip the AI (rules-only, no 20 s timeouts per file,
//! CLAUDE.md rule 4) and probe now and then with a cheap request; see
//! [`AiHealth::probe_due`].

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AiAvailability {
    Ready,
    Unavailable,
}

#[derive(Debug)]
pub struct AiHealth {
    threshold: u32,
    failures: AtomicU32,
    unavailable: AtomicBool,
    /// A state change not yet picked up by [`AiHealth::take_change`].
    changed: AtomicBool,
    last_error: Mutex<Option<String>>,
    probe: Mutex<Probe>,
}

#[derive(Debug)]
struct Probe {
    next: Option<Instant>,
    interval: Duration,
}

/// First probe after going unavailable, doubled after each failed probe.
pub const PROBE_FIRST: Duration = Duration::from_secs(60);
pub const PROBE_MAX: Duration = Duration::from_secs(600);

impl AiHealth {
    /// `threshold` consecutive failures make the AI unavailable (min 1).
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold: threshold.max(1),
            failures: AtomicU32::new(0),
            unavailable: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            last_error: Mutex::new(None),
            probe: Mutex::new(Probe {
                next: None,
                interval: PROBE_FIRST,
            }),
        }
    }

    pub fn availability(&self) -> AiAvailability {
        if self.unavailable.load(Ordering::SeqCst) {
            AiAvailability::Unavailable
        } else {
            AiAvailability::Ready
        }
    }

    pub fn is_available(&self) -> bool {
        self.availability() == AiAvailability::Ready
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.failures.load(Ordering::SeqCst)
    }

    /// Last error message (never contains keys: provider errors don't).
    pub fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn record_success(&self) {
        self.failures.store(0, Ordering::SeqCst);
        if self.unavailable.swap(false, Ordering::SeqCst) {
            self.changed.store(true, Ordering::SeqCst);
            tracing::info!("AI is reachable again");
        }
        let mut p = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        p.next = None;
        p.interval = PROBE_FIRST;
    }

    pub fn record_failure(&self, error: &str) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error.to_string());
        let n = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        let mut p = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        if self.unavailable.load(Ordering::SeqCst) {
            // A failed probe (or call): wait longer before the next one.
            p.interval = (p.interval * 2).min(PROBE_MAX);
            p.next = Some(Instant::now() + p.interval);
        } else if n >= self.threshold {
            self.unavailable.store(true, Ordering::SeqCst);
            self.changed.store(true, Ordering::SeqCst);
            p.interval = PROBE_FIRST;
            p.next = Some(Instant::now() + p.interval);
            tracing::warn!(failures = n, "AI marked unavailable: {error}");
        }
    }

    /// The new availability if it changed since the last call.
    pub fn take_change(&self) -> Option<AiAvailability> {
        self.changed
            .swap(false, Ordering::SeqCst)
            .then(|| self.availability())
    }

    /// While unavailable: is it time for a probe request?
    pub fn probe_due(&self) -> bool {
        if self.is_available() {
            return false;
        }
        let p = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        p.next.is_none_or(|t| Instant::now() >= t)
    }
}

impl Default for AiHealth {
    fn default() -> Self {
        Self::new(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_after_n_failures_and_back_on_success() {
        let h = AiHealth::new(3);
        h.record_failure("a");
        h.record_failure("b");
        assert!(h.is_available());
        assert_eq!(h.take_change(), None);
        h.record_failure("c");
        assert_eq!(h.availability(), AiAvailability::Unavailable);
        assert_eq!(h.take_change(), Some(AiAvailability::Unavailable));
        assert_eq!(h.take_change(), None);
        assert_eq!(h.last_error().as_deref(), Some("c"));
        // Not due right away; a probe is scheduled.
        assert!(!h.probe_due());
        h.record_failure("d");
        assert_eq!(h.take_change(), None);
        h.record_success();
        assert_eq!(h.take_change(), Some(AiAvailability::Ready));
        assert_eq!(h.consecutive_failures(), 0);
        assert!(!h.probe_due());
        // A success in between resets the count.
        h.record_failure("x");
        h.record_failure("x");
        h.record_success();
        h.record_failure("x");
        assert!(h.is_available());
    }
}
