//! Owner: E3. Implement, dock, ramp and cannon phrasing and their constants
//! (`beamtel.py:1480-2115`). Pure functions over `state::DockReading`, `state::RampSelf`,
//! `state::CannonShot`.
//!
//! Positive is LEFT on every signed axis here (lateral, yaw, bearing), matching the compass
//! clicks and the scanner bearing. `dock_readout_sim` and `cannon_shot_sim` assert both
//! directions, because a sign error reads as perfectly plausible speech.

use beam_audio::consts::DOCK_RAMP_MAX_RANGE_M;
use beam_base::pyfmt::{py_f, py_float_parse, py_is_space, py_mod, py_strip};
use beam_base::pyval::{PyInt, py_int_parse, py_max};

use crate::state::{CannonShot, DockReading, RampSelf};
use crate::units::{FEET_PER_M, fmt_distance, py_int, units_mode};

// Loader implement proximity speech. Distance hysteresis alone is not enough: at the boundary a
// genuine relation flap would still re-announce, hence the per-transition dwell timers.
pub const IMPL_PROX_ENTER_M: f64 = 3.0;
pub const IMPL_PROX_LEAVE_M: f64 = 4.5;
/// Seconds a new relation must persist before it is spoken.
pub const IMPL_PROX_RELATION_HOLD: f64 = 0.40;
pub const IMPL_PROX_INSIDE_HOLD: f64 = 0.25;
pub const IMPL_PROX_LEAVE_HOLD: f64 = 0.30;

// Docking readout thresholds: soft-body jitter moves the cutting edge a couple of centimetres on
// a parked machine, and "left 0.01 meters" invites a correction that cannot be made.
/// Below this an axis is called level / centred rather than numbered.
pub const IMPL_DOCK_LEVEL_M: f64 = 0.05;
/// Squareness is only worth saying once it would jam the tines.
pub const IMPL_DOCK_YAW_DEG: f64 = 8.0;
/// Mirrors IMPL_ENTRY_MIN_DEPTH_M in implementProximity.lua; the gate itself is decided there.
pub const IMPL_DOCK_ENTRY_MIN_M: f64 = 0.40;
/// ...and how far back through it must fall to be lost again.
pub const IMPL_DOCK_ENTRY_EXIT_M: f64 = 0.34;
/// Keep asking the mod which implement is fitted until it answers.
pub const IMPL_ANNOUNCE_PROBE_S: f64 = 2.0;

// Ramp-mode readout thresholds: a car in a four-metre trough, not tines in a pallet pocket.
pub const RAMP_DOCK_CENTRE_M: f64 = 0.15;
pub const RAMP_DOCK_SQUARE_DEG: f64 = 3.0;
pub const RAMP_DOCK_HEADING_ZERO_DEG: f64 = 0.5;
pub const RAMP_DOCK_TIGHT_M: f64 = 0.25;
pub const RAMP_DOCK_PITCH_DEG: f64 = 5.0;
/// The deck readout: under this the ramp is called level.
pub const RAMP_SELF_LEVEL_DEG: f64 = 1.0;
/// Above this stroke a hydraulic group reads as a distance ("14 of 18 feet"), below it as a
/// percentage. A property of the ram, not of the machine.
pub const RAMP_SELF_DISTANCE_STROKE_M: f64 = 1.0;
/// Ramp align width margin: at or below, it will not fit.
pub const RAMP_ALIGN_TIGHT_M: f64 = 0.0;
/// ...above that but under this, it fits but barely.
pub const RAMP_ALIGN_SNUG_M: f64 = 0.10;
/// Lip height above which a ramp is a wall. Must match the mod's RAMP_ALIGN_LIP_SAY_M.
pub const RAMP_ALIGN_LIP_SAY_M: f64 = 0.30;
pub const CANNON_READOUT_STALE_S: f64 = 1.5;

// The approach corridor decides which QUESTION the ramp instrument answers: outside it, where
// the mouth is; inside it, the steering correction. Hysteretic, because it swaps what the pan
// means.
pub const RAMP_CORRIDOR_ENTER_M: f64 = 6.0;
pub const RAMP_CORRIDOR_EXIT_M: f64 = 9.0;
/// Behind the mouth plane (or within this of it) there is no corridor.
pub const RAMP_CORRIDOR_MIN_RANGE_M: f64 = 0.5;

// Cannon shot outcome: two numbers and then only what is notable.
/// At or under this the shot went straight; no lateral number.
pub const CANNON_SHOT_CENTRE_M: f64 = 3.0;
/// Apex earns a clause only when it is the interesting thing about the shot.
pub const CANNON_SHOT_APEX_SAY_M: f64 = 15.0;
/// Below this two shots went the same distance.
pub const CANNON_SHOT_COMPARE_M: f64 = 5.0;

