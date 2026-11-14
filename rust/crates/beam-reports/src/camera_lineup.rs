//! Free-camera line-up guide: geometry, tone mapping and wording.
//!
//! Pure functions only. The sockets, key layer and state live in the engine (the
//! `_lineup_*` block); the synthesis is the audio crate's line-up voice. See
//! docs/camera-lineup.md.
//!
//! The one test both modes share: does the ray from the front centre of the car, along
//! its heading, pass through a target point P?
//!
//! ```text
//!   head-on   P = camera + camera_left * offset   (offset positive = car passes the camera's LEFT)
//!   crossing  P = camera + camera_forward * depth (the car's path crosses the view at that depth)
//! ```
//!
//! The error is the signed sideways miss of the car's path at P, in the CAR's frame,
//! positive LEFT (lateral is up x forward, built from the un-negated forward). A path
//! missing to the left means the car is left of the line: the FM ratio dips below 1.0
//! and the sound sits on the left. A target behind the nose has no miss at all and reads
//! as full error from behind.

use beam_base::pyfmt::{py_mod, py_strip};

use crate::pyops::{PyErr, py_hypot, py_max, py_min, py_round_to_int, py_str_to_float};

pub const HEAD_ON: &str = "head_on";
pub const CROSSING: &str = "crossing";

/// Head-on: pass 3 m to the camera's left.
pub const DEFAULT_OFFSET_M: f64 = 3.0;
/// Crossing: cross the view 10 m out.
pub const DEFAULT_DEPTH_M: f64 = 10.0;
pub const STEP_M: f64 = 1.0;
pub const OFFSET_LIMIT_M: f64 = 50.0;
pub const DEPTH_MIN_M: f64 = 1.0;
pub const DEPTH_MAX_M: f64 = 200.0;

/// Silent within this miss.
pub const DEADZONE_M: f64 = 0.5;
/// Full volume and grit at this miss.
pub const FULL_M: f64 = 8.0;
pub const INDEX_MIN: f64 = 1.0;
pub const INDEX_MAX: f64 = 1.3;
/// Ratio 0.92 at full left, 1.08 at full right.
pub const RATIO_DRIFT: f64 = 0.08;
pub const CARRIER_HZ: f64 = 440.0;
/// How far round the head the sound swings at full error.
pub const SIDE_AZIMUTH_DEG: f64 = 90.0;
pub const BEHIND_AZIMUTH_DEG: f64 = 180.0;

pub const FEET_PER_M: f64 = 3.28084;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub cam_x: f64,
    pub cam_y: f64,
    pub cam_fx: f64,
    pub cam_fy: f64,
    pub nose_x: f64,
    pub nose_y: f64,
    pub veh_fx: f64,
    pub veh_fy: f64,
}

/// Audio parameters. `amp` is 0..1 of the guide's peak level; `az` is HRTF degrees
/// (0 front, 90 left, counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    pub amp: f64,
    pub index: f64,
    pub ratio: f64,
    pub az: f64,
}

pub const SILENT: Tone = Tone {
    amp: 0.0,
    index: INDEX_MIN,
    ratio: 1.0,
    az: 0.0,
};

/// A parsed LINEUP datagram: Python's `"OFF"` or a `Geometry`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineupPacket {
    Off,
    Geometry(Geometry),
}

/// `Off` for LINEUP:OFF, a geometry for a live packet, `None` for anything else.
pub fn parse_packet(text: &str) -> Option<LineupPacket> {
    let text = py_strip(text);
    let body = text.strip_prefix("LINEUP:")?;
    if body == "OFF" {
        return Some(LineupPacket::Off);
    }
    let parts: Vec<&str> = body.split(',').collect();
    if parts.len() < 8 {
        return None;
    }
    let mut vals = [0.0f64; 8];
    for (slot, p) in vals.iter_mut().zip(&parts[..8]) {
        *slot = py_str_to_float(p).ok()?;
    }
    if !vals.iter().all(|v| v.is_finite()) {
        return None;
    }
    let [cam_x, cam_y, cam_fx, cam_fy, nose_x, nose_y, veh_fx, veh_fy] = vals;
    Some(LineupPacket::Geometry(Geometry {
        cam_x,
        cam_y,
        cam_fx,
        cam_fy,
        nose_x,
        nose_y,
        veh_fx,
        veh_fy,
    }))
}

fn unit(x: f64, y: f64) -> Option<(f64, f64)> {
    let n = py_hypot(x, y);
    if n < 1e-6 {
        return None;
    }
    Some((x / n, y / n))
}

pub fn target_point(geom: &Geometry, mode: &str, offset_m: f64, depth_m: f64) -> Option<(f64, f64)> {
    let cf = unit(geom.cam_fx, geom.cam_fy)?;
    if mode == CROSSING {
        return Some((geom.cam_x + cf.0 * depth_m, geom.cam_y + cf.1 * depth_m));
    }
    let left = (-cf.1, cf.0); // up x forward, positive LEFT
    Some((geom.cam_x + left.0 * offset_m, geom.cam_y + left.1 * offset_m))
}

pub const STATUS_OK: &str = "ok";
pub const STATUS_BEHIND: &str = "behind";
pub const STATUS_UNKNOWN: &str = "unknown";

