//! Owner: E1. Telemetry decode and the telemetry loop (`beamtel.py:644-735`,
//! `_report_gear_change` at 4306-4316, `telemetry_loop` at 10526-11481): OutGauge, the
//! Extended layouts V1..V6 with their padding, MotionSim, and everything the loop drives.
//!
//! # Shape
//!
//! The struct formats are kept as the Python's format strings and decoded by a small
//! `struct.unpack` for the codes they use, into the same positional tuple the Python indexes
//! ([`PyField`]). The float tail is positional on the wire and positional here, so an index
//! read against the Python reads the same field.
//!
//! [`telemetry_loop`] owns the socket. Each datagram goes to
//! [`TelemetryLoop::handle_datagram`], which is the loop body with the Python's locals as
//! fields and everything outside the module (speech, the audio controller, the drift
//! announcer, the UDP pushers, the clocks the shared helpers read) behind
//! [`TelemetryHooks`], so the body can be driven packet by packet by a test.
//!
//! # Locking
//!
//! The Python body is one `with state_lock:` block for a MotionSim packet, one for the
//! telemetry fields, then a stretch that reads and writes globals bare. Here the bare
//! stretch takes the guard too (engine-map section 1); it is released between the two
//! telemetry stretches exactly where the Python releases it.

use std::io;
use std::sync::Arc;

use beam_audio::controller::{AudioController, TelemetryState};
use beam_base::gear_report;
use beam_base::pyfmt::{py_f, py_mod, py_strip};
use beam_base::route_beacon::{
    AT_DESTINATION_M, normalize_bearing, relative_bearing, target_bearing,
};
use beam_reports::pyops::py_hypot;
use beam_reports::wheel_slip;
use parking_lot::MutexGuard;

use crate::net::{Decode, Recv, UdpListener, decode};
use crate::state::{
    DRIFT_OFF_RATE, DRIFT_ON_RATE, DRIFT_PAN_MIN_RATE, DRIFT_RELEASE_HOLD, DRIFT_SMOOTH_TAU,
    LOWSPEED_DIAG_INTERVAL_S, LS_ACCEL_DEADBAND, LS_ACCEL_MAX_DT, LS_ACCEL_MIN_DT, LS_ACCEL_TAU_S,
    LS_MS_STALE_S, LS_STEADY_HOLD_S, LS_STOP_ENTER_MS, LS_STOP_EXIT_MS, LS_STOP_TONE_MIN_MS,
    REVERSE, State,
};
use crate::timer::{Event, time_time};
use crate::units::{
    COMPASS_NAMES, MPH_PER_MS, compass_snap_match, fmt_speed, get_speed_bucket, py_int,
    py_int_round, yaw_to_heading_deg,
};

pub use crate::state::WHEEL_POS_CORNERS;
pub use crate::units::{
    TELEMETRY_HAS_AIR, TELEMETRY_HAS_CLUTCH, TELEMETRY_HAS_ELECTRIC, TELEMETRY_HAS_ENGINE_TEMP,
};

// =========================
//  OutGauge / MotionSim / Telemetry
// =========================

pub const OG_FORMAT: &str = "<I4sHBBfffffffIIfff16s16si";
pub const OG_SIZE: usize = calcsize(OG_FORMAT);
/// BeamNG/LFS OutGauge flag: this vehicle exposes turbo boost.
pub const OG_TURBO: i64 = 1 << 13;

// Extended telemetry. bng_mod/ is a directory junction into the game install, so the Lua
// half can move (git checkout, mod update) without beamtel restarting. A hard equality test
// on the packet length would then reject every packet and take ALL extended telemetry down
// with it, with no error anywhere. So every historical layout stays decodable and short
// packets are padded with the sentinels the newer fields use for "absent".

/// Pre-implement (no bucket/fork block).
pub const EXT_FORMAT_V1: &str = "<H4sBx9fII28f";
pub const EXT_SIZE_V1: usize = calcsize(EXT_FORMAT_V1);

/// Implement block, four corner wheels only.
pub const EXT_FORMAT_V2: &str = "<H4sBx9fII36f";
pub const EXT_SIZE_V2: usize = calcsize(EXT_FORMAT_V2);

/// v3 appends centred front/rear pressure, tire temperature and brake temperature,
/// followed by an explicit telemetry-presence bitmask. Appending keeps every v2 offset stable.
pub const EXT_FORMAT_V3: &str = "<H4sBx9fII42fI";
pub const EXT_SIZE_V3: usize = calcsize(EXT_FORMAT_V3);

/// v4 appends the raw fastest-driven and slowest wheel speeds for the slip detector.
pub const EXT_FORMAT_V4: &str = "<H4sBx9fII42fI2f";
pub const EXT_SIZE_V4: usize = calcsize(EXT_FORMAT_V4);

/// v5 appends electric drive power (kW), torque (Nm) and remaining range (m), and the
/// presence mask gains the engine-temperature and electric-drive bits.
pub const EXT_FORMAT_V5: &str = "<H4sBx9fII42fI5f";
pub const EXT_SIZE_V5: usize = calcsize(EXT_FORMAT_V5);

/// v6 appends the game's trip distance, odometer and lifetime distance in this model (m, -1
/// unknown), and the presence mask gains the air-system bit.
pub const EXT_FORMAT: &str = "<H4sBx9fII42fI8f";
pub const EXT_SIZE: usize = calcsize(EXT_FORMAT);

/// What a pre-v4 packet implies for the two wheel-extreme floats: -1 = not sent, which
/// drops the slip detector onto its legacy rule (see wheel_slip).
pub const EXT_V3_SLIP_DEFAULTS: [f64; 2] = [-1.0, -1.0];

/// What a pre-v5 packet implies for the electric drive floats. The presence bit, not these
/// values, is what hides the rows; -1 range is "unknown" for anything that reads it raw.
pub const EXT_V4_ELECTRIC_DEFAULTS: [f64; 3] = [0.0, 0.0, -1.0];

/// What a pre-v6 packet implies for trip, odometer and lifetime: unknown, which hides the rows.
pub const EXT_V5_DISTANCE_DEFAULTS: [f64; 3] = [-1.0, -1.0, -1.0];

/// Values a v1 packet implies for the eight implement floats. Mirrors the sentinels in
/// 796F6C6F313035.lua: flags 0 = nothing valid, -1 = no ground reading.
pub const EXT_V1_IMPLEMENT_DEFAULTS: [f64; 8] = [0.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

pub const WHEEL_POS_FL: i64 = 1 << 0;
pub const WHEEL_POS_FR: i64 = 1 << 1;
pub const WHEEL_POS_RL: i64 = 1 << 2;
pub const WHEEL_POS_RR: i64 = 1 << 3;
pub const WHEEL_POS_F: i64 = 1 << 4;
pub const WHEEL_POS_R: i64 = 1 << 5;

/// A v2 mod cannot tell Python which wheel names were present. Preserve its historical
/// four-corner interpretation and leave the new centred-wheel values unavailable: six
/// floats, then the (int) presence mask.
pub const EXT_V2_WHEEL_DEFAULTS: ([f64; 6], i64) = ([0.0; 6], WHEEL_POS_CORNERS);

pub const MS_MAGIC: &[u8; 4] = b"BNG1";
pub const MS_FORMAT: &str = "<4s21f";
pub const MS_SIZE: usize = calcsize(MS_FORMAT);

// Dashboard Light bitmasks
pub const DL_SHIFT: i64 = 1 << 0;
pub const DL_FULLBEAM: i64 = 1 << 1;
pub const DL_HANDBRAKE: i64 = 1 << 2;
pub const DL_TC: i64 = 1 << 4;
pub const DL_SIGNAL_L: i64 = 1 << 5;
pub const DL_SIGNAL_R: i64 = 1 << 6;
pub const DL_CHECK: i64 = 1 << 7;
pub const DL_OILWARN: i64 = 1 << 8;
pub const DL_BATTERY: i64 = 1 << 9;
pub const DL_ABS: i64 = 1 << 10;
pub const DL_LOWBEAM: i64 = 1 << 11;

/// `ALERT_INTERVAL_SEC`, the oil chime's repeat interval (a local of the Python loop).
pub const ALERT_INTERVAL_SEC: f64 = 3.0;

// ---- struct.unpack for the codes these formats use --------------------------------------

/// `struct.calcsize(fmt)` for a little-endian standard-size format (`<`, `x`, `B`, `H`, `I`,
/// `i`, `f`, `s`, with repeat counts). Evaluated at compile time for the constants above.
pub const fn calcsize(fmt: &str) -> usize {
    let b = fmt.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'<' {
        i += 1;
    }
    let mut total = 0;
    while i < b.len() {
        let mut count = 0;
        let mut has_count = false;
        while i < b.len() && b[i].is_ascii_digit() {
            count = count * 10 + (b[i] - b'0') as usize;
            has_count = true;
            i += 1;
        }
        if !has_count {
            count = 1;
        }
        let size = match b[i] {
            b'x' | b'B' | b's' => 1,
            b'H' => 2,
            b'I' | b'i' | b'f' => 4,
            _ => panic!("unsupported struct code"),
        };
        total += size * count;
        i += 1;
    }
    total
}

/// One element of an unpacked tuple, as Python holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum PyField {
    Int(i64),
    Float(f64),
    Bytes(Vec<u8>),
}