/// `_SLAM_CUES`: slam gate state -> the audio cue. NONE is deliberately absent: leaving the
/// gate is not an event.
pub const SLAM_CUES: [(&str, &str); 3] = [
    ("CLEAR", "clear"),
    ("OVER", "over"),
    ("COMMITTED", "committed"),
];

/// `_IMPL_RELATION_PHRASE`.
pub const IMPL_RELATION_PHRASE: [(&str, &str); 3] = [
    ("ABOVE", "above it"),
    ("BELOW", "below it"),
    ("LEVEL", "level with it"),
];

/// `_SLAM_CUES.get(state)`.
pub fn slam_cue(state: &str) -> Option<&'static str> {
    SLAM_CUES.iter().find(|(k, _)| *k == state).map(|(_, v)| *v)
}

/// `_IMPL_RELATION_PHRASE.get(relation)`.
pub fn impl_relation_phrase(relation: &str) -> Option<&'static str> {
    IMPL_RELATION_PHRASE
        .iter()
        .find(|(k, _)| *k == relation)
        .map(|(_, v)| *v)
}

/// `int(text)` for a wire field, as an `i64`. `None` where Python raises, and also for an
/// integer outside `i64` (Python would accept it).
pub(crate) fn wire_int(text: &str) -> Option<i64> {
    match py_int_parse(text)? {
        PyInt::Small(v) => Some(v),
        PyInt::Big(_) => None,
    }
}

/// `_implement_word(part_name)`: what to call the business end.
pub fn implement_word(part_name: &str) -> &'static str {
    let low = part_name.to_lowercase();
    if low.contains("fork") {
        return "Forks";
    }
    if low.contains("grapple") {
        return "Grapple";
    }
    "Bucket"
}

/// `_band_name(kind, idx, count)`: named by position in the stack, not by world Z.
pub fn band_name(kind: &str, idx: i64, count: i64) -> String {
    let name = if kind == "GAP" {
        if idx <= 1 { "underside" } else { "opening" }
    } else if idx >= count {
        "roof"
    } else if idx <= 1 {
        "base"
    } else {
        "body"
    };
    name.to_string()
}

/// `_ramp_bearing_deg(range, lateral, yaw)`: bearing to the ramp mouth from the driver's
/// heading, positive-LEFT, wrapped to [-180, 180). `atan2(lateral, range) + yaw` exactly.
pub fn ramp_bearing_deg(range_m: f64, lateral_m: f64, yaw_deg: f64) -> f64 {
    let beta = lateral_m.atan2(range_m).to_degrees() + yaw_deg;
    py_mod(beta + 180.0, 360.0) - 180.0
}

/// `_ramp_acquire(range, lateral, prev)`: still hunting for the mouth (true), or lined up on
/// the approach (false). One authority for both the tones and the spoken readout.
pub fn ramp_acquire(range_m: f64, lateral_m: f64, prev: bool) -> bool {
    if range_m < RAMP_CORRIDOR_MIN_RANGE_M {
        return true;
    }
    let limit = if !prev {
        RAMP_CORRIDOR_EXIT_M
    } else {
        RAMP_CORRIDOR_ENTER_M
    };
    lateral_m.abs() > limit
}

/// `fmt_distance(x)` as the two strings an f-string would produce.
fn dist(metres: f64) -> (String, &'static str) {
    let (v, u) = fmt_distance(metres);
    (v.to_string(), u)
}

/// `_ramp_align_phrase(payload)`: the outcome of a ramp align teleport. FAIL is spoken
/// verbatim; an unknown payload is "".
pub fn ramp_align_phrase(payload: &str) -> String {
    if let Some(rest) = payload.strip_prefix("FAIL,") {
        return py_strip(rest).to_string();
    }
    let Some(rest) = payload.strip_prefix("OK,") else {
        return String::new();
    };
    let bits: Vec<&str> = rest.split(',').collect();
    let target = match py_strip(bits[0]) {
        "" => "ramp",
        t => t,
    };
    let num = |i: usize| bits.get(i).and_then(|b| py_float_parse(b));
    let (gap, margin, lip) = (num(1), num(2), num(3));
    let mut phrase = match gap {
        None => format!("Aligned to {target}"),
        Some(gap) => {
            let (val, unit) = dist(gap);
            format!("Aligned to {target}, {val} {unit} back")
        }
    };
    // The lip first: a ramp you cannot get onto makes the width margin beside the point. A
    // missing field is silence, not zero (zero reads as "lip on the ground").
    if let Some(lip) = lip.filter(|l| *l >= RAMP_ALIGN_LIP_SAY_M) {
        let (lv, lu) = dist(lip);
        phrase.push_str(&format!(", ramp not down, lip {lv} {lu} up"));
    }
    // NA parses to None and is silence, never a clearance of zero. Only bad news is added.
    if let Some(margin) = margin {
        if margin <= RAMP_ALIGN_TIGHT_M {
            phrase.push_str(", you do not fit");
        } else if margin < RAMP_ALIGN_SNUG_M {
            phrase.push_str(", tight");
        }
    }
    phrase
}