/// `(status, miss, bearing)`.
///
/// * status: `"ok"`, `"behind"` (target not ahead of the nose) or `"unknown"`
///   (degenerate frame)
/// * miss: signed perpendicular miss in metres, positive = path passes LEFT of P;
///   `None` unless `"ok"`
/// * bearing: signed angle from the car's heading to P in degrees, positive = P to the
///   LEFT, in (-180, 180]; `None` only when `"unknown"`
pub fn solve(
    geom: &Geometry,
    mode: &str,
    offset_m: f64,
    depth_m: f64,
) -> (&'static str, Option<f64>, Option<f64>) {
    let p = target_point(geom, mode, offset_m, depth_m);
    let vf = unit(geom.veh_fx, geom.veh_fy);
    let (Some(p), Some(vf)) = (p, vf) else {
        return (STATUS_UNKNOWN, None, None);
    };
    let vl = (-vf.1, vf.0);
    let (dx, dy) = (p.0 - geom.nose_x, p.1 - geom.nose_y);
    let along = dx * vf.0 + dy * vf.1;
    let lat = dx * vl.0 + dy * vl.1; // how far LEFT of the car's path the target is
    let bearing = lat.atan2(along).to_degrees();
    if along <= 0.0 {
        return (STATUS_BEHIND, None, Some(bearing));
    }
    // Target to the car's left means the path passes to the target's RIGHT.
    (STATUS_OK, Some(-lat), Some(bearing))
}

/// `(status, miss)` -- [`solve`] without the bearing.
pub fn miss_left_m(
    geom: &Geometry,
    mode: &str,
    offset_m: f64,
    depth_m: f64,
) -> (&'static str, Option<f64>) {
    let (status, miss, _bearing) = solve(geom, mode, offset_m, depth_m);
    (status, miss)
}

/// Loudness and grit come from the miss in metres; position comes from the bearing.
///
/// Position is the bearing to the target MIRRORED onto the side of the error: a target
/// 20 degrees to the right means the path misses left, so the sound sits 20 degrees
/// left. That keeps it continuous all the way round and moves with the steering angle
/// directly, where a position taken from the metres jumps.
pub fn tone_for(status: &str, miss: Option<f64>, bearing: Option<f64>) -> Tone {
    if status == STATUS_BEHIND {
        let Some(bearing) = bearing else {
            return Tone {
                amp: 1.0,
                index: INDEX_MAX,
                ratio: 1.0,
                az: BEHIND_AZIMUTH_DEG,
            };
        };
        let side = if bearing > 0.0 { -1.0 } else { 1.0 }; // error side, +1 left
        return Tone {
            amp: 1.0,
            index: INDEX_MAX,
            ratio: ratio_for(side, bearing),
            az: py_mod(-bearing, 360.0),
        };
    }
    let miss = match miss {
        Some(m) if status == STATUS_OK => m,
        _ => return SILENT,
    };
    let t = (miss.abs() - DEADZONE_M) / (FULL_M - DEADZONE_M);
    let t = py_max(0.0, py_min(1.0, t));
    if t <= 0.0 {
        return SILENT;
    }
    let side = if miss > 0.0 { 1.0 } else { -1.0 }; // +1 left
    let (az, ratio) = match bearing {
        None => (
            py_mod(side * SIDE_AZIMUTH_DEG * t, 360.0),
            1.0 - RATIO_DRIFT * t * side,
        ),
        Some(b) => (py_mod(-b, 360.0), ratio_for(side, b)),
    };
    Tone {
        amp: t,
        index: INDEX_MIN + (INDEX_MAX - INDEX_MIN) * t,
        ratio,
        az,
    }
}

/// The ratio drifts with the ANGLE, not the metres: 1.0 lined up, the full drift only at
/// 180 degrees. Tied to the metres it saturated at FULL_M, a few degrees of turn far
/// out, so most of the turn carried no change at all.
fn ratio_for(side: f64, bearing: f64) -> f64 {
    1.0 - RATIO_DRIFT * side * py_min(1.0, bearing.abs() / 180.0)
}

/// The new `(offset_m, depth_m)` after `delta` steps.
pub fn step_value(mode: &str, offset_m: f64, depth_m: f64, delta: i64) -> (f64, f64) {
    let step = delta as f64 * STEP_M;
    if mode == CROSSING {
        let depth = py_max(DEPTH_MIN_M, py_min(DEPTH_MAX_M, depth_m + step));
        (offset_m, depth)
    } else {
        let offset = py_max(-OFFSET_LIMIT_M, py_min(OFFSET_LIMIT_M, offset_m + step));
        (offset, depth_m)
    }
}

/// `Err` where Python's `round()` raises (a NaN or infinite distance).
fn dist(m: f64, units: &str) -> Result<String, PyErr> {
    if units == "imperial" {
        let v = py_round_to_int(m.abs() * FEET_PER_M)?;
        return Ok(format!("{v} {}", if v == "1" { "foot" } else { "feet" }));
    }
    let v = py_round_to_int(m.abs())?;
    Ok(format!("{v} {}", if v == "1" { "meter" } else { "meters" }))
}

/// `Err` only for a NaN or infinite value, where Python's `round()` raises.
pub fn value_phrase(mode: &str, offset_m: f64, depth_m: f64, units: &str) -> Result<String, PyErr> {
    if mode == CROSSING {
        return Ok(format!("{} out", dist(depth_m, units)?));
    }
    if offset_m.abs() < 0.05 {
        return Ok("through the camera".to_string());
    }
    Ok(format!(
        "{} {}",
        dist(offset_m, units)?,
        if offset_m > 0.0 { "left" } else { "right" }
    ))
}

pub fn mode_name(mode: &str) -> &'static str {
    if mode == CROSSING {
        "Crossing"
    } else {
        "Head-on"
    }
}

/// `Err` as [`value_phrase`].
pub fn target_phrase(mode: &str, offset_m: f64, depth_m: f64, units: &str) -> Result<String, PyErr> {
    let mut v = value_phrase(mode, offset_m, depth_m, units)?;
    if mode == HEAD_ON && offset_m.abs() >= 0.05 {
        v.push_str(" of the camera");
    }
    Ok(format!("{}, {v}", mode_name(mode)))
}