impl PyField {
    /// The element used as a number (an int or a float).
    fn num(&self) -> f64 {
        match self {
            PyField::Int(n) => *n as f64,
            PyField::Float(x) => *x,
            PyField::Bytes(_) => panic!("TypeError: bytes used as a number"),
        }
    }

    /// `int(element)`: Python's truncation, raising (panicking) on NaN and infinity.
    fn int(&self) -> i64 {
        match self {
            PyField::Int(n) => *n,
            PyField::Float(x) => py_int(*x),
            PyField::Bytes(_) => panic!("TypeError: int() of bytes"),
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            PyField::Bytes(b) => b,
            _ => panic!("TypeError: expected bytes"),
        }
    }
}

/// `int(element)`, or `None` where Python's int() raises (NaN, infinity, bytes).
fn finite_int(f: &PyField) -> Option<i64> {
    match f {
        PyField::Int(n) => Some(*n),
        PyField::Float(x) if x.is_finite() => Some(py_int(*x)),
        _ => None,
    }
}

fn take<const N: usize>(data: &[u8], off: usize) -> Option<[u8; N]> {
    data.get(off..off + N)?.try_into().ok()
}

/// `struct.unpack(fmt, data)`. `None` where Python raises `struct.error` (wrong length).
pub fn unpack(fmt: &str, data: &[u8]) -> Option<Vec<PyField>> {
    if data.len() != calcsize(fmt) {
        return None;
    }
    let b = fmt.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'<'));
    let mut off = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let mut count = 0usize;
        let mut has_count = false;
        while i < b.len() && b[i].is_ascii_digit() {
            count = count * 10 + usize::from(b[i] - b'0');
            has_count = true;
            i += 1;
        }
        if !has_count {
            count = 1;
        }
        let code = *b.get(i)?;
        i += 1;
        match code {
            b's' => {
                out.push(PyField::Bytes(data.get(off..off + count)?.to_vec()));
                off += count;
            }
            b'x' => off += count,
            _ => {
                for _ in 0..count {
                    let field = match code {
                        b'B' => PyField::Int(i64::from(take::<1>(data, off)?[0])),
                        b'H' => PyField::Int(i64::from(u16::from_le_bytes(take(data, off)?))),
                        b'I' => PyField::Int(i64::from(u32::from_le_bytes(take(data, off)?))),
                        b'i' => PyField::Int(i64::from(i32::from_le_bytes(take(data, off)?))),
                        b'f' => PyField::Float(f64::from(f32::from_le_bytes(take(data, off)?))),
                        _ => return None,
                    };
                    off += calcsize_code(code);
                    out.push(field);
                }
            }
        }
    }
    Some(out)
}

fn calcsize_code(code: u8) -> usize {
    match code {
        b'B' => 1,
        b'H' => 2,
        _ => 4,
    }
}

/// Which wire layout a telemetry packet was decoded as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    OutGauge,
    ExtendedV1,
    ExtendedV2,
    ExtendedV3,
    ExtendedV4,
    ExtendedV5,
    /// The current (v6) layout.
    Extended,
}

/// What `telemetry_loop` does with one datagram.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    /// `struct.unpack(MS_FORMAT, data[:MS_SIZE])`.
    MotionSim(Vec<PyField>),
    /// The OutGauge tuple, or the Extended tuple padded to the current layout's 65 fields.
    Telemetry {
        layout: Layout,
        unpacked: Vec<PyField>,
    },
    /// Skipped (`continue`): a short MotionSim packet, or no layout for this length and mode.
    Ignored,
}

fn floats(xs: &[f64]) -> impl Iterator<Item = PyField> + '_ {
    xs.iter().map(|&x| PyField::Float(x))
}

fn pad(mut v: Vec<PyField>, tails: &[&[f64]]) -> Vec<PyField> {
    for t in tails {
        v.extend(floats(t));
    }
    v
}

fn v2_wheel_defaults() -> Vec<PyField> {
    let (fs, mask) = EXT_V2_WHEEL_DEFAULTS;
    let mut v: Vec<PyField> = floats(&fs).collect();
    v.push(PyField::Int(mask));
    v
}

/// The loop's packet classification, in the Python's order: MotionSim magic first, then
/// the Extended layouts by exact length (protocol "extended"), then OutGauge by minimum
/// length (protocol "outgauge").
pub fn decode_datagram(data: &[u8], protocol_mode: &str) -> Decoded {
    if data.len() >= 4 && &data[..4] == MS_MAGIC {
        if data.len() >= MS_SIZE {
            return match unpack(MS_FORMAT, &data[..MS_SIZE]) {
                Some(v) => Decoded::MotionSim(v),
                None => Decoded::Ignored,
            };
        }
        return Decoded::Ignored;
    }
    let extended = protocol_mode == "extended";
    let n = data.len();
    let telemetry = |layout, v: Option<Vec<PyField>>| match v {
        Some(unpacked) => Decoded::Telemetry { layout, unpacked },
        None => Decoded::Ignored,
    };
    if extended && n == EXT_SIZE {
        telemetry(Layout::Extended, unpack(EXT_FORMAT, data))
    } else if extended && n == EXT_SIZE_V5 {
        let v = unpack(EXT_FORMAT_V5, data).map(|v| pad(v, &[&EXT_V5_DISTANCE_DEFAULTS]));
        telemetry(Layout::ExtendedV5, v)
    } else if extended && n == EXT_SIZE_V4 {
        let v = unpack(EXT_FORMAT_V4, data)
            .map(|v| pad(v, &[&EXT_V4_ELECTRIC_DEFAULTS, &EXT_V5_DISTANCE_DEFAULTS]));
        telemetry(Layout::ExtendedV4, v)
    } else if extended && n == EXT_SIZE_V3 {
        let v = unpack(EXT_FORMAT_V3, data).map(|v| {
            pad(
                v,
                &[
                    &EXT_V3_SLIP_DEFAULTS,
                    &EXT_V4_ELECTRIC_DEFAULTS,
                    &EXT_V5_DISTANCE_DEFAULTS,
                ],
            )
        });
        telemetry(Layout::ExtendedV3, v)
    } else if extended && n == EXT_SIZE_V2 {
        let v = unpack(EXT_FORMAT_V2, data).map(|mut v| {
            v.extend(v2_wheel_defaults());
            pad(
                v,
                &[
                    &EXT_V3_SLIP_DEFAULTS,
                    &EXT_V4_ELECTRIC_DEFAULTS,
                    &EXT_V5_DISTANCE_DEFAULTS,
                ],
            )
        });
        telemetry(Layout::ExtendedV2, v)
    } else if extended && n == EXT_SIZE_V1 {
        // Older mod half. Decode what it does send and pad the implement block, so a
        // version skew costs the loader features rather than all telemetry.
        let v = unpack(EXT_FORMAT_V1, data).map(|v| {
            let mut v = pad(v, &[&EXT_V1_IMPLEMENT_DEFAULTS]);
            v.extend(v2_wheel_defaults());
            pad(
                v,
                &[
                    &EXT_V3_SLIP_DEFAULTS,
                    &EXT_V4_ELECTRIC_DEFAULTS,
                    &EXT_V5_DISTANCE_DEFAULTS,
                ],
            )
        });
        telemetry(Layout::ExtendedV1, v)
    } else if protocol_mode == "outgauge" && n >= OG_SIZE {
        telemetry(Layout::OutGauge, unpack(OG_FORMAT, &data[..OG_SIZE]))
    } else {
        Decoded::Ignored
    }
}