/// `_cannon_shot_long_distance(metres)`: whole numbers, not implement precision.
pub fn cannon_shot_long_distance(metres: f64) -> String {
    // Departure (python-bugs.md 46): the Python tested == "imperial" here while
    // fmt_distance tests == "metric", so any other value mixed units in one utterance.
    // Both now treat everything but "metric" as imperial.
    if units_mode() != "metric" {
        return format!("{} feet", py_f(metres * FEET_PER_M, 0));
    }
    format!("{} meters", py_f(metres, 0))
}

/// `_cannon_shot_phrase(shot)`: two numbers, then only what is notable. Nothing here
/// confirms that a shot went well.
pub fn cannon_shot_phrase(shot: &CannonShot) -> String {
    // A shot that never came to rest has no distance, and must not be given one.
    if !shot.settled {
        return "Shot did not settle".to_string();
    }
    let mut bits: Vec<String> = Vec::new();
    let downrange = shot.downrange;
    let lateral = shot.lateral;

    let d = cannon_shot_long_distance(downrange.abs());
    if downrange < 0.0 {
        bits.push(format!("{d} backwards"));
    } else if lateral.abs() <= CANNON_SHOT_CENTRE_M {
        bits.push(format!("{d}, straight"));
    } else {
        let side = if lateral > 0.0 { "left" } else { "right" };
        bits.push(format!(
            "{d}, {} {side}",
            cannon_shot_long_distance(lateral.abs())
        ));
    }

    if shot.apex >= CANNON_SHOT_APEX_SAY_M {
        bits.push(format!(
            "peaked at {}",
            cannon_shot_long_distance(shot.apex)
        ));
    }

    // Silence means open ground.
    if !shot.near_name.is_empty() && shot.near_dist >= 0.0 {
        bits.push(format!("next to {}", shot.near_name));
    }

    if let Some(prev) = shot.prev_downrange {
        let delta = downrange - prev;
        if delta.abs() >= CANNON_SHOT_COMPARE_M {
            let word = if delta > 0.0 { "further" } else { "shorter" };
            bits.push(format!("{} {word}", cannon_shot_long_distance(delta.abs())));
        }
    }

    bits.join(". ")
}

/// `_dock_phrase(dock)`: the F9+I cane tap, dispatched on the mode the mod reported. The two
/// readouts share no wording, which keeps the implement phrasing provably unchanged.
pub fn dock_phrase(dock: &DockReading) -> String {
    if dock.mode == "RAMP" {
        return dock_phrase_ramp(dock);
    }
    dock_phrase_impl(dock)
}

