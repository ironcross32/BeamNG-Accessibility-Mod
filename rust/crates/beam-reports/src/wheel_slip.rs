//! Wheel slip (spin / lockup) and side-slide detection, and where to place the voice.
//!
//! No engine, audio or speech dependencies, so the live telemetry loop, the road
//! diagnostic and challenge analysers, and the sim all run the same rule.
//!
//! Three things this module exists to get right, each measured on the Ice Pool:
//!
//! * **The wheel signal is the raw per-wheel extreme, never `electrics.wheelspeed`.**
//!   That electrics value is a smoothed average of every wheel: it trailed a 30 ms
//!   spin-up by about 0.4 s. The vehicle VM sends the fastest driven wheel and the
//!   slowest wheel of any kind.
//! * **Spin has no ground-speed gate; lockup does.** A wheel doing 10 m/s on a car doing
//!   0.1 m/s is unambiguous; gating spin made a clutch dump on ice sound a second late.
//!   At a standstill every wheel "looks locked", so lockup keeps the gate.
//! * **Wheels are compared with LONGITUDINAL travel only.** Against total ground speed a
//!   sideways slide reads as lockup. Sideways travel is its own voice (the slide).
//!
//! The voice's pitch follows the **slip ratio** (divergence over the faster of wheel
//! and MotionSim ground speed, bounded 0..1), never the wheel speed: the game's speed
//! readout is wheel-derived.
//!
//! Bearings follow the mod-wide convention: **positive is the driver's LEFT**.

use beam_base::pyfmt::py_round;
use beam_base::route_beacon::{normalize_bearing, relative_bearing};

use crate::pyops::{PyDict, PyErr, PyValue, py_max, py_min};

// --- Spin / lockup (raw wheel extremes) ---
/// EMA on the divergence; the raw signal is already clean.
pub const SLIP_TAU_S: f64 = 0.05;
/// Divergence must persist this long (shift and packet jitter).
pub const SLIP_SUSTAIN_S: f64 = 0.08;
pub const SLIP_ABS_THRESHOLD_MS: f64 = 1.5;
pub const SLIP_REL_THRESHOLD: f64 = 0.25;
/// LOCKUP only -- see the module doc.
pub const SLIP_MIN_GROUND_MS: f64 = 2.0;

// --- Legacy rule, for a mod half that does not send the wheel extremes ---
// Exactly the old behaviour, so an older bng_mod degrades to it rather than to silence.
pub const LEGACY_TAU_S: f64 = 0.10;
pub const LEGACY_SUSTAIN_S: f64 = 0.15;

// --- Side slide ---
/// Below this the reference node's heading error is noise.
pub const SLIDE_MIN_GROUND_MS: f64 = 3.0;
pub const SLIDE_ENTER_DEG: f64 = 12.0;
/// Hysteresis: a drift held near the threshold must not chatter.
pub const SLIDE_EXIT_DEG: f64 = 8.0;
pub const SLIDE_SUSTAIN_S: f64 = 0.10;

// --- Voice placement ---
/// Travel within this of the car's own axis is "the direction the wheels are turning":
/// the voice sits dead ahead or dead behind rather than wobbling by a few degrees.
pub const AZIMUTH_SNAP_DEG: f64 = 10.0;
/// Below DIR_MIN the direction of travel is numerical residue, so the voice is placed
/// by the gear; the two are blended up to DIR_FULL.
pub const DIR_MIN_MS: f64 = 0.5;
pub const DIR_FULL_MS: f64 = 2.0;

pub const KIND_NONE: i32 = 0;
pub const KIND_SPIN: i32 = 1;
pub const KIND_LOCK: i32 = -1;