/// The once-per-session warning an older Extended layout logs, or `None`.
pub fn ext_version_warning(layout: Layout) -> Option<String> {
    let (size, missing) = match layout {
        Layout::ExtendedV5 => (
            EXT_SIZE_V5,
            "trip distance and odometer will be unavailable.",
        ),
        Layout::ExtendedV4 => (
            EXT_SIZE_V4,
            "electric vehicle readouts will be unavailable.",
        ),
        Layout::ExtendedV3 => (
            EXT_SIZE_V3,
            "wheel slip detection will use the slower legacy rule.",
        ),
        Layout::ExtendedV2 => (EXT_SIZE_V2, "centred-wheel telemetry will be unavailable."),
        Layout::ExtendedV1 => (
            EXT_SIZE_V1,
            "implement and centred-wheel telemetry will be unavailable.",
        ),
        Layout::Extended | Layout::OutGauge => return None,
    };
    Some(format!(
        "Extended telemetry packet is {size} bytes, expected {EXT_SIZE}. The Lua mod in \
         bng_mod/ is older than this build; {missing}"
    ))
}

// =========================
//  Gear helpers
// =========================

/// `_report_gear_change` with its three collaborators passed in (the loop's hooks, or the
/// globals for [`report_gear_change`]).
fn report_gear_change_with(
    st: &State,
    phrase: Option<&str>,
    degree: Option<i64>,
    sport: bool,
    say: impl FnOnce(&str, bool),
    drift_quiet: impl FnOnce() -> bool,
    gear_tone: impl FnOnce(i64, bool),
) {
    let mode = st.cfg.gear_report_settings.mode.as_str();
    if mode == "speech" {
        if let Some(phrase) = phrase.filter(|p| !p.is_empty()) {
            // During a drift a shift waits behind the drift speech instead of cutting
            // it off; the first driven run lost bonuses to "second" and "third".
            say(phrase, !drift_quiet());
        }
    } else if mode == "audio" {
        if let Some(degree) = degree {
            gear_tone(degree, sport);
        }
    }
}

/// `_report_gear_change(phrase, degree, sport=False)`. Caller holds `state_lock`.
///
/// Speaks or plays one gear change, per gear_report_mode. The phrases, the scale maths and
/// the tone itself live in `beam_base::gear_report`.
pub fn report_gear_change(st: &State, phrase: Option<&str>, degree: Option<i64>, sport: bool) {
    report_gear_change_with(
        st,
        phrase,
        degree,
        sport,
        |text, interrupt| crate::speech_out::say(text, interrupt, true, "python"),
        || crate::state::drift_announcer().quiet(),
        |degree, sport| {
            if let Some(ac) = crate::state::audio_controller_ref() {
                ac.trigger_gear_tone(degree, sport);
            }
        },
    );
}

// =========================
//  Telemetry loop
// =========================

/// Everything the loop body reaches outside this module. [`LiveHooks`] is the real one.
///
/// The `&State` helpers are here (rather than called directly) only because they read a
/// clock the test needs to script.
pub trait TelemetryHooks {
    /// `with state_lock:`.
    fn lock(&self) -> MutexGuard<'_, State>;
    /// `say(text, interrupt=..., exclude_from_buffer=...)` (source "python").
    fn say(&self, text: &str, interrupt: bool, exclude_from_buffer: bool);
    /// `DRIFT_ANNOUNCER.quiet()`.
    fn drift_quiet(&self) -> bool;
    /// `baseline_frame = _telemetry_baseline_pending; if it: clear it`.
    fn take_telemetry_baseline_pending(&self) -> bool;
    /// `_push_gear_direction(gear)`.
    fn push_gear_direction(&self, st: &mut State, gear: &str);
    /// `_push_obstacle_state()`.
    fn push_obstacle_state(&self, st: &mut State);
    /// `bnvda_debug_enabled()`.
    fn bnvda_debug_enabled(&self) -> bool;
    /// `_travel_bearing_or_none()`.
    fn travel_bearing_or_none(&self, st: &State) -> Option<f64>;
    /// `_route_is_set()`.
    fn route_is_set(&self, st: &State) -> bool;
    /// `_trailer_artic_norm()`.
    fn trailer_artic_norm(&self, st: &State) -> f64;
    /// `logger.<level>(msg)`.
    fn log(&self, level: log::Level, msg: &str);
    /// `audio_controller_ref.trigger_gear_tone(degree, sport)` if there is one
    /// (`_report_gear_change` uses the global, not the loop's controller).
    fn gear_tone(&self, degree: i64, sport: bool);
    // The loop's own `audio_controller`.
    fn trigger_compass_highlight(&self, heading: f64, pitch_mult: f64);
    fn trigger_compass_click(&self, heading: f64, pitch_mult: f64);
    fn trigger_lowspeed_stopped(&self);
    fn update_route_beacon(&self, available: bool, bearing_deg: f64, distance_m: f64);
    fn update_telemetry_state(&self, state: &TelemetryState);
    fn trigger_oil_chime(&self);
}

/// The real collaborators: the global state, `speech_out`, the drift announcer, the E2
/// pushers and the audio controller `_run_engine` passed in.
pub struct LiveHooks {
    pub audio_controller: Arc<AudioController>,
}

impl TelemetryHooks for LiveHooks {
    fn lock(&self) -> MutexGuard<'_, State> {
        crate::state::state()
    }
    fn say(&self, text: &str, interrupt: bool, exclude_from_buffer: bool) {
        crate::speech_out::say(text, interrupt, exclude_from_buffer, "python");
    }
    fn drift_quiet(&self) -> bool {
        crate::state::drift_announcer().quiet()
    }
    fn take_telemetry_baseline_pending(&self) -> bool {
        crate::speech_out::take_telemetry_baseline_pending()
    }
    fn push_gear_direction(&self, st: &mut State, gear: &str) {
        crate::listeners::scanner::push_gear_direction(st, gear);
    }
    fn push_obstacle_state(&self, st: &mut State) {
        crate::listeners::obstacle::push_obstacle_state(st, false, None);
    }
    fn bnvda_debug_enabled(&self) -> bool {
        crate::ui_bridge::bnvda_debug_enabled()
    }
    fn travel_bearing_or_none(&self, st: &State) -> Option<f64> {
        crate::state::travel_bearing_or_none(st)
    }
    fn route_is_set(&self, st: &State) -> bool {
        crate::listeners::route_beacon::route_is_set(st)
    }
    fn trailer_artic_norm(&self, st: &State) -> f64 {
        crate::listeners::trailer::trailer_artic_norm(st)
    }
    fn log(&self, level: log::Level, msg: &str) {
        log::log!(level, "{msg}");
    }
    fn gear_tone(&self, degree: i64, sport: bool) {
        if let Some(ac) = crate::state::audio_controller_ref() {
            ac.trigger_gear_tone(degree, sport);
        }
    }
    fn trigger_compass_highlight(&self, heading: f64, pitch_mult: f64) {
        self.audio_controller
            .trigger_compass_highlight(heading, pitch_mult);
    }
    fn trigger_compass_click(&self, heading: f64, pitch_mult: f64) {
        self.audio_controller
            .trigger_compass_click(heading, pitch_mult);
    }
    fn trigger_lowspeed_stopped(&self) {
        self.audio_controller.trigger_lowspeed_stopped();
    }
    fn update_route_beacon(&self, available: bool, bearing_deg: f64, distance_m: f64) {
        self.audio_controller
            .update_route_beacon(available, bearing_deg, distance_m);
    }
    fn update_telemetry_state(&self, state: &TelemetryState) {
        self.audio_controller.update_telemetry_state(state);
    }
    fn trigger_oil_chime(&self) {
        self.audio_controller.trigger_oil_chime();
    }
}

