//! Process-wide limit on concurrent heavy network IO (SPEC §5.9: "NAS 동시
//! IO 1~2개"). Copy-verify moves and content reads of network roots take a
//! slot; metadata calls (stat, directory listings) do not.

use std::sync::{Condvar, Mutex, MutexGuard};

pub struct IoSlots {
    /// (in use, limit)
    state: Mutex<(usize, usize)>,
    freed: Condvar,
}

/// Held while doing heavy IO; the slot is returned on drop.
pub struct IoGuard<'a>(&'a IoSlots);

impl IoSlots {
    pub const fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new((0, limit)),
            freed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, (usize, usize)> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Change the limit (at least 1). Waiters re-check immediately.
    pub fn set_limit(&self, limit: usize) {
        self.lock().1 = limit.max(1);
        self.freed.notify_all();
    }

    pub fn limit(&self) -> usize {
        self.lock().1
    }

    /// Block until a slot is free.
    pub fn acquire(&self) -> IoGuard<'_> {
        let mut st = self.lock();
        while st.0 >= st.1.max(1) {
            st = self.freed.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        st.0 += 1;
        IoGuard(self)
    }
}

impl Drop for IoGuard<'_> {
    fn drop(&mut self) {
        let mut st = self.0.lock();
        st.0 = st.0.saturating_sub(1);
        drop(st);
        self.0.freed.notify_one();
    }
}

static NETWORK_IO: IoSlots = IoSlots::new(2);

/// The process-wide slots (`[mover] nas_concurrency`).
pub fn network_io() -> &'static IoSlots {
    &NETWORK_IO
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn never_more_than_limit_at_once() {
        let slots = Arc::new(IoSlots::new(2));
        let busy = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..6)
            .map(|_| {
                let (slots, busy, peak) = (slots.clone(), busy.clone(), peak.clone());
                std::thread::spawn(move || {
                    let _g = slots.acquire();
                    let now = busy.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    busy.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        slots.set_limit(0);
        assert_eq!(slots.limit(), 1);
    }
}