/// One detector step's answer (Python's `SlipResult` named tuple).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlipResult {
    pub active: bool,
    /// KIND_SPIN, KIND_LOCK or KIND_NONE.
    pub kind: i32,
    pub magnitude_ms: f64,
    pub slide_active: bool,
    pub slide_deg: f64,
    /// +LEFT, -180..180.
    pub azimuth_deg: f64,
    pub v_long_ms: f64,
    /// Signed: + means ground faster than wheels (lockup side).
    pub filtered_ms: f64,
    pub legacy: bool,
    /// Slip ratio 0..1, see [`slip_ratio`].
    pub ratio: f64,
}

pub const INACTIVE: SlipResult = SlipResult {
    active: false,
    kind: KIND_NONE,
    magnitude_ms: 0.0,
    slide_active: false,
    slide_deg: 0.0,
    azimuth_deg: 0.0,
    v_long_ms: 0.0,
    filtered_ms: 0.0,
    legacy: false,
    ratio: 0.0,
};

/// Divergence as a share of the faster speed, 0..1.
///
/// `v_long_ms` is the MotionSim ground speed along the car's axis. For spin the wheel is
/// the faster (`v_long + magnitude`); for lockup the ground is.
pub fn slip_ratio(kind: i32, magnitude_ms: f64, v_long_ms: f64) -> f64 {
    let mag = py_max(0.0, magnitude_ms);
    let v = py_max(0.0, v_long_ms);
    let denom = if kind == KIND_SPIN {
        v + mag
    } else if kind == KIND_LOCK {
        py_max(v, mag)
    } else {
        return 0.0;
    };
    if denom <= 1e-6 {
        return 0.0;
    }
    py_max(0.0, py_min(1.0, mag / denom))
}

/// Signed bearing of the direction of TRAVEL relative to the car's nose, +LEFT.
///
/// `heading_deg` is BeamTel's MotionSim heading, 180 degrees off a true compass heading;
/// `route_beacon::relative_bearing` carries the matching offset, so the velocity vector
/// is handed to it as a destination seen from the origin. Do not re-derive this: a lone
/// correction mirrors every answer front to back.
pub fn travel_bearing(vel_x: f64, vel_y: f64, heading_deg: f64) -> f64 {
    relative_bearing(0.0, 0.0, heading_deg, vel_x, vel_y).1
}

/// How far travel is off the car's axis, 0..90, either end of the car.
pub fn sideslip_deg(bearing_deg: f64) -> f64 {
    let b = normalize_bearing(bearing_deg).abs();
    py_min(b, 180.0 - b)
}

/// Where the slip voices sit: the direction of travel, +LEFT, -180..180.
pub fn voice_azimuth(bearing_deg: Option<f64>, ground_ms: f64, reverse: bool) -> f64 {
    let gear_az = if reverse { 180.0 } else { 0.0 };
    let Some(bearing_deg) = bearing_deg else {
        return gear_az;
    };
    let mut b = normalize_bearing(bearing_deg);
    if b.abs() < AZIMUTH_SNAP_DEG {
        b = 0.0;
    } else if b.abs() > 180.0 - AZIMUTH_SNAP_DEG {
        b = 180.0;
    }
    let w = (ground_ms - DIR_MIN_MS) / (DIR_FULL_MS - DIR_MIN_MS);
    let w = py_max(0.0, py_min(1.0, w));
    if w >= 1.0 {
        return b;
    }
    if w <= 0.0 {
        return gear_az;
    }
    // Blend as unit vectors so ahead-to-behind never sweeps through the side at random.
    let x = (1.0 - w) * gear_az.to_radians().cos() + w * b.to_radians().cos();
    let y = (1.0 - w) * gear_az.to_radians().sin() + w * b.to_radians().sin();
    if x.abs() < 1e-9 && y.abs() < 1e-9 {
        return b;
    }
    y.atan2(x).to_degrees()
}

fn threshold(v_long: f64) -> f64 {
    py_max(SLIP_ABS_THRESHOLD_MS, SLIP_REL_THRESHOLD * v_long)
}

#[doc(hidden)]
/// The private `_threshold`, for the sim's "naive rule" check.
pub fn threshold_for_tests(v_long: f64) -> f64 {
    threshold(v_long)
}