/// `max(a, b)` as Python evaluates it: `a` unless `b > a` (so a NaN `b` loses).
fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// `min(a, b)` as Python evaluates it: `a` unless `b < a`.
fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `max(0.0, min(1.0, x))`.
fn clamp01(x: f64) -> f64 {
    py_max(0.0, py_min(1.0, x))
}

/// Python's `str(bool)`.
fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// `"%+.2f" % x`.
fn py_signed_f2(x: f64) -> String {
    let s = py_f(x, 2);
    if s.starts_with('-') {
        s
    } else {
        format!("+{s}")
    }
}

/// Python's `x / y` for floats (raises on a zero divisor).
fn py_truediv(x: f64, y: f64) -> f64 {
    if y == 0.0 {
        panic!("ZeroDivisionError: float division by zero");
    }
    x / y
}

/// `(gear or "").strip().upper()`.
fn norm_gear(gear: Option<&str>) -> String {
    py_strip(gear.unwrap_or("")).to_uppercase()
}

/// A value Python would have found to be `None` in arithmetic (`TypeError`).
fn not_none(v: Option<f64>, name: &str) -> f64 {
    v.unwrap_or_else(|| {
        panic!("TypeError: unsupported operand type(s) for -: 'NoneType' and 'float' ({name})")
    })
}

/// The loop body and its locals (`telemetry_loop`'s function-scope variables).
#[derive(Debug, Clone, Default)]
pub struct TelemetryLoop {
    // Drift state (sampled per telemetry packet, low-pass filtered)
    drift_rate_signed: f64,
    drift_below_since: f64,
    prev_drift_sample_ts: f64,
    prev_drift_sample_heading: f64,
    last_lowbeam_on: bool,
    last_highbeam_on: bool,
    last_chime_ts: f64,
}

impl TelemetryLoop {
    /// The loop's prologue: `drift_rate_val = 0.0` (a global) and fresh locals.
    pub fn start(st: &mut State) -> TelemetryLoop {
        st.tel.drift_rate_val = 0.0;
        TelemetryLoop::default()
    }

    /// One received datagram, at `now` (`time.time()` taken before the receive).
    pub fn handle_datagram<H: TelemetryHooks + ?Sized>(&mut self, h: &H, data: &[u8], now: f64) {
        if data.is_empty() {
            return;
        }

        let protocol_mode = {
            let mut st = h.lock();
            st.tel.last_telemetry_ts = now;
            st.tel.protocol_mode.clone()
        };

        match decode_datagram(data, &protocol_mode) {
            Decoded::Ignored => {}
            Decoded::MotionSim(ms) => self.motion_sim(h, &ms, now),
            Decoded::Telemetry { layout, unpacked } => {
                if let Some(msg) = ext_version_warning(layout) {
                    let first = {
                        let mut st = h.lock();
                        !std::mem::replace(&mut st.tel.ext_version_warned, true)
                    };
                    if first {
                        h.log(log::Level::Warn, &msg);
                    }
                }
                self.telemetry(h, layout, &unpacked, data.len(), now);
            }
        }
    }

    fn motion_sim<H: TelemetryHooks + ?Sized>(&mut self, h: &H, ms: &[PyField], now: f64) {
        let f = |i: usize| ms[i].num();
        let (pos_x, pos_y, pos_z) = (f(1), f(2), f(3));
        let (vel_x, vel_y) = (f(4), f(5));
        let (up_z, roll_rad, pitch_rad, yaw_pos) = (f(12), f(13), f(14), f(15));

        let mut guard = h.lock();
        let st = &mut *guard;
        // Ground truth speed: horizontal component only, so driving down a slope doesn't
        // read as extra speed.
        let ground_ms = py_hypot(vel_x, vel_y);
        st.tel.last_ground_speed_ms = ground_ms;
        st.tel.last_vel_x = vel_x;
        st.tel.last_vel_y = vel_y;
        st.tel.ms_last_rx_ts = now;

        // Signed longitudinal acceleration by differentiating ground speed (rather than
        // the packet's vehicle-local accelerometer), so the sign is unambiguous.
        let ms_dt = now - st.tel.ls_ms_prev_ts;
        if st.tel.ls_ms_prev_ts <= 0.0 || ms_dt > LS_ACCEL_MAX_DT {
            // First sample, or a gap in the stream (a vehicle reload halts the vehicle VM
            // for well over a second). Re-baseline instead of differentiating across it.
            st.tel.ls_ms_prev_speed_ms = ground_ms;
            st.tel.ls_ms_prev_ts = now;
            st.tel.ls_accel_smooth = 0.0;
        } else if ms_dt >= LS_ACCEL_MIN_DT {
            let raw_accel = (ground_ms - st.tel.ls_ms_prev_speed_ms) / ms_dt;
            let alpha = 1.0 - (-ms_dt / LS_ACCEL_TAU_S).exp();
            st.tel.ls_accel_smooth += alpha * (raw_accel - st.tel.ls_accel_smooth);
            st.tel.ls_ms_prev_speed_ms = ground_ms;
            st.tel.ls_ms_prev_ts = now;
        }
        // else: gap shorter than LS_ACCEL_MIN_DT, keep accumulating

        st.tel.last_pos_x = pos_x;
        st.tel.last_pos_y = pos_y;
        st.tel.last_pos_z = pos_z;
        st.tel.last_yaw_rad = yaw_pos;
        st.tel.last_roll_rad = roll_rad;
        st.tel.last_pitch_rad = pitch_rad;
        st.tel.last_up_z = up_z;
        let heading = yaw_to_heading_deg(yaw_pos);

        // Drift: sampled every packet (~60 Hz) and low-pass filtered rather than decimated,
        // which turned yaw quantisation into a staircase in the audio.
        if st.tel.drift_mode_active {
            if self.prev_drift_sample_ts == 0.0 {
                self.prev_drift_sample_ts = now;
                self.prev_drift_sample_heading = heading;
            }

            let dt = now - self.prev_drift_sample_ts;
            if dt >= 0.004 {
                // guard against dt ~= 0 blowing up the rate
                let mut d_head = heading - self.prev_drift_sample_heading;
                if d_head > 180.0 {
                    d_head -= 360.0;
                } else if d_head < -180.0 {
                    d_head += 360.0;
                }

                let rate = d_head / dt;
                let beta = 1.0 - (-dt / DRIFT_SMOOTH_TAU).exp();
                self.drift_rate_signed += beta * (rate - self.drift_rate_signed);
                st.tel.drift_rate_val = self.drift_rate_signed.abs();

                // Only update direction if significant drift to avoid noise
                if st.tel.drift_rate_val > DRIFT_PAN_MIN_RATE {
                    st.tel.drift_pan_direction = if self.drift_rate_signed > 0.0 {
                        1.0
                    } else {
                        -1.0
                    };
                }

                self.prev_drift_sample_heading = heading;
                self.prev_drift_sample_ts = now;
            }
        } else {
            st.tel.drift_rate_val = 0.0;
            self.drift_rate_signed = 0.0;
            self.prev_drift_sample_ts = 0.0;
            self.prev_drift_sample_heading = 0.0;
        }

        st.tel.last_heading = heading;

        let mut delta_heading = heading - st.tel.last_click_heading_deg;
        if delta_heading > 180.0 {
            delta_heading -= 360.0;
        }
        if delta_heading < -180.0 {
            delta_heading += 360.0;
        }

        let interval = st.cfg.compass_click_interval_deg;
        if delta_heading.abs() >= interval {
            st.tel.compass_click_counter += 1;
            let pitch_mult = 1.0 + 0.25 * yaw_pos.cos();

            // Check if this click should be a highlight
            if st.cfg.compass_highlight_enabled
                && st.tel.compass_click_counter >= st.cfg.compass_highlight_nth_click
            {
                h.trigger_compass_highlight(heading, pitch_mult * 1.5);
                st.tel.compass_click_counter = 0;
            } else {
                h.trigger_compass_click(heading, pitch_mult);
            }

            // Snap to the nearest interval boundary. `round()` gives a Python int; the
            // product with a float is the float of that int, which is the rounded float.
            let q = py_truediv(heading, interval);
            let num_intervals = if q.is_finite() {
                q.round_ties_even()
            } else {
                py_int_round(q) as f64
            };
            st.tel.last_click_heading_deg = py_mod(num_intervals * interval, 360.0);

            // Announce the compass name when the snapped heading lands on a named
            // direction (a 45 degree multiple), in sync with the click.
            let snapped = st.tel.last_click_heading_deg;
            let compass_match = compass_snap_match(snapped);

            if compass_match != -1 && compass_match != st.tel.last_announced_compass_idx {
                // The drift display mutes the spoken heading, not the clicks; the index
                // still moves on so nothing is said late.
                if (now - st.tel.last_compass_ts) >= st.tel.compass_min_interval && !h.drift_quiet()
                {
                    h.say(COMPASS_NAMES[compass_match as usize], true, true);
                    st.tel.last_compass_ts = now;
                }
                st.tel.last_announced_compass_idx = compass_match;
            } else if compass_match == -1 {
                st.tel.last_announced_compass_idx = -1;
            }
        }

        let current_inverted_state = up_z < -0.6;
        if !st.tel.inverted && current_inverted_state {
            st.tel.inverted = true;
            st.tel.inverted_announced = true;
            h.say("Up side down", true, false);
        } else if st.tel.inverted && !current_inverted_state {
            st.tel.inverted = false;
            st.tel.inverted_announced = false;
        }
    }

