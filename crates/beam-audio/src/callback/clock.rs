//! The two clocks audio.py reads: `time.monotonic()` (route/road/lineup/junction timing,
//! obstacle re-arm) and `time.perf_counter()` (scanner closing speed, cannon aim and dock
//! staleness). Kept apart because Python uses them deliberately, and injectable because
//! every audio sim drives the controller against a fake clock.

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

/// Seconds on each clock, as `f64`, matching Python's float return values.
pub trait Clock: Send + Sync {
    /// `time.monotonic()`.
    fn monotonic(&self) -> f64;
    /// `time.perf_counter()`.
    fn perf_counter(&self) -> f64;
}

/// The real clocks. Both are `Instant`-based (Windows QPC), offset so neither starts at 0:
/// a few state machines treat 0.0 as "never" and Python's clocks are never 0 in practice.
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Offset added to the elapsed time so the clock reads like a long-running system clock.
const SYSTEM_CLOCK_BASE_S: f64 = 1000.0;

impl Clock for SystemClock {
    fn monotonic(&self) -> f64 {
        SYSTEM_CLOCK_BASE_S + self.origin.elapsed().as_secs_f64()
    }
    fn perf_counter(&self) -> f64 {
        SYSTEM_CLOCK_BASE_S + self.origin.elapsed().as_secs_f64()
    }
}

/// A clock the tests move by hand, the equivalent of the sims' `patch("audio.time.monotonic")`.
/// Both readings are set together unless a test sets them separately.
#[derive(Default)]
pub struct FakeClock {
    inner: Mutex<(f64, f64)>,
}

impl FakeClock {
    pub fn new(start: f64) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new((start, start)),
        })
    }
    /// Set both clocks.
    pub fn set(&self, t: f64) {
        *self.inner.lock() = (t, t);
    }
    /// Advance both clocks.
    pub fn advance(&self, dt: f64) {
        let mut g = self.inner.lock();
        g.0 += dt;
        g.1 += dt;
    }
    pub fn set_monotonic(&self, t: f64) {
        self.inner.lock().0 = t;
    }
    pub fn set_perf_counter(&self, t: f64) {
        self.inner.lock().1 = t;
    }
}

impl Clock for FakeClock {
    fn monotonic(&self) -> f64 {
        self.inner.lock().0
    }
    fn perf_counter(&self) -> f64 {
        self.inner.lock().1
    }
}
