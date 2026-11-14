//! Owner: EA. `threading.Event`, `threading.Timer`, the `STOP` event and the two clocks.
//!
//! Python uses `time.time()` (wall clock) and `time.monotonic()` deliberately and stores
//! both as float seconds, with `0.0` meaning "never". Both are reproduced as `f64`
//! seconds so the stored fields keep their Python names, values and sentinels.

use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::{Condvar, Mutex};

/// `threading.Event`.
#[derive(Debug, Default)]
pub struct Event {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Event {
    pub const fn new() -> Self {
        Event {
            flag: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    pub fn set(&self) {
        let mut f = self.flag.lock();
        *f = true;
        self.cv.notify_all();
    }

    pub fn clear(&self) {
        *self.flag.lock() = false;
    }

    pub fn is_set(&self) -> bool {
        *self.flag.lock()
    }

    /// `event.wait(timeout)`: true if the flag is set (on entry or before the timeout).
    /// `None` waits forever. A negative or zero timeout only samples the flag.
    pub fn wait(&self, timeout: Option<Duration>) -> bool {
        let mut f = self.flag.lock();
        if *f {
            return true;
        }
        match timeout {
            None => {
                while !*f {
                    self.cv.wait(&mut f);
                }
                true
            }
            Some(d) => {
                let deadline = Instant::now() + d;
                while !*f {
                    if self.cv.wait_until(&mut f, deadline).timed_out() {
                        return *f;
                    }
                }
                true
            }
        }
    }

    /// `event.wait(seconds)` with Python's float seconds.
    pub fn wait_secs(&self, seconds: f64) -> bool {
        self.wait(Some(secs(seconds)))
    }
}

/// Float seconds as a `Duration`; negative and NaN become zero, as `Event.wait` treats them.
pub fn secs(seconds: f64) -> Duration {
    if seconds.is_nan() || seconds <= 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(seconds.min(1.0e9))
    }
}

/// The process-wide `STOP` event. `_handle_sigint`, the window's close and the engine's
/// `finally` all set it; every listener and worker loop polls it.
pub static STOP: Event = Event::new();

/// `threading.Timer(interval, fn)`, started immediately (Python always calls `.start()`
/// on the line after constructing one). Cancelling before it fires stops it firing;
/// cancelling after is a no-op, as in Python.
#[derive(Debug, Clone)]
pub struct Timer {
    finished: Arc<Event>,
}

impl Timer {
    pub fn start<F>(interval_s: f64, function: F) -> Timer
    where
        F: FnOnce() + Send + 'static,
    {
        let finished = Arc::new(Event::new());
        let f2 = Arc::clone(&finished);
        let spawned = std::thread::Builder::new()
            .name("Timer".to_string())
            .spawn(move || {
                // Timer.run: wait, then fire unless cancelled, then mark finished.
                if !f2.wait(Some(secs(interval_s))) {
                    function();
                }
                f2.set();
            });
        if let Err(e) = spawned {
            log::error!("Failed to start timer thread: {e}");
        }
        Timer { finished }
    }

    pub fn cancel(&self) {
        self.finished.set();
    }
}

/// Run `f` on a detached thread (Python's `threading.Thread(target=f, daemon=True).start()`).
/// A failure to spawn is logged, as an exception from `start()` would surface in Python.
pub fn spawn_daemon<F>(name: &str, f: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(e) = std::thread::Builder::new().name(name.to_string()).spawn(f) {
        log::error!("Failed to start thread {name}: {e}");
    }
}

/// `time.sleep(seconds)`.
pub fn sleep(seconds: f64) {
    std::thread::sleep(secs(seconds));
}

/// `time.time()`: seconds since the Unix epoch.
pub fn time_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

struct MonoBase {
    at: Instant,
    uptime_s: f64,
}

static MONO_BASE: LazyLock<MonoBase> = LazyLock::new(|| MonoBase {
    at: Instant::now(),
    uptime_s: uptime_secs(),
});

#[cfg(windows)]
fn uptime_secs() -> f64 {
    // SAFETY: GetTickCount64 takes no arguments and cannot fail.
    let ms = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    ms as f64 / 1000.0
}

#[cfg(not(windows))]
fn uptime_secs() -> f64 {
    1000.0
}

/// `time.monotonic()`. Like Python's it counts from system boot, so it is never near
/// zero and `0.0` stays usable as the "never" sentinel every stamp in the state uses.
pub fn monotonic() -> f64 {
    let base = &*MONO_BASE;
    base.uptime_s + base.at.elapsed().as_secs_f64()
}

/// `datetime.datetime.now().strftime('%H:%M:%S.%f')[:-3]`: local time to the millisecond.
pub fn local_hms_millis() -> String {
    #[cfg(windows)]
    {
        // SAFETY: GetLocalTime fills a caller-owned SYSTEMTIME and cannot fail.
        let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
    #[cfg(not(windows))]
    {
        let t = time_time();
        let day = t.rem_euclid(86400.0);
        let h = (day / 3600.0) as u32;
        let m = ((day % 3600.0) / 60.0) as u32;
        let s = (day % 60.0) as u32;
        let ms = ((t.fract()) * 1000.0) as u32;
        format!("{h:02}:{m:02}:{s:02}.{ms:03}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn event_wait_times_out_and_wakes() {
        let e = Arc::new(Event::new());
        assert!(!e.wait_secs(0.01));
        let e2 = Arc::clone(&e);
        std::thread::spawn(move || {
            sleep(0.02);
            e2.set();
        });
        assert!(e.wait_secs(5.0));
        assert!(e.is_set());
        e.clear();
        assert!(!e.is_set());
        assert!(!e.wait_secs(-1.0));
    }

    #[test]
    fn timer_fires_and_cancels() {
        let n = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&n);
        let _t = Timer::start(0.01, move || {
            a.fetch_add(1, Ordering::SeqCst);
        });
        let b = Arc::clone(&n);
        let t2 = Timer::start(0.2, move || {
            b.fetch_add(10, Ordering::SeqCst);
        });
        t2.cancel();
        sleep(0.4);
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn clocks_look_like_pythons() {
        let m = monotonic();
        assert!(m > 1.0, "monotonic counts from boot, never near zero");
        assert!(monotonic() >= m);
        assert!(time_time() > 1.7e9);
        assert_eq!(local_hms_millis().len(), 12);
    }
}