    fn telemetry<H: TelemetryHooks + ?Sized>(
        &mut self,
        h: &H,
        layout: Layout,
        u: &[PyField],
        data_len: usize,
        now: f64,
    ) {
        let n = |i: usize| u[i].num();
        let extended = layout != Layout::OutGauge;
        // Every pre-v5 layout predates the engine-temperature presence bit; see
        // TELEMETRY_HAS_ENGINE_TEMP.
        let ext_pre_v5 = data_len != EXT_SIZE && data_len != EXT_SIZE_V5;

        let show_lights: i64;
        let speed_ms: f64;
        let shift_active_frame: bool;
        let tc_active_frame: bool;
        let last_steering: f64;
        let last_actual_steering: f64;
        let last_steering_input: f64;

        {
            let mut guard = h.lock();
            let st = &mut *guard;
            let baseline_frame = h.take_telemetry_baseline_pending();
            let protocol_flags: i64;
            let rpm: f64;
            let turbo: f64;
            let engtemp: f64;
            let fuel: f64;
            let oiltemp: f64;
            let throttle: f64;
            let brake: f64;
            let clutch: f64;
            let steering: f64;
            let actual_steering: f64;
            let steering_input: f64;
            let implement: [f64; 8];
            if extended {
                protocol_flags = u[0].int();
                show_lights = u[13].int();
                speed_ms = n(3);
                rpm = n(4);
                st.tel.last_rpm_max = n(5);
                turbo = n(6);
                st.tel.last_turbo_max = n(7);
                engtemp = n(8);
                fuel = n(9);
                let oil_pressure = n(10);
                oiltemp = n(11);
                let t = |k: usize| n(14 + k);
                throttle = t(0);
                brake = t(1);
                clutch = t(2);
                steering = t(3);
                actual_steering = t(4);
                steering_input = t(5);
                let air_pressure = t(6);
                let air_pressure_max = t(7);
                let (clutch_temp, g_lat, g_lon) = (t(8), t(9), t(10));
                let (tire_p_fl, tire_p_fr, tire_p_rl, tire_p_rr) = (t(11), t(12), t(13), t(14));
                let (tire_t_fl, tire_t_fr, tire_t_rl, tire_t_rr) = (t(15), t(16), t(17), t(18));
                let (brake_t_fl, brake_t_fr, brake_t_rl, brake_t_rr) = (t(19), t(20), t(21), t(22));
                let (sig_left_in, sig_right_in, hazard_en) = (t(23), t(24), t(25));
                let lightbar_raw = &u[14 + 26];
                let fog_raw = &u[14 + 27];
                implement = [t(28), t(29), t(30), t(31), t(32), t(33), t(34), t(35)];
                let (tire_p_f, tire_p_r, tire_t_f, tire_t_r, brake_t_f, brake_t_r) =
                    (n(50), n(51), n(52), n(53), n(54), n(55));
                let mut telemetry_presence = u[56].int();
                st.tel.last_wheel_max_driven = n(57);
                st.tel.last_wheel_min = n(58);
                st.tel.last_motor_power_kw = n(59);
                st.tel.last_motor_torque_nm = n(60);
                st.tel.last_ev_range_m = n(61);
                st.tel.last_trip_m = n(62);
                st.tel.last_odometer_m = n(63);
                st.tel.last_lifetime_m = n(64);
                if ext_pre_v5 {
                    telemetry_presence |= TELEMETRY_HAS_ENGINE_TEMP;
                }
                if data_len != EXT_SIZE && (air_pressure_max > 0.0 || air_pressure > 0.0) {
                    telemetry_presence |= TELEMETRY_HAS_AIR;
                }
                st.tel.last_oil_pressure = oil_pressure;
                st.tel.last_air_pressure = air_pressure;
                st.tel.last_air_pressure_max = air_pressure_max;
                st.tel.last_clutch_temp = clutch_temp;
                st.tel.last_g_lat = g_lat;
                st.tel.last_g_lon = g_lon;
                st.tel.last_tire_pressure_fl = tire_p_fl;
                st.tel.last_tire_pressure_fr = tire_p_fr;
                st.tel.last_tire_pressure_rl = tire_p_rl;
                st.tel.last_tire_pressure_rr = tire_p_rr;
                st.tel.last_tire_temp_fl = tire_t_fl;
                st.tel.last_tire_temp_fr = tire_t_fr;
                st.tel.last_tire_temp_rl = tire_t_rl;
                st.tel.last_tire_temp_rr = tire_t_rr;
                st.tel.last_brake_temp_fl = brake_t_fl;
                st.tel.last_brake_temp_fr = brake_t_fr;
                st.tel.last_brake_temp_rl = brake_t_rl;
                st.tel.last_brake_temp_rr = brake_t_rr;
                st.tel.last_tire_pressure_f = tire_p_f;
                st.tel.last_tire_pressure_r = tire_p_r;
                st.tel.last_tire_temp_f = tire_t_f;
                st.tel.last_tire_temp_r = tire_t_r;
                st.tel.last_brake_temp_f = brake_t_f;
                st.tel.last_brake_temp_r = brake_t_r;
                st.tel.last_telemetry_presence = telemetry_presence;

                // Turn signal / hazard announcements
                let cur_left = sig_left_in > 0.5;
                let cur_right = sig_right_in > 0.5;
                let cur_hazard = hazard_en > 0.5;
                let prev_left = st.tel.last_signal_left_input;
                let prev_right = st.tel.last_signal_right_input;
                let prev_hazard = st.tel.last_hazard_enabled;
                st.tel.last_signal_left_input = cur_left;
                st.tel.last_signal_right_input = cur_right;
                st.tel.last_hazard_enabled = cur_hazard;
                if !baseline_frame
                    && st.cfg.announce_turn_signals
                    && (cur_hazard != prev_hazard
                        || cur_left != prev_left
                        || cur_right != prev_right)
                {
                    if cur_hazard {
                        h.say("Hazards on", true, false);
                    } else if cur_left {
                        h.say("Left turn signal on", true, false);
                    } else if cur_right {
                        h.say("Right turn signal on", true, false);
                    } else {
                        h.say(
                            if prev_hazard {
                                "Hazards off"
                            } else {
                                "Turn signals off"
                            },
                            true,
                            false,
                        );
                    }
                }

                // Lightbar announcements.
                // Departure (python-bugs.md 31): the -1 start value is "never seen", and
                // the first reading is a baseline; the Python spoke "Lightbar off" and
                // "Fog lights off" on it whenever no loading screen had armed one.
                // Departure (python-bugs.md 33): a NaN or infinite reading is skipped;
                // int() raised on it and ended the telemetry thread.
                if let Some(cur_lightbar) = finite_int(lightbar_raw)
                    && cur_lightbar != st.tel.last_lightbar
                {
                    let first = st.tel.last_lightbar == -1;
                    st.tel.last_lightbar = cur_lightbar;
                    if !baseline_frame && !first {
                        match cur_lightbar {
                            0 => h.say("Lightbar off", true, false),
                            1 => h.say("Lightbar on", true, false),
                            2 => h.say("Lightbar and siren on", true, false),
                            _ => {}
                        }
                    }
                }

                // Fog light announcements
                if let Some(cur_fog) = finite_int(fog_raw)
                    && cur_fog != st.tel.last_fog
                {
                    let first = st.tel.last_fog == -1;
                    st.tel.last_fog = cur_fog;
                    if !baseline_frame && !first {
                        h.say(
                            if cur_fog != 0 {
                                "Fog lights on"
                            } else {
                                "Fog lights off"
                            },
                            true,
                            false,
                        );
                    }
                }
            } else {
                // outgauge
                protocol_flags = u[2].int();
                show_lights = u[13].int();
                speed_ms = n(5);
                rpm = n(6);
                turbo = n(7);
                engtemp = n(8);
                fuel = n(9);
                // unpacked[10], the oil pressure, is not stored on this path.
                oiltemp = n(11);
                throttle = n(14);
                brake = n(15);
                clutch = n(16);
                steering = 0.0;
                actual_steering = 0.0;
                steering_input = 0.0;
                // OutGauge carries none of the implement block; the sentinels here are what
                // keep the status metrics hidden and the tones silent in that mode.
                implement = [0.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
                // No wheel extremes either: the slip detector drops to its legacy rule.
                st.tel.last_wheel_max_driven = EXT_V3_SLIP_DEFAULTS[0];
                st.tel.last_wheel_min = EXT_V3_SLIP_DEFAULTS[1];
            }

            st.tel.last_speed_ms = speed_ms;
            st.tel.last_rpm = rpm;
            st.tel.last_fuel = fuel;
            st.tel.last_turbo = turbo;
            st.tel.last_engtemp = engtemp;
            st.tel.last_oiltemp = oiltemp;
            st.tel.last_protocol_flags = protocol_flags;
            st.tel.last_throttle = clamp01(throttle);
            st.tel.last_brake = clamp01(brake);
            st.tel.last_clutch = clamp01(clutch);
            // `last_steering, last_actual_steering, last_steering_input = ...`: the Python
            // loop has no `global` for these three, so the assignment binds LOCALS of
            // telemetry_loop and the module globals stay 0.0 for the session. The loop's own
            // readers below use the locals.
            //
            // **Deliberate difference (python-bugs.md 30, user decision 2026-11-21):** the
            // shared state is written too, as the Python meant, so the obstacle push and the
            // MCP and road-diagnostic snapshots see the real steering instead of 0.
            last_steering = steering;
            last_actual_steering = actual_steering;
            last_steering_input = steering_input;
            st.tel.last_steering = steering;
            st.tel.last_actual_steering = actual_steering;
            st.tel.last_steering_input = steering_input;
            // Departure (python-bugs.md 33): a NaN or infinite flags word is no flags;
            // every later int() of it raised and ended the thread.
            st.tel.last_implement_flags = if implement[0].is_finite() {
                implement[0]
            } else {
                0.0
            };
            st.tel.last_implement_edge_height = implement[1];
            st.tel.last_implement_min_clearance = implement[2];
            st.tel.last_implement_tilt_deg = implement[3];
            st.tel.last_implement_tilt_percent = implement[4];
            st.tel.last_implement_lift = implement[5];
            st.tel.last_implement_activity = implement[6];
            st.tel.last_articulation_deg = implement[7];
            shift_active_frame = (show_lights & DL_SHIFT) != 0;
            tc_active_frame = (show_lights & DL_TC) != 0;
            st.tel.last_tc_active = tc_active_frame;

            if extended {
                let gear_str = decode(u[1].bytes(), Decode::Utf8Ignore)
                    .unwrap_or_default()
                    .trim_matches('\0')
                    .to_string();
                if st.tel.last_gear_str.as_deref() != Some(gear_str.as_str()) {
                    // Suppressed in a cannon: large_cannon's controller writes the shoot
                    // strength into electrics.values.gear as "80%" and rewrites it on every
                    // press of the strength keys. F9+I reads the strength on demand.
                    //
                    // Read directly from the state this block already holds: the lock is
                    // not reentrant, and re-taking it here deadlocks the telemetry thread
                    // while holding it, on the very first packet.
                    let in_cannon = st.dock.cannon_active;
                    let settings = &st.cfg.gear_report_settings;
                    let prev = st.tel.last_gear_str.as_deref();
                    // Sport collapse applies to the tone as well as the speech, so a
                    // silenced S1 -> S2 step skips the report outright.
                    if !baseline_frame
                        && !in_cannon
                        && !gear_report::sport_step_silenced(
                            &gear_str,
                            prev.unwrap_or(""),
                            settings.collapse_sport,
                        )
                    {
                        let phrase = gear_report::extended_change_phrase(
                            &gear_str,
                            prev,
                            settings.collapse_sport,
                            &settings.number_style,
                        );
                        report_gear_change_with(
                            st,
                            phrase.as_deref(),
                            gear_report::extended_gear_degree(&gear_str),
                            gear_report::is_sport(&gear_str),
                            |text, interrupt| h.say(text, interrupt, true),
                            || h.drift_quiet(),
                            |degree, sport| h.gear_tone(degree, sport),
                        );
                    }
                    st.tel.last_gear_str = Some(gear_str.clone());
                    h.push_gear_direction(st, &gear_str);
                }
            } else {
                // outgauge
                let gear_byte = u[3].int();
                if st.tel.last_gear_byte != Some(gear_byte) {
                    if !baseline_frame {
                        let style = &st.cfg.gear_report_settings.number_style;
                        let phrase = gear_report::outgauge_change_phrase(gear_byte, style);
                        report_gear_change_with(
                            st,
                            phrase.as_deref(),
                            gear_report::outgauge_gear_degree(gear_byte),
                            false,
                            |text, interrupt| h.say(text, interrupt, true),
                            || h.drift_quiet(),
                            |degree, sport| h.gear_tone(degree, sport),
                        );
                    }
                    st.tel.last_gear_byte = Some(gear_byte);
                    // OutGauge encodes reverse as byte 0, unlike the extended protocol's
                    // "R" string, so the two paths cannot share a formatter.
                    h.push_gear_direction(st, if gear_byte == REVERSE { "R" } else { "D" });
                }
            }

            // Departure (python-bugs.md 33): a NaN or infinite speed has no bucket; the
            // Python's int() raised on it and ended the telemetry thread.
            let current_bucket = speed_ms.is_finite().then(|| get_speed_bucket(st, speed_ms));
            if let Some(current_bucket) = current_bucket
                && st.tel.last_bucket != Some(current_bucket)
            {
                if !baseline_frame
                    && st.cfg.announce_speed
                    && now - st.tel.last_speed_announce_ts >= st.tel.cooldown_sec
                    && !h.drift_quiet()
                {
                    let (spd_val, spd_unit) = fmt_speed(speed_ms);
                    h.say(&format!("{spd_val} {spd_unit}"), true, true);
                    st.tel.last_speed_announce_ts = now;
                }
                st.tel.last_bucket = Some(current_bucket);
            }
        }

        let mut guard = h.lock();
        let st = &mut *guard;
        h.push_obstacle_state(st);

        // --- Ground truth selection, shared by low speed and slip detection ---
        // MotionSim gives real chassis velocity; the telemetry speed field is
        // drivetrain-derived and collapses to zero under a locked-wheel brake. Fall back to
        // it only when the MotionSim protocol isn't running.
        let ms_fresh = (now - st.tel.ms_last_rx_ts) <= LS_MS_STALE_S;
        let ground_ms;
        let a_long;
        if ms_fresh {
            ground_ms = st.tel.last_ground_speed_ms;
            a_long = st.tel.ls_accel_smooth;
        } else {
            ground_ms = st.tel.last_speed_ms;
            let dt = now - st.tel.ls_prev_speed_ts;
            if st.tel.ls_prev_speed_ts <= 0.0 || dt > LS_ACCEL_MAX_DT {
                st.tel.ls_prev_speed_ms = st.tel.last_speed_ms;
                st.tel.ls_prev_speed_ts = now;
                st.tel.ls_accel_smooth = 0.0;
            } else if dt >= LS_ACCEL_MIN_DT {
                let raw_accel = (st.tel.last_speed_ms - st.tel.ls_prev_speed_ms) / dt;
                let alpha = 1.0 - (-dt / LS_ACCEL_TAU_S).exp();
                st.tel.ls_accel_smooth += alpha * (raw_accel - st.tel.ls_accel_smooth);
                st.tel.ls_prev_speed_ms = st.tel.last_speed_ms;
                st.tel.ls_prev_speed_ts = now;
            }
            a_long = st.tel.ls_accel_smooth;
        }

        // Standstill tracking, with hysteresis so idle jitter can't chatter the flag.
        // ls_was_moving latches once the vehicle has genuinely driven, so a car that is
        // merely parked at startup doesn't chime.
        if ground_ms > LS_STOP_TONE_MIN_MS {
            st.tel.ls_was_moving = true;
        }
        if st.tel.ls_stopped {
            if ground_ms > LS_STOP_EXIT_MS {
                st.tel.ls_stopped = false;
            }
        } else if ground_ms < LS_STOP_ENTER_MS {
            st.tel.ls_stopped = true;
            if st.tel.low_speed_mode_active && st.tel.ls_was_moving {
                h.trigger_lowspeed_stopped();
            }
            st.tel.ls_was_moving = false;
        }

        // Low Speed Detection Logic
        let mut ls_clicks_active = false;
        let mut ls_speed_mph = 0.0;
        let mut ls_accel = 0.0;
        if st.tel.low_speed_mode_active {
            let current_mph = ground_ms * MPH_PER_MS;

            // Steady-speed suppression: go quiet once longitudinal accel has stayed inside
            // the deadband long enough. Keyed off accel rather than a speed window so a
            // braking transient can't reset it spuriously.
            if a_long.abs() > LS_ACCEL_DEADBAND {
                st.tel.ls_steady_start_ts = now;
            }
            let steady_suppressed = (now - st.tel.ls_steady_start_ts) > LS_STEADY_HOLD_S;

            // Gear check: neutral/park suppression
            let mut in_neutral_or_park = false;
            if st.tel.protocol_mode == "outgauge" {
                if st.tel.last_gear_byte == Some(1) {
                    in_neutral_or_park = true;
                }
            } else {
                let gs = norm_gear(st.tel.last_gear_str.as_deref());
                if gs == "N" || gs == "P" {
                    in_neutral_or_park = true;
                }
            }

            if !st.tel.ls_stopped && current_mph < 25.0 && !in_neutral_or_park && !steady_suppressed
            {
                ls_clicks_active = true;
                ls_speed_mph = current_mph;
                ls_accel = a_long;
            }

            if h.bnvda_debug_enabled() && (now - st.tel.ls_diag_last_ts) >= LOWSPEED_DIAG_INTERVAL_S
            {
                st.tel.ls_diag_last_ts = now;
                h.log(
                    log::Level::Info,
                    &format!(
                        "[LOWSPEED] src={} ground={}m/s wheel={}m/s mph={} accel={} clicks={} \
                         stopped={} steady_supp={} gearNP={} ms_age={}s",
                        if ms_fresh {
                            "motionsim"
                        } else {
                            "wheel-fallback"
                        },
                        py_f(ground_ms, 2),
                        py_f(st.tel.last_speed_ms, 2),
                        py_f(current_mph, 1),
                        py_signed_f2(a_long),
                        py_bool(ls_clicks_active),
                        py_bool(st.tel.ls_stopped),
                        py_bool(steady_suppressed),
                        py_bool(in_neutral_or_park),
                        py_f(now - st.tel.ms_last_rx_ts, 2),
                    ),
                );
            }
        }

        // --- Wheel slip (spin / lockup) and side slide ---
        // All of the rule is in wheel_slip. MotionSim is required: without it there is
        // neither a longitudinal speed nor a direction of travel.
        if st.tel.slip_mode_active && ms_fresh {
            let in_reverse = if st.tel.protocol_mode == "outgauge" {
                st.tel.last_gear_byte == Some(REVERSE)
            } else {
                norm_gear(st.tel.last_gear_str.as_deref()).starts_with('R')
            };
            let bearing = h.travel_bearing_or_none(st);
            let (wmax, wmin, wheel) = (
                st.tel.last_wheel_max_driven,
                st.tel.last_wheel_min,
                st.tel.last_speed_ms,
            );
            st.tel.last_slip_result = st.tel.slip_detector.step(
                now,
                ground_ms,
                bearing,
                Some(wmax),
                Some(wmin),
                Some(wheel),
                in_reverse,
            );
        } else {
            st.tel.slip_detector.reset();
            st.tel.last_slip_result = wheel_slip::INACTIVE;
        }
        let slip = st.tel.last_slip_result;
        let slip_active = slip.active;
        let slip_kind = i64::from(slip.kind);
        let slip_mag = if slip_active { slip.magnitude_ms } else { 0.0 };
        st.tel.last_slip_active = slip_active;
        st.tel.last_slip_kind = slip_kind;
        st.tel.last_slip_mag = slip_mag;

        let mut guidance_diff = 0.0;
        if st.tel.heading_guidance_active {
            let mut diff = st.tel.last_heading - st.tel.heading_guidance_target;
            if diff > 180.0 {
                diff -= 360.0;
            } else if diff < -180.0 {
                diff += 360.0;
            }
            guidance_diff = diff;
        }

        if st.tel.coord_guidance_active && st.tel.marked_coord_x.is_some() {
            #[allow(clippy::collapsible_if)]
            if now - st.tel.last_coord_bearing_ts >= 1.0 {
                let mx = not_none(st.tel.marked_coord_x, "marked_coord_x");
                let my = not_none(st.tel.marked_coord_y, "marked_coord_y");
                let dx = mx - st.tel.last_pos_x;
                let dy = my - st.tel.last_pos_y;
                if dx * dx + dy * dy > 0.25 {
                    st.tel.coord_target_bearing =
                        target_bearing(st.tel.last_pos_x, st.tel.last_pos_y, mx, my);
                }
                st.tel.last_coord_bearing_ts = now;
            }
        }

        let mut coord_bearing_error = 0.0;
        if st.tel.coord_guidance_active && st.tel.marked_coord_x.is_some() {
            coord_bearing_error =
                normalize_bearing(st.tel.last_heading - st.tel.coord_target_bearing);
        }

        // The route beacon's bearing is derived HERE, from the position and heading this
        // loop already has at 60 Hz, rather than being sent by the mod. The age-out lives
        // here too, because this is where the value is read.
        if st.route.route_beacon_active && h.route_is_set(st) {
            let dest_x = not_none(st.route.route_dest_x, "route_dest_x");
            let dest_y = not_none(st.route.route_dest_y, "route_dest_y");
            let (rb_dist, rb_bearing) = relative_bearing(
                st.tel.last_pos_x,
                st.tel.last_pos_y,
                st.tel.last_heading,
                dest_x,
                dest_y,
            );
            // Standing on the destination: the bearing to a point you are on is residue
            // and would spin the beacon around the head. Silence is the honest answer.
            h.update_route_beacon(rb_dist > AT_DESTINATION_M, rb_bearing, rb_dist);
        } else if st.route.route_beacon_active {
            h.update_route_beacon(false, 0.0, 0.0);
        }

        // Drift detection (continuous). Start when the rate exceeds DRIFT_ON_RATE with the
        // steering near zero; stop once it stays below DRIFT_OFF_RATE for
        // DRIFT_RELEASE_HOLD. The split thresholds and the hold keep the alert from
        // chattering. Steering input does NOT cancel an active alert.
        if st.tel.drift_mode_active {
            if st.tel.drift_alert_active {
                if st.tel.drift_rate_val < DRIFT_OFF_RATE {
                    if self.drift_below_since == 0.0 {
                        self.drift_below_since = now;
                    } else if now - self.drift_below_since >= DRIFT_RELEASE_HOLD {
                        st.tel.drift_alert_active = false;
                        self.drift_below_since = 0.0;
                    }
                } else {
                    self.drift_below_since = 0.0;
                }
            } else if st.tel.drift_rate_val > DRIFT_ON_RATE && last_steering.abs() < 5.0 {
                st.tel.drift_alert_active = true;
                self.drift_below_since = 0.0;
            }
        } else {
            st.tel.drift_alert_active = false;
            self.drift_below_since = 0.0;
        }

        let t = &st.tel;
        let telemetry_state = TelemetryState {
            shift_active: Some(shift_active_frame),
            tc_active: Some(tc_active_frame),
            pedal_tones_active: Some(t.pedal_tones_active),
            last_clutch: Some(t.last_clutch),
            last_brake: Some(t.last_brake),
            last_throttle: Some(t.last_throttle),
            last_steering: Some(last_steering),
            last_actual_steering: Some(last_actual_steering),
            last_steering_input: Some(last_steering_input),
            // Same scale as last_actual_steering and driving the same tone. Reverse comes
            // from the gear rather than from velocity: at a standstill about to reverse,
            // the instrument should already be at full volume, and velocity reads zero.
            trailer_artic: Some(h.trailer_artic_norm(st)),
            trailer_reverse: Some(norm_gear(t.last_gear_str.as_deref()).starts_with('R')),
            // audio.py takes int() of this on the way in.
            implement_flags: Some(py_int(t.last_implement_flags)),
            implement_min_clearance: Some(t.last_implement_min_clearance),
            implement_tilt_deg: Some(t.last_implement_tilt_deg),
            implement_lift: Some(t.last_implement_lift),
            implement_activity: Some(t.last_implement_activity),
            inverted: Some(t.inverted),
            last_roll_rad: Some(t.last_roll_rad),
            last_pitch_rad: Some(t.last_pitch_rad),
            guidance_active: Some(t.heading_guidance_active),
            guidance_error_deg: Some(guidance_diff),
            drift_alert_active: Some(t.drift_alert_active),
            drift_pan: Some(t.drift_pan_direction),
            drift_rate: Some(t.drift_rate_val),
            ls_clicks_active: Some(ls_clicks_active),
            ls_speed_mph: Some(ls_speed_mph),
            scan_speed_ms: Some(speed_ms),
            ls_accel: Some(ls_accel),
            ls_stopped: Some(t.ls_stopped),
            slip_active: Some(slip_active),
            slip_kind: Some(slip_kind),
            slip_mag: Some(slip_mag),
            slip_ratio: Some(slip.ratio),
            slide_active: Some(slip.slide_active),
            slide_deg: Some(slip.slide_deg),
            slip_azimuth_deg: Some(slip.azimuth_deg),
            coord_guidance_active: Some(t.coord_guidance_active),
            coord_guidance_error_deg: Some(coord_bearing_error),
        };
        h.update_telemetry_state(&telemetry_state);

        let lowbeam_on = (show_lights & DL_LOWBEAM) != 0;
        let highbeam_on = (show_lights & DL_FULLBEAM) != 0;
        let oil_warn_on = (show_lights & DL_OILWARN) != 0;

        if lowbeam_on != self.last_lowbeam_on || highbeam_on != self.last_highbeam_on {
            // Departure (python-bugs.md 32): with both bits set the Python fell through to
            // "Headlights off". The high beam is what is lit then.
            if lowbeam_on && !highbeam_on {
                h.say("Low beams on", true, false);
            } else if highbeam_on {
                h.say("High beams on", true, false);
            } else {
                h.say("Headlights off", true, false);
            }
            self.last_lowbeam_on = lowbeam_on;
            self.last_highbeam_on = highbeam_on;
        }

        if st.cfg.oil_chime_enabled
            && oil_warn_on
            && (now - self.last_chime_ts) >= ALERT_INTERVAL_SEC
        {
            h.trigger_oil_chime();
            self.last_chime_ts = now;
        }
    }
}

/// `telemetry_loop(audio_controller, host="0.0.0.0", port=4444, stop_event=None)`.
///
/// Returns when `stop_event` is set. `Err` is the Python's re-raised `OSError` (a bind
/// failure, or a receive error while not stopping), which ends `_run_engine`.
pub fn telemetry_loop(
    audio_controller: Arc<AudioController>,
    host: &str,
    port: u16,
    stop_event: &Event,
) -> io::Result<()> {
    let hooks = LiveHooks { audio_controller };
    let (mut body, mode) = {
        let mut st = crate::state::state();
        let body = TelemetryLoop::start(&mut st);
        (body, st.tel.protocol_mode.to_uppercase())
    };

    log::info!("Telemetry mode set to: {mode}");
    log::info!("Listening for telemetry (UDP) on {host}:{port} ...");

    let mut sock = UdpListener::bind_on(host, port, 0.25, 2048)?;
    while !stop_event.is_set() {
        let now = time_time();
        match sock.recv() {
            Recv::Timeout => continue,
            Recv::Error(e) => {
                if stop_event.is_set() {
                    break;
                }
                return Err(e);
            }
            Recv::Data(data, _) => body.handle_datagram(&hooks, data, now),
        }
    }
    Ok(())
}