fn has_wheels(wheel_max: Option<f64>, wheel_min: Option<f64>) -> bool {
    matches!((wheel_max, wheel_min), (Some(a), Some(b)) if a >= 0.0 && b >= 0.0)
}

/// Stateful detector, stepped once per telemetry packet.
#[derive(Debug, Clone, Default)]
pub struct SlipDetector {
    prev_ts: f64,
    spin_sm: f64,
    lock_sm: f64,
    spin_since: f64,
    lock_since: f64,
    legacy_sm: f64,
    legacy_since: f64,
    slide_on: bool,
    slide_since: f64,
}

/// Which `_since` field `_sustain` updates.
#[derive(Clone, Copy)]
enum Since {
    Spin,
    Lock,
}

impl SlipDetector {
    pub fn new() -> Self {
        let mut d = SlipDetector::default();
        d.reset();
        d
    }

    pub fn reset(&mut self) {
        self.prev_ts = 0.0;
        self.spin_sm = 0.0;
        self.lock_sm = 0.0;
        self.spin_since = 0.0;
        self.lock_since = 0.0;
        self.legacy_sm = 0.0;
        self.legacy_since = 0.0;
        self.slide_on = false;
        self.slide_since = 0.0;
    }

    /// Advance by one packet. `bearing_deg` is `None` when travel has no direction; a
    /// wheel extreme of `None` (or negative, the wire's `-1`) selects the legacy rule.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        now: f64,
        ground_ms: f64,
        bearing_deg: Option<f64>,
        wheel_max_driven: Option<f64>,
        wheel_min: Option<f64>,
        legacy_wheel_ms: Option<f64>,
        reverse: bool,
    ) -> SlipResult {
        let dt = if self.prev_ts > 0.0 {
            now - self.prev_ts
        } else {
            0.0
        };
        let rebase = !(0.0 < dt && dt <= 1.0);
        self.prev_ts = now;

        let v_long = match bearing_deg {
            Some(b) => (ground_ms * b.to_radians().cos()).abs(),
            None => ground_ms,
        };

        let legacy = !has_wheels(wheel_max_driven, wheel_min);
        let (active, kind, mag, filtered) = if legacy {
            self.step_legacy(now, dt, rebase, ground_ms, legacy_wheel_ms)
        } else {
            self.step_raw(
                now,
                dt,
                rebase,
                v_long,
                wheel_max_driven.unwrap_or(0.0),
                wheel_min.unwrap_or(0.0),
            )
        };

        let (slide_active, slide_deg) = self.step_slide(now, ground_ms, bearing_deg);
        let azimuth = voice_azimuth(bearing_deg, ground_ms, reverse);
        let ratio = if active {
            slip_ratio(kind, mag, if legacy { ground_ms } else { v_long })
        } else {
            0.0
        };
        SlipResult {
            active,
            kind,
            magnitude_ms: mag,
            slide_active,
            slide_deg,
            azimuth_deg: azimuth,
            v_long_ms: v_long,
            filtered_ms: filtered,
            legacy,
            ratio,
        }
    }

    /// Spin / lockup from the raw wheel extremes.
    fn step_raw(
        &mut self,
        now: f64,
        dt: f64,
        rebase: bool,
        v_long: f64,
        wheel_max: f64,
        wheel_min: f64,
    ) -> (bool, i32, f64, f64) {
        let spin_raw = wheel_max - v_long;
        let lock_raw = v_long - wheel_min;
        if rebase {
            self.spin_sm = spin_raw;
            self.lock_sm = lock_raw;
        } else {
            let alpha = 1.0 - (-dt / SLIP_TAU_S).exp();
            self.spin_sm += alpha * (spin_raw - self.spin_sm);
            self.lock_sm += alpha * (lock_raw - self.lock_sm);
        }
        self.legacy_sm = 0.0;
        self.legacy_since = 0.0;

        let thr = threshold(v_long);
        let spin_on = self.sustain(Since::Spin, now, self.spin_sm > thr);
        let lock_on = self.sustain(
            Since::Lock,
            now,
            v_long > SLIP_MIN_GROUND_MS && self.lock_sm > thr,
        );
        // Positive = lockup side, matching the legacy filtered value.
        let filtered = if self.lock_sm >= self.spin_sm {
            self.lock_sm
        } else {
            -self.spin_sm
        };
        if spin_on && (!lock_on || self.spin_sm >= self.lock_sm) {
            return (true, KIND_SPIN, self.spin_sm, filtered);
        }
        if lock_on {
            return (true, KIND_LOCK, self.lock_sm, filtered);
        }
        (false, KIND_NONE, 0.0, filtered)
    }

    /// The pre-v4 rule, unchanged.
    fn step_legacy(
        &mut self,
        now: f64,
        dt: f64,
        rebase: bool,
        ground_ms: f64,
        wheel_ms: Option<f64>,
    ) -> (bool, i32, f64, f64) {
        // `wheel_ms or 0.0`: None and both zeros are falsy.
        let wheel = match wheel_ms {
            Some(w) if w != 0.0 => w,
            _ => 0.0,
        };
        let raw = ground_ms - wheel;
        if rebase {
            self.legacy_sm = raw;
        } else {
            let alpha = 1.0 - (-dt / LEGACY_TAU_S).exp();
            self.legacy_sm += alpha * (raw - self.legacy_sm);
        }
        self.spin_sm = 0.0;
        self.lock_sm = 0.0;
        self.spin_since = 0.0;
        self.lock_since = 0.0;

        let thr = threshold(ground_ms);
        let diverging = ground_ms > SLIP_MIN_GROUND_MS && self.legacy_sm.abs() > thr;
        if diverging {
            if self.legacy_since == 0.0 {
                self.legacy_since = now;
            }
            if (now - self.legacy_since) >= LEGACY_SUSTAIN_S {
                let kind = if self.legacy_sm > 0.0 {
                    KIND_LOCK
                } else {
                    KIND_SPIN
                };
                return (true, kind, self.legacy_sm.abs(), self.legacy_sm);
            }
        } else {
            self.legacy_since = 0.0;
        }
        (false, KIND_NONE, 0.0, self.legacy_sm)
    }

    /// Sideways travel.
    fn step_slide(&mut self, now: f64, ground_ms: f64, bearing_deg: Option<f64>) -> (bool, f64) {
        let bearing_deg = match bearing_deg {
            Some(b) if !(ground_ms <= SLIDE_MIN_GROUND_MS) => b,
            _ => {
                self.slide_on = false;
                self.slide_since = 0.0;
                return (false, 0.0);
            }
        };
        let beta = sideslip_deg(bearing_deg);
        if self.slide_on {
            if beta < SLIDE_EXIT_DEG {
                self.slide_on = false;
                self.slide_since = 0.0;
            }
        } else if beta > SLIDE_ENTER_DEG {
            if self.slide_since == 0.0 {
                self.slide_since = now;
            }
            if (now - self.slide_since) >= SLIDE_SUSTAIN_S {
                self.slide_on = true;
            }
        } else {
            self.slide_since = 0.0;
        }
        (self.slide_on, if self.slide_on { beta } else { 0.0 })
    }

    fn sustain(&mut self, which: Since, now: f64, cond: bool) -> bool {
        let slot = match which {
            Since::Spin => &mut self.spin_since,
            Since::Lock => &mut self.lock_since,
        };
        if !cond {
            *slot = 0.0;
            return false;
        }
        if *slot == 0.0 {
            *slot = now;
        }
        (now - *slot) >= SLIP_SUSTAIN_S
    }
}