/// `_dock_phrase_ramp(dock)`: compact geometry, and then only what is wrong. The target is
/// named once, on acquisition, by the listener; a tap is a question about geometry.
pub fn dock_phrase_ramp(dock: &DockReading) -> String {
    let mut bits: Vec<String> = Vec::new();
    let rng = dock.range;
    let lat = dock.lateral;
    let yaw = dock.yaw;

    if dock.acquire {
        // Hunting: the mouth is a place to get to, named by which way to turn and how far.
        let (dv, du) = dist(rng.hypot(lat));
        let bearing = ramp_bearing_deg(rng, lat, yaw);
        if bearing.abs() < RAMP_DOCK_SQUARE_DEG {
            bits.push(format!("ahead, {dv} {du}"));
        } else {
            bits.push(format!(
                "{} {}, {dv} {du}",
                py_f(bearing.abs(), 0),
                if bearing > 0.0 { "left" } else { "right" }
            ));
        }
        // "wrong side", never "behind": this clause is in the mouth-plane frame and the
        // bearing above is in the driver's, and both can be true at once. Fired on the sign of
        // the range, not the corridor threshold.
        if rng <= -RAMP_CORRIDOR_MIN_RANGE_M {
            bits.push("wrong side".to_string());
        }
    } else {
        // On the approach. Lateral first: it is the axis carried by pulse position.
        let (rv, ru) = dist(py_max(0.0, rng));
        if lat.abs() <= RAMP_DOCK_CENTRE_M {
            bits.push(format!("centred, {rv} {ru}"));
        } else {
            let (lv, _lu) = dist(lat.abs());
            bits.push(format!(
                "{lv} {}, {rv} {ru}",
                if lat > 0.0 { "left" } else { "right" }
            ));
        }
        // Heading is always explicit on the approach because it is the beat pair's null.
        if yaw.abs() < RAMP_DOCK_HEADING_ZERO_DEG {
            bits.push("heading zero degrees".to_string());
        } else {
            let whole = py_int((yaw.abs() + 0.5).floor());
            let heading = if whole > 1 { whole } else { 1 };
            bits.push(format!(
                "heading {heading} degrees {}",
                if yaw > 0.0 { "left" } else { "right" }
            ));
        }
    }

    // Fit between the walls, bad news only. A negative sentinel is "not measured" (silence);
    // held back while hunting, where the margin is about where you happen to be standing.
    let margin = if dock.acquire { None } else { dock.margin };
    if let Some(margin) = margin.filter(|m| *m > -0.999) {
        if margin <= 0.0 {
            let (mv, mu) = dist(margin.abs());
            bits.push(format!("too narrow by {mv} {mu}"));
        } else if margin < RAMP_DOCK_TIGHT_M {
            let (mv, mu) = dist(margin);
            bits.push(format!("tight, {mv} {mu} each side"));
        }
    }

    // The ramp's own inclination, once steep enough to matter. -999 is "could not measure".
    let pitch = dock.entry_theta.filter(|p| p.abs() <= 900.0 || p.is_nan());
    if let Some(p) = pitch.filter(|p| p.abs() >= RAMP_DOCK_PITCH_DEG) {
        bits.push(format!(
            "ramp {} {} degrees",
            if p > 0.0 { "up" } else { "down" },
            py_f(p.abs(), 0)
        ));
    }

    // The feed runs past the tone range on purpose; unexplained silence is named.
    if rng.hypot(lat) >= DOCK_RAMP_MAX_RANGE_M {
        bits.push("too far for tones".to_string());
    }

    bits.join(". ")
}

/// `_parse_ramp_self(payload)`: the `RAMPSELF:` body -> the deck state of the machine you
/// are sitting in, or None (`NONE`, empty, or an unparseable head). Wire form
/// `<pitchDeg>[,<lipM>];<name>:<pct>:<strokeCm>;...`.
pub fn parse_ramp_self(payload: &str) -> Option<RampSelf> {
    let payload = py_strip(payload);
    if payload.is_empty() || payload.to_uppercase() == "NONE" {
        return None;
    }
    let fields: Vec<&str> = payload.split(';').collect();
    let head: Vec<&str> = fields[0].split(',').collect();
    let pitch = py_float_parse(head[0])?;
    let lip = head.get(1).and_then(|h| py_float_parse(h));
    let mut groups = Vec::new();
    for chunk in &fields[1..] {
        if chunk.is_empty() {
            continue;
        }
        let bits: Vec<&str> = chunk.split(':').collect();
        if bits.len() != 3 {
            continue;
        }
        let (Some(pct), Some(stroke)) = (wire_int(bits[1]), wire_int(bits[2])) else {
            continue;
        };
        groups.push((bits[0].to_string(), pct, stroke as f64 / 100.0));
    }
    // -999 is the mod's "could not measure" sentinel for both figures.
    Some(RampSelf {
        pitch: if pitch < -180.0 { None } else { Some(pitch) },
        lip: match lip {
            Some(l) if l >= -900.0 || l.is_nan() => Some(l),
            _ => None,
        },
        groups,
    })
}

/// `" ".join(s.split())` with Python's whitespace rules.
fn collapse_ws(s: &str) -> String {
    s.split(py_is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `_ramp_hyd_words(name)`: underscores and camelCase humps become spaces.
pub fn ramp_hyd_words(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '_' {
            out.push(' ');
        } else if ch.is_uppercase() && i > 0 && !chars[i - 1].is_uppercase() && chars[i - 1] != '_'
        {
            out.push(' ');
            out.extend(ch.to_lowercase());
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    collapse_ws(&out)
}

/// `_ramp_hyd_strip_namespace(names)`: drop a leading segment every group shares. Needs two
/// or more groups: a prefix is only identifiable as a namespace because several names share it.
pub fn ramp_hyd_strip_namespace(names: &[String]) -> Vec<String> {
    if names.len() < 2 {
        return names.to_vec();
    }
    let head = names[0].split('_').next().unwrap_or("");
    if names
        .iter()
        .any(|n| n.split('_').next().unwrap_or("") != head)
    {
        return names.to_vec();
    }
    let skip = head.chars().count() + 1;
    let stripped: Vec<String> = names
        .iter()
        .map(|n| n.chars().skip(skip).collect())
        .collect();
    // A group whose entire name IS the shared prefix has nothing left to be called.
    if stripped.iter().any(|s| s.is_empty()) {
        return names.to_vec();
    }
    stripped
}

/// `_ramp_self_phrase(state)`: the deck readout. Unconditional, unlike the ramp tap: these
/// numbers ARE the question being asked.
pub fn ramp_self_phrase(state: &RampSelf) -> String {
    let mut bits: Vec<String> = Vec::new();
    match state.pitch {
        None => bits.push("Ramp angle unavailable".to_string()),
        Some(p) if p.abs() < RAMP_SELF_LEVEL_DEG => bits.push("Ramp level".to_string()),
        Some(p) => bits.push(format!(
            "Ramp {} {} degrees",
            if p > 0.0 { "up" } else { "down" },
            py_f(p.abs(), 0)
        )),
    }

    // Stated in both directions: this readout is solicited.
    if let Some(lip) = state.lip {
        if lip < RAMP_ALIGN_LIP_SAY_M {
            bits.push("Lip on the ground".to_string());
        } else {
            let (lv, lu) = dist(lip);
            bits.push(format!("Lip {lv} {lu} up"));
        }
    }

    let names: Vec<String> = state.groups.iter().map(|g| g.0.clone()).collect();
    let labels = ramp_hyd_strip_namespace(&names);
    for ((_name, pct, stroke), label) in state.groups.iter().zip(labels.iter()) {
        let words = ramp_hyd_words(label);
        if *stroke >= RAMP_SELF_DISTANCE_STROKE_M {
            // One figure per group, the unit once.
            let (out_v, out_u) = dist(stroke * *pct as f64 / 100.0);
            let (full_v, _full_u) = dist(*stroke);
            bits.push(format!("{words} {out_v} of {full_v} {out_u}"));
        } else {
            bits.push(format!("{words} {pct} percent"));
        }
    }
    bits.join(". ")
}

/// `_dock_phrase_impl(dock)`: the whole docking picture in one utterance.
pub fn dock_phrase_impl(dock: &DockReading) -> String {
    let band = band_name(&dock.kind, dock.idx, dock.count);
    let thick = dock.hi_z - dock.lo_z;
    let mut bits = vec![format!("{}, {}", dock.impl_word, dock.name)];

    let mut reference = format!("{band} {} of {}", dock.idx, dock.count);
    if dock.manual {
        reference.push_str(", held");
    }
    let (tv, tu) = dist(py_max(0.0, thick));
    bits.push(format!("{reference}, {tv} {tu} tall"));

    let vert = dock.vertical;
    if vert.abs() < IMPL_DOCK_LEVEL_M {
        bits.push("level".to_string());
    } else {
        let (vv, vu) = dist(vert.abs());
        bits.push(format!(
            "{} {vv} {vu}",
            if vert > 0.0 { "raise" } else { "lower" }
        ));
    }

    let lat = dock.lateral;
    if lat.abs() < IMPL_DOCK_LEVEL_M {
        bits.push("centred".to_string());
    } else {
        let (lv, lu) = dist(lat.abs());
        bits.push(format!(
            "{} {lv} {lu}",
            if lat > 0.0 { "left" } else { "right" }
        ));
    }

    let (rv, ru) = dist(py_max(0.0, dock.range));
    bits.push(format!("range {rv} {ru}"));

    // Squareness only when it would jam the tines.
    let yaw = dock.yaw;
    if yaw.abs() >= IMPL_DOCK_YAW_DEG {
        bits.push(format!(
            "face {} degrees {}",
            py_f(yaw.abs(), 0),
            if yaw > 0.0 { "left" } else { "right" }
        ));
    }

    // The entry gate, bad news only. A negative depth is "could not measure": silence.
    if let Some(depth) = dock
        .entry_depth
        .filter(|d| *d >= 0.0 && *d < IMPL_DOCK_ENTRY_MIN_M)
    {
        let (dv, du) = dist(py_max(0.0, depth));
        bits.push(format!("tines enter {dv} {du}, too steep"));
    }

    bits.join(". ")
}