/// `_finite`: `float(value)` if finite, else `None`; `TypeError`/`ValueError` give
/// `None`, any other exception (`OverflowError` for a huge int) propagates.
fn finite(value: Option<&PyValue>) -> Result<Option<f64>, PyErr> {
    let v = value.unwrap_or(&PyValue::None);
    match v.py_float() {
        Ok(f) => Ok(if f.is_finite() { Some(f) } else { None }),
        Err(e) if e.is_type_or_value() => Ok(None),
        Err(e) => Err(e),
    }
}

/// [`classify`]'s answer (Python's dict of the same keys).
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub active: bool,
    /// `"wheelspin"`, `"lockup"` or `"none"`.
    pub kind: &'static str,
    pub magnitude_mps: f64,
    /// Signed: positive means the ground is faster than the wheels.
    pub raw_mps: f64,
    pub ratio: f64,
    pub slide: bool,
    pub slide_deg: f64,
}

/// Stateless one-shot classification of a recorded telemetry snapshot.
///
/// For the analysers (road diagnostics, challenge results). Uses the raw wheel extremes
/// and the travel bearing when the sample carries them, and the legacy total-speed rule
/// otherwise, so recordings made before those fields existed analyse as they did.
///
/// `Err` only where Python's `float()` raises something other than `TypeError` or
/// `ValueError` (an int too large for a float).
pub fn classify(sample: &PyDict) -> Result<Classification, PyErr> {
    let ground = finite(sample.get("ground_speed_ms"))?;
    // `_finite(...) or 0.0`: a finite zero is falsy too, and becomes +0.0.
    let ground = match ground {
        Some(g) if g != 0.0 => g,
        _ => 0.0,
    };
    let bearing = finite(sample.get("travel_bearing_deg"))?;
    let wmax = finite(sample.get("wheel_max_driven_ms"))?;
    let wmin = finite(sample.get("wheel_min_ms"))?;

    let slide_deg = match bearing {
        Some(b) => sideslip_deg(b),
        None => 0.0,
    };
    let slide = bearing.is_some() && ground > SLIDE_MIN_GROUND_MS && slide_deg > SLIDE_ENTER_DEG;

    let mut v_ref = ground;
    let kind: &'static str;
    let mag: f64;
    let raw: f64;
    if has_wheels(wmax, wmin) {
        let (wmax, wmin) = (wmax.unwrap_or(0.0), wmin.unwrap_or(0.0));
        let v_long = match bearing {
            Some(b) => (ground * b.to_radians().cos()).abs(),
            None => ground,
        };
        let spin = wmax - v_long;
        let lock = v_long - wmin;
        let thr = threshold(v_long);
        let spin_on = spin > thr;
        let lock_on = v_long > SLIP_MIN_GROUND_MS && lock > thr;
        v_ref = v_long;
        if spin_on && (!lock_on || spin >= lock) {
            kind = "wheelspin";
            mag = spin;
            raw = -spin;
        } else if lock_on {
            kind = "lockup";
            mag = lock;
            raw = lock;
        } else {
            kind = "none";
            raw = if lock >= spin { lock } else { -spin };
            mag = raw.abs();
        }
    } else {
        let wheel = finite(sample.get("wheel_speed_ms"))?;
        let wheel = match wheel {
            Some(w) if w != 0.0 => w,
            _ => 0.0,
        };
        raw = ground - wheel;
        let active = ground > SLIP_MIN_GROUND_MS && raw.abs() > threshold(ground);
        kind = if active {
            if raw > 0.0 { "lockup" } else { "wheelspin" }
        } else {
            "none"
        };
        mag = raw.abs();
    }

    let kind_id = match kind {
        "wheelspin" => KIND_SPIN,
        "lockup" => KIND_LOCK,
        _ => KIND_NONE,
    };
    Ok(Classification {
        active: kind != "none",
        kind,
        magnitude_mps: py_round(mag, 4),
        raw_mps: py_round(raw, 4),
        ratio: py_round(slip_ratio(kind_id, mag, v_ref), 4),
        slide,
        slide_deg: py_round(if slide { slide_deg } else { 0.0 }, 2),
    })
}
