//! Capture and summarise road-guidance sessions for live driving diagnosis, from
//! road_diagnostics.py.
//!
//! Sessions are NDJSON under `%LOCALAPPDATA%\beamtel\road_diagnostics`, written
//! compact (`separators=(",", ":")`, ASCII-escaped, NaN refused) with Windows
//! text-mode line endings, plus a `.summary.json` (indent 2) beside each one, so
//! either build can review the other's sessions.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use beam_base::pyfmt::{py_round, py_strip};
use beam_reports::pyops::{PyDict, PyValue, py_hypot};
use beam_reports::wheel_slip;
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::pyfs::{self, CrlfWriter, glob_sorted, path_str, with_suffix};
use crate::pyjson::{
    self, Dict, DumpOpts, PyErr, Value, dict, finite, or_empty_dict, py_max, py_min,
    read_text_lines,
};
use crate::pytime::{self, DateTime};

pub const SCHEMA_VERSION: i64 = 1;
pub const REPEATED_SETTLED_WINDOW_S: f64 = 3.0;
pub const STEERING_DEADZONE: f64 = 0.08;
pub const BEARING_DEADZONE_DEG: f64 = 3.0;
/// Recorded episodes shorter than the live detector's own sustain are not episodes.
pub const SLIP_SUSTAIN_S: f64 = wheel_slip::SLIP_SUSTAIN_S;

const ROW_OPTS: DumpOpts = DumpOpts::compact().allow_nan(false);
const SUMMARY_OPTS: DumpOpts = DumpOpts::indented(2).allow_nan(false);

// ---------------------------------------------------------------------------------
// Shared with challenge_results

/// `wheel_slip.classify(telemetry)` on a decoded telemetry dict.
pub(crate) fn classify(telemetry: &Value) -> Result<wheel_slip::Classification, PyErr> {
    let d = match telemetry {
        Value::Dict(d) => d,
        other => {
            return Err(PyErr::AttributeError(format!(
                "'{}' object has no attribute 'get'",
                other.type_name()
            )));
        }
    };
    let mut sample = PyDict::new();
    for key in [
        "ground_speed_ms",
        "travel_bearing_deg",
        "wheel_max_driven_ms",
        "wheel_min_ms",
        "wheel_speed_ms",
    ] {
        if let Some(v) = d.get(key) {
            sample.insert(key.to_string(), to_pyvalue(v));
        }
    }
    wheel_slip::classify(&sample).map_err(|e| match e.kind {
        beam_reports::pyops::PyErrKind::OverflowError => PyErr::OverflowError(e.msg),
        beam_reports::pyops::PyErrKind::TypeError => PyErr::TypeError(e.msg),
        _ => PyErr::ValueError(e.msg),
    })
}

/// The classification as Python's dict.
pub(crate) fn classification_value(c: &wheel_slip::Classification) -> Value {
    dict([
        ("active", c.active.into()),
        ("kind", c.kind.into()),
        ("magnitude_mps", c.magnitude_mps.into()),
        ("raw_mps", c.raw_mps.into()),
        ("ratio", c.ratio.into()),
        ("slide", c.slide.into()),
        ("slide_deg", c.slide_deg.into()),
    ])
}

fn to_pyvalue(v: &Value) -> PyValue {
    match v {
        Value::Null => PyValue::None,
        Value::Bool(b) => PyValue::Bool(*b),
        Value::Int(i) => PyValue::Int(i.to_string()),
        Value::BigInt(t) => PyValue::Int(t.clone()),
        Value::Float(f) => PyValue::Float(*f),
        Value::Str(s) => PyValue::Str(s.clone()),
        Value::List(l) => PyValue::List(l.iter().map(to_pyvalue).collect()),
        Value::Dict(d) => {
            PyValue::Dict(d.iter().map(|(k, v)| (k.clone(), to_pyvalue(v))).collect())
        }
    }
}

/// `_finite(x) or 0.0`: a zero (either sign) and `None` both become `+0.0`.
pub(crate) fn or_zero(v: Option<f64>) -> f64 {
    match v {
        Some(x) if x != 0.0 => x,
        _ => 0.0,
    }
}

/// `_round(value, digits)`.
pub(crate) fn round_opt(v: Option<f64>, digits: i32) -> Value {
    match v {
        Some(x) => Value::Float(py_round(x, digits)),
        None => Value::Null,
    }
}

/// A dict key as `json.dumps` writes a non-string key.
fn json_key(v: &Value) -> Result<String, PyErr> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        Value::Null => Ok("null".to_string()),
        Value::Bool(true) => Ok("true".to_string()),
        Value::Bool(false) => Ok("false".to_string()),
        Value::Int(i) => Ok(i.to_string()),
        Value::BigInt(t) => Ok(t.clone()),
        Value::Float(f) => Ok(pyjson::dumps(&Value::Float(*f), &DumpOpts::compact())?),
        other => Err(pyjson::unhashable(other)),
    }
}

// ---------------------------------------------------------------------------------
// Analysis

fn sign(value: Option<f64>, deadzone: f64) -> i32 {
    match value {
        Some(v) if v.abs() > deadzone => {
            if v > 0.0 {
                1
            } else {
                -1
            }
        }
        _ => 0,
    }
}

struct Episode {
    start_s: f64,
    side: Value,
    phases: Vec<String>,
    max_severity: f64,
    closed: Option<(f64, f64, String)>,
}

impl Episode {
    fn close(mut self, episodes: &mut Vec<Episode>, end_t: f64, outcome: &str) {
        self.closed = Some((
            py_round(end_t, 3),
            py_round(py_max(0.0, end_t - self.start_s), 3),
            outcome.to_string(),
        ));
        episodes.push(self);
    }

    fn outcome(&self) -> &str {
        self.closed.as_ref().map(|c| c.2.as_str()).unwrap_or("")
    }

    fn to_value(&self) -> Value {
        let (end_s, duration_s, outcome) = self.closed.clone().unwrap_or_default();
        dict([
            ("start_s", self.start_s.into()),
            ("side", self.side.clone()),
            ("max_severity", self.max_severity.into()),
            ("end_s", end_s.into()),
            ("duration_s", duration_s.into()),
            ("outcome", outcome.into()),
            (
                "phase_transitions",
                (self.phases.len() as i64 - 1).max(0).into(),
            ),
        ])
    }
}

struct SlipEpisode {
    start_s: f64,
    kind: &'static str,
    max_magnitude_mps: f64,
    samples: i64,
    during_correction: bool,
    end_s: f64,
    duration_s: f64,
}

impl SlipEpisode {
    fn to_value(&self) -> Value {
        dict([
            ("start_s", self.start_s.into()),
            ("kind", self.kind.into()),
            ("max_magnitude_mps", self.max_magnitude_mps.into()),
            ("samples", self.samples.into()),
            ("during_correction", self.during_correction.into()),
            ("end_s", self.end_s.into()),
            ("duration_s", self.duration_s.into()),
        ])
    }
}

fn get<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Value>, PyErr> {
    v.py_get(key)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(Value::truthy)
}

fn float_or(v: Option<&Value>, default: f64) -> Result<f64, PyErr> {
    match v {
        None => Ok(default),
        Some(v) => v.py_float(),
    }
}

/// A compact, evidence-oriented summary of recorded sample dicts.
///
/// `Err` where Python raises: a record or nested field of the wrong type
/// (`AttributeError`/`TypeError`), or a sample time that is not a number.
pub fn analyze_records(records: &[Value], path: Option<&str>) -> Result<Value, PyErr> {
    let mut samples: Vec<&Value> = Vec::new();
    let mut markers: Vec<Value> = Vec::new();
    for row in records {
        if get(row, "kind")?.is_some_and(|k| k.eq_str("sample")) {
            samples.push(row);
        }
    }
    for row in records {
        if get(row, "kind")?.is_some_and(|k| k.eq_str("marker")) {
            let note = match get(row, "note")? {
                Some(v) => v.py_str(),
                None => String::new(),
            };
            markers.push(dict([
                ("t", round_opt(finite(get(row, "t")?), 3)),
                ("note", note.into()),
            ]));
        }
    }
    let path_value: Value = path.filter(|p| !p.is_empty()).map(str::to_string).into();
    if samples.is_empty() {
        return Ok(dict([
            ("schema", SCHEMA_VERSION.into()),
            ("path", path_value),
            ("samples", 0.into()),
            ("duration_s", 0.0.into()),
            (
                "assessment_flags",
                Value::List(vec!["no road-guidance samples were recorded".into()]),
            ),
        ]));
    }

    let last = samples[samples.len() - 1];
    let duration = py_max(0.0, float_or(get(last, "t")?, 0.0)?);
    let mut distance_m = 0.0f64;
    let mut previous_pos: Option<(f64, f64)> = None;
    let mut state_counts: IndexMap<String, i64> = IndexMap::new();
    let mut edge_ids: BTreeSet<String> = BTreeSet::new();
    let mut missing_diagnostic = 0i64;
    let mut leading_missing_diagnostic = 0i64;
    let mut saw_diagnostic = false;
    let mut eligible = 0i64;
    let (mut within_lateral, mut within_heading, mut within_lateral_speed) = (0i64, 0i64, 0i64);
    let mut settled_candidates = 0i64;
    let mut longest_candidate_run = 0i64;
    let mut candidate_run = 0i64;
    let mut settled_times: Vec<f64> = Vec::new();
    let mut audio_settled_times: Vec<f64> = Vec::new();
    let mut steering_reversals = 0i64;
    let (mut source_lua, mut source_telemetry) = (0i64, 0i64);
    let mut bearing_reversals = 0i64;
    let mut target_crossings = 0i64;
    let (mut last_steer_sign, mut last_bearing_sign, mut last_target_sign) = (0, 0, 0);
    let mut correction_samples = 0i64;
    // max_abs, in Python's key order.
    let mut max_target_error = 0.0f64;
    let mut max_lateral_speed = 0.0f64;
    let mut max_heading_error = 0.0f64;
    let mut max_correction_bearing = 0.0f64;
    let mut max_steering_input = 0.0f64;
    let mut max_applied_steering = 0.0f64;
    let mut max_raw_slip = 0.0f64;
    let mut max_pitch = 0.0f64;
    // drive
    let mut start_z: Option<f64> = None;
    let mut end_z: Option<f64> = None;
    let mut min_ground: Option<f64> = None;
    let mut max_ground = 0.0f64;
    let mut max_throttle = 0.0f64;
    let mut max_brake = 0.0f64;
    let mut tc_samples = 0i64;
    // settled band
    let mut band_min_lat: Option<f64> = None;
    let mut band_max_lat: Option<f64> = None;
    let mut band_heading: Option<f64> = None;
    let mut band_min_speed: Option<f64> = None;
    let mut band_max_speed: Option<f64> = None;
    let mut contact_material_sets: IndexMap<String, i64> = IndexMap::new();
    let mut contact_material_changes: Vec<Value> = Vec::new();
    let mut last_contact_materials: Option<String> = None;

    let mut episodes: Vec<Episode> = Vec::new();
    let mut current_episode: Option<Episode> = None;
    let mut slip_episodes: Vec<SlipEpisode> = Vec::new();
    let mut current_slip: Option<SlipEpisode> = None;
    let mut slip_overlap_samples = 0i64;

    let unknown = Value::from("unknown");
    for row in &samples {
        let t = float_or(get(row, "t")?, 0.0)?;
        let road = or_empty_dict(get(row, "road")?);
        let correction = or_empty_dict(get(&road, "correction")?);
        let diagnostic = get(&road, "diagnostic")?.filter(|v| !v.is_null());
        let telemetry = or_empty_dict(get(row, "telemetry")?);
        let state = get(&road, "state")?.unwrap_or(&unknown).clone();
        *state_counts.entry(json_key(&state)?).or_insert(0) += 1;

        let pos = or_empty_dict(get(&telemetry, "position")?);
        let x = finite(get(&pos, "x")?);
        let y = finite(get(&pos, "y")?);
        if let (Some(x), Some(y)) = (x, y) {
            if let Some((px, py)) = previous_pos {
                let step = py_hypot(x - px, y - py);
                if step < 50.0 {
                    // vehicle resets are boundaries, not distance travelled
                    distance_m += step;
                }
            }
            previous_pos = Some((x, y));
        }
        let z = finite(get(&pos, "z")?);
        if let Some(z) = z {
            if start_z.is_none() {
                start_z = Some(z);
            }
            end_z = Some(z);
        }
        if let Some(gs) = finite(get(&telemetry, "ground_speed_ms")?) {
            let cur = min_ground.unwrap_or(gs);
            min_ground = Some(py_min(cur, gs));
            max_ground = py_max(max_ground, gs);
        }
        max_throttle = py_max(max_throttle, or_zero(finite(get(&telemetry, "throttle")?)));
        max_brake = py_max(max_brake, or_zero(finite(get(&telemetry, "brake")?)));
        tc_samples += truthy(get(&telemetry, "traction_control_active")?) as i64;
        if let Some(pitch) = finite(get(&telemetry, "pitch_rad")?) {
            max_pitch = py_max(max_pitch, pitch.to_degrees().abs());
        }

        let active = truthy(get(&correction, "active")?);
        let settled = truthy(get(&correction, "settled")?);
        if settled {
            settled_times.push(py_round(t, 3));
        }
        let audio = or_empty_dict(get(row, "audio")?);
        if truthy(get(&audio, "settled_triggered")?) {
            audio_settled_times.push(py_round(t, 3));
        }

        let mut diag: Option<&Dict> = None;
        match diagnostic {
            None => {
                if state.eq_str("onRoad") {
                    if saw_diagnostic {
                        missing_diagnostic += 1;
                    } else {
                        leading_missing_diagnostic += 1;
                    }
                }
            }
            Some(dv) => {
                saw_diagnostic = true;
                let edge_id = get(dv, "edgeId")?;
                let d = dv.as_dict().expect("py_get succeeded, so this is a dict");
                diag = Some(d);
                if let Some(e) = edge_id.filter(|e| e.truthy()) {
                    edge_ids.insert(e.py_str());
                }
                let was_active = truthy(d.get("activeBefore")) || active || settled;
                if was_active {
                    eligible += 1;
                    within_lateral += truthy(d.get("withinLateral")) as i64;
                    within_heading += truthy(d.get("withinHeading")) as i64;
                    within_lateral_speed += truthy(d.get("withinLateralSpeed")) as i64;
                    let candidate = truthy(d.get("settledCandidate"));
                    settled_candidates += candidate as i64;
                    candidate_run = if candidate { candidate_run + 1 } else { 0 };
                    longest_candidate_run = longest_candidate_run.max(candidate_run);
                    if let Some(tol) = finite(d.get("targetTolerance")) {
                        band_min_lat = Some(band_min_lat.map_or(tol, |c| py_min(c, tol)));
                        band_max_lat = Some(band_max_lat.map_or(tol, |c| py_max(c, tol)));
                    }
                    let heading_tol = finite(d.get("settledHeadingTolerance"));
                    let speed_tol = finite(d.get("settledLateralSpeedTolerance"));
                    if heading_tol.is_some() {
                        band_heading = heading_tol;
                    }
                    if let Some(tol) = speed_tol {
                        band_min_speed = Some(band_min_speed.map_or(tol, |c| py_min(c, tol)));
                        band_max_speed = Some(band_max_speed.map_or(tol, |c| py_max(c, tol)));
                    }
                }

                let cm = match d.get("contactMaterials") {
                    Some(v) if v.truthy() => py_strip(&v.py_str()).to_string(),
                    _ => String::new(),
                };
                if !cm.is_empty() {
                    *contact_material_sets.entry(cm.clone()).or_insert(0) += 1;
                    if let Some(last) = &last_contact_materials
                        && !last.is_empty()
                        && *last != cm
                    {
                        contact_material_changes.push(dict([
                            ("t", py_round(t, 3).into()),
                            ("from", last.as_str().into()),
                            ("to", cm.as_str().into()),
                            ("z_m", round_opt(z, 2)),
                            ("road_state", state.clone()),
                        ]));
                    }
                    last_contact_materials = Some(cm);
                }

                for (slot, field) in [
                    (&mut max_target_error, "targetError"),
                    (&mut max_lateral_speed, "lateralSpeed"),
                    (&mut max_heading_error, "headingError"),
                    (&mut max_correction_bearing, "correctionBearing"),
                ] {
                    if let Some(v) = finite(d.get(field)) {
                        *slot = py_max(*slot, v.abs());
                    }
                }
            }
        }
        let diag_truthy = diag.filter(|d| !d.is_empty());

        let mut raw_steering = diag_truthy.and_then(|d| finite(d.get("steeringInput")));
        if raw_steering.is_none() {
            raw_steering = finite(get(&telemetry, "steering_input")?);
        }
        let mut applied = diag_truthy.and_then(|d| finite(d.get("steering")));
        if applied.is_some() {
            source_lua += 1;
        } else {
            applied = finite(get(&telemetry, "steering")?);
            if applied.is_some() {
                source_telemetry += 1;
            }
        }
        // Older diagnostic files predate the applied channel. Raw input is still
        // preferable to reporting no steering at all when reviewing those sessions.
        let steering = or_zero(applied.or(raw_steering));
        max_steering_input = py_max(max_steering_input, or_zero(raw_steering).abs());
        max_applied_steering = py_max(max_applied_steering, steering.abs());
        if active {
            correction_samples += 1;
            let steer_sign = sign(Some(steering), STEERING_DEADZONE);
            if steer_sign != 0 && last_steer_sign != 0 && steer_sign != last_steer_sign {
                steering_reversals += 1;
            }
            if steer_sign != 0 {
                last_steer_sign = steer_sign;
            }

            let bearing_value = match diag_truthy {
                Some(d) => finite(d.get("correctionBearing")),
                None => finite(get(&correction, "bearing")?),
            };
            let bearing_sign = sign(bearing_value, BEARING_DEADZONE_DEG);
            if bearing_sign != 0 && last_bearing_sign != 0 && bearing_sign != last_bearing_sign {
                bearing_reversals += 1;
            }
            if bearing_sign != 0 {
                last_bearing_sign = bearing_sign;
            }

            let target_sign = sign(diag_truthy.and_then(|d| finite(d.get("targetError"))), 0.05);
            if target_sign != 0 && last_target_sign != 0 && target_sign != last_target_sign {
                target_crossings += 1;
            }
            if target_sign != 0 {
                last_target_sign = target_sign;
            }

            let episode = current_episode.get_or_insert_with(|| Episode {
                start_s: py_round(t, 3),
                side: diag_truthy
                    .and_then(|d| d.get("targetSide").cloned())
                    .unwrap_or(Value::Null),
                phases: Vec::new(),
                max_severity: 0.0,
                closed: None,
            });
            let phase = match get(&correction, "phase")? {
                Some(v) => v.py_str(),
                None => "idle".to_string(),
            };
            if episode.phases.last() != Some(&phase) {
                episode.phases.push(phase);
            }
            episode.max_severity = py_round(
                py_max(
                    episode.max_severity,
                    or_zero(finite(get(&correction, "severity")?)),
                ),
                3,
            );
        } else if let Some(episode) = current_episode.take() {
            episode.close(
                &mut episodes,
                t,
                if settled {
                    "settled"
                } else {
                    "cleared_without_settling"
                },
            );
            last_steer_sign = 0;
            last_bearing_sign = 0;
            last_target_sign = 0;
        }

        // One rule for the live tone and every recording (wheel_slip).
        let slip = classify(&telemetry)?;
        let raw_slip = slip.raw_mps;
        max_raw_slip = py_max(max_raw_slip, raw_slip.abs());
        if slip.active {
            let cur = current_slip.get_or_insert_with(|| SlipEpisode {
                start_s: py_round(t, 3),
                kind: slip.kind,
                max_magnitude_mps: 0.0,
                samples: 0,
                during_correction: false,
                end_s: 0.0,
                duration_s: 0.0,
            });
            cur.samples += 1;
            cur.max_magnitude_mps = py_round(py_max(cur.max_magnitude_mps, raw_slip.abs()), 3);
            cur.during_correction |= active;
            slip_overlap_samples += active as i64;
        } else if let Some(mut cur) = current_slip.take() {
            cur.end_s = py_round(t, 3);
            cur.duration_s = py_round(t - cur.start_s, 3);
            if cur.duration_s >= SLIP_SUSTAIN_S {
                slip_episodes.push(cur);
            }
        }
    }

    let end_t = float_or(get(last, "t")?, 0.0)?;
    if let Some(episode) = current_episode.take() {
        episode.close(&mut episodes, end_t, "recording_ended");
    }
    if let Some(mut cur) = current_slip.take() {
        cur.end_s = py_round(end_t, 3);
        cur.duration_s = py_round(end_t - cur.start_s, 3);
        if cur.duration_s >= SLIP_SUSTAIN_S {
            slip_episodes.push(cur);
        }
    }

    let mut repeated: Vec<Value> = Vec::new();
    for pair in audio_settled_times.windows(2) {
        let (previous, current) = (pair[0], pair[1]);
        let interval = current - previous;
        if interval <= REPEATED_SETTLED_WINDOW_S {
            repeated.push(dict([
                ("first_s", previous.into()),
                ("second_s", current.into()),
                ("interval_s", py_round(interval, 3).into()),
            ]));
        }
    }

    let ratio = |count: i64| -> Value {
        if eligible != 0 {
            Value::Float(py_round(count as f64 / eligible as f64, 3))
        } else {
            Value::Null
        }
    };

    let start_z_r = start_z.map(|v| py_round(v, 2));
    let end_z_r = end_z.map(|v| py_round(v, 2));
    let elevation = match (start_z_r, end_z_r) {
        (Some(s), Some(e)) => Value::Float(py_round(e - s, 2)),
        _ => Value::Null,
    };
    let drive = dict([
        ("start_z_m", start_z_r.into()),
        ("end_z_m", end_z_r.into()),
        ("min_ground_speed_mps", round_opt(min_ground, 3)),
        ("max_ground_speed_mps", round_opt(Some(max_ground), 3)),
        ("max_throttle", round_opt(Some(max_throttle), 3)),
        ("max_brake", round_opt(Some(max_brake), 3)),
        ("traction_control_samples", tc_samples.into()),
        ("elevation_change_m", elevation),
    ]);
    let settled_band = dict([
        ("min_lateral_tolerance_m", round_opt(band_min_lat, 3)),
        ("max_lateral_tolerance_m", round_opt(band_max_lat, 3)),
        ("heading_tolerance_deg", round_opt(band_heading, 3)),
        (
            "min_lateral_speed_tolerance_mps",
            round_opt(band_min_speed, 3),
        ),
        (
            "max_lateral_speed_tolerance_mps",
            round_opt(band_max_speed, 3),
        ),
    ]);

    let mut flags: Vec<String> = Vec::new();
    if !saw_diagnostic && leading_missing_diagnostic != 0 {
        missing_diagnostic += leading_missing_diagnostic;
    }
    if !repeated.is_empty() {
        flags.push(format!(
            "{} settled-tone retrigger(s) within {} seconds",
            repeated.len(),
            beam_base::pyfmt::py_f(REPEATED_SETTLED_WINDOW_S, 0)
        ));
    }
    let unresolved = episodes.iter().filter(|e| e.outcome() != "settled").count();
    if unresolved != 0 {
        flags.push(format!("{unresolved} correction episode(s) did not settle"));
    }
    if steering_reversals >= 4 {
        flags.push(format!(
            "{steering_reversals} steering reversals occurred during correction"
        ));
    }
    if target_crossings >= 2 {
        flags.push(format!(
            "the vehicle crossed the lane target {target_crossings} times"
        ));
    }
    let overlap = slip_episodes.iter().filter(|e| e.during_correction).count();
    if overlap != 0 {
        flags.push(format!(
            "{overlap} sustained slip episode(s) overlapped lane correction"
        ));
    }
    if eligible != 0 && (settled_candidates as f64 / eligible as f64) < 0.05 {
        flags.push(
            "the full settled condition held in fewer than 5 percent of correction samples"
                .to_string(),
        );
    }
    if correction_samples != 0 && source_lua == 0 {
        flags.push("vehicle-VM steering input was unavailable during correction".to_string());
    }
    if saw_diagnostic && contact_material_sets.is_empty() {
        flags.push("wheel contact-material data was unavailable".to_string());
    }
    if missing_diagnostic != 0 {
        flags.push(format!(
            "{missing_diagnostic} sample(s) lacked Lua diagnostic fields"
        ));
    }
    if flags.is_empty() {
        flags.push("no automatic anomaly threshold was crossed".to_string());
    }

    let mut sets: Vec<(String, i64)> = contact_material_sets.into_iter().collect();
    sets.sort_by(|a, b| a.0.cmp(&b.0));
    let floats = |v: &[f64]| Value::List(v.iter().map(|x| Value::Float(*x)).collect());

    Ok(dict([
        ("schema", SCHEMA_VERSION.into()),
        ("path", path_value),
        ("samples", samples.len().into()),
        ("duration_s", py_round(duration, 3).into()),
        ("distance_travelled_m", py_round(distance_m, 2).into()),
        ("drive", drive),
        (
            "road_state_samples",
            Value::Dict(
                state_counts
                    .into_iter()
                    .map(|(k, v)| (k, Value::Int(v)))
                    .collect(),
            ),
        ),
        ("markers", Value::List(markers)),
        (
            "navigation_edges",
            Value::List(edge_ids.into_iter().map(Value::from).collect()),
        ),
        (
            "diagnostic_startup_samples_skipped",
            (if saw_diagnostic {
                leading_missing_diagnostic
            } else {
                0
            })
            .into(),
        ),
        (
            "correction",
            dict([
                ("samples", correction_samples.into()),
                (
                    "episodes",
                    Value::List(episodes.iter().map(Episode::to_value).collect()),
                ),
                ("settled_packets_s", floats(&settled_times)),
                ("settled_audio_triggers_s", floats(&audio_settled_times)),
                ("repeated_settled", Value::List(repeated)),
                ("eligible_samples", eligible.into()),
                ("within_lateral_ratio", ratio(within_lateral)),
                ("within_heading_ratio", ratio(within_heading)),
                ("within_lateral_speed_ratio", ratio(within_lateral_speed)),
                ("all_settled_conditions_ratio", ratio(settled_candidates)),
                (
                    "longest_settled_candidate_run_samples",
                    longest_candidate_run.into(),
                ),
                ("steering_reversals", steering_reversals.into()),
                (
                    "steering_source_samples",
                    dict([
                        ("lua_vehicle", source_lua.into()),
                        ("telemetry", source_telemetry.into()),
                    ]),
                ),
                ("correction_bearing_reversals", bearing_reversals.into()),
                ("target_crossings", target_crossings.into()),
                ("settled_band", settled_band),
            ]),
        ),
        (
            "slip",
            dict([
                (
                    "episodes",
                    Value::List(slip_episodes.iter().map(SlipEpisode::to_value).collect()),
                ),
                ("correction_overlap_samples", slip_overlap_samples.into()),
            ]),
        ),
        (
            "contact_materials",
            dict([
                (
                    "sample_sets",
                    Value::Dict(sets.into_iter().map(|(k, v)| (k, Value::Int(v))).collect()),
                ),
                ("changes", Value::List(contact_material_changes)),
            ]),
        ),
        (
            "max_absolute",
            dict([
                ("target_error_m", py_round(max_target_error, 3).into()),
                ("lateral_speed_mps", py_round(max_lateral_speed, 3).into()),
                ("heading_error_deg", py_round(max_heading_error, 3).into()),
                (
                    "correction_bearing_deg",
                    py_round(max_correction_bearing, 3).into(),
                ),
                ("steering_input", py_round(max_steering_input, 3).into()),
                ("applied_steering", py_round(max_applied_steering, 3).into()),
                ("raw_slip_mps", py_round(max_raw_slip, 3).into()),
                ("vehicle_pitch_deg", py_round(max_pitch, 3).into()),
            ]),
        ),
        (
            "assessment_flags",
            Value::List(flags.into_iter().map(Value::from).collect()),
        ),
    ]))
}

/// Read an NDJSON session (undecodable lines skipped, non-dict rows dropped) and
/// analyse it.
pub fn analyze_file(path: &Path) -> Result<Value, PyErr> {
    let mut records = Vec::new();
    for line in read_text_lines(path)? {
        let Ok(row) = pyjson::loads(&line) else {
            continue;
        };
        if matches!(row, Value::Dict(_)) {
            records.push(row);
        }
    }
    analyze_records(&records, Some(&path_str(path)))
}

// ---------------------------------------------------------------------------------
// Recorder

struct RecState {
    file: Option<CrlfWriter<File>>,
    path: Option<PathBuf>,
    started: Option<f64>,
    samples: i64,
    markers: i64,
}

/// Thread-safe NDJSON session recorder with reviewable summary files.
pub struct RoadDiagnosticRecorder {
    pub directory: PathBuf,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    wall_clock: Box<dyn Fn() -> f64 + Send + Sync>,
    utc_now: Box<dyn Fn() -> DateTime + Send + Sync>,
    local_now: Box<dyn Fn() -> DateTime + Send + Sync>,
    state: Mutex<RecState>,
}

impl RoadDiagnosticRecorder {
    /// On `time.monotonic()` and `time.time()`.
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self::with_clocks(directory, pytime::monotonic, pytime::time_time)
    }

    pub fn with_clocks(
        directory: impl AsRef<Path>,
        clock: impl Fn() -> f64 + Send + Sync + 'static,
        wall_clock: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        RoadDiagnosticRecorder {
            directory: pyfs::py_path(directory.as_ref()),
            clock: Box::new(clock),
            wall_clock: Box::new(wall_clock),
            utc_now: Box::new(DateTime::utc_now),
            local_now: Box::new(DateTime::local_now),
            state: Mutex::new(RecState {
                file: None,
                path: None,
                started: None,
                samples: 0,
                markers: 0,
            }),
        }
    }

    /// Replace `datetime.now(timezone.utc)` (the meta row) and `datetime.now()` (the
    /// file name stamp), which Python does not let a caller inject; for fixtures.
    pub fn with_datetimes(
        mut self,
        utc_now: impl Fn() -> DateTime + Send + Sync + 'static,
        local_now: impl Fn() -> DateTime + Send + Sync + 'static,
    ) -> Self {
        self.utc_now = Box::new(utc_now);
        self.local_now = Box::new(local_now);
        self
    }

    fn write_locked(s: &mut RecState, row: &Value) -> Result<(), PyErr> {
        let text = pyjson::dumps(row, &ROW_OPTS)?;
        let file = s
            .file
            .as_mut()
            .ok_or_else(|| PyErr::RuntimeError("no road diagnostic recording is active".into()))?;
        file.write_all(format!("{text}\n").as_bytes())?;
        file.flush()?;
        Ok(())
    }

    /// Open a new session. `label` falsy means `"hill-climb"`.
    pub fn start(&self, label: Option<&str>) -> Result<Value, PyErr> {
        let mut s = self.state.lock();
        if s.file.is_some() {
            return Err(PyErr::RuntimeError(
                "a road diagnostic recording is already active".to_string(),
            ));
        }
        std::fs::create_dir_all(&self.directory)?;
        let label = label.filter(|l| !l.is_empty()).unwrap_or("hill-climb");
        let subbed = pyjson::sub_runs(label, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        let mut safe = pyjson::take_chars(pyjson::strip_chars(&subbed, &['-']), 48);
        if safe.is_empty() {
            safe = "hill-climb".to_string();
        }
        let stamp = (self.local_now)().strftime("%Y%m%d_%H%M%S");
        let mut path = self
            .directory
            .join(format!("road_guidance_{stamp}_{safe}.ndjson"));
        let mut suffix = 2;
        while path.exists() {
            path = self
                .directory
                .join(format!("road_guidance_{stamp}_{safe}_{suffix}.ndjson"));
            suffix += 1;
        }
        s.file = Some(pyfs::create_new_text(&path)?);
        s.path = Some(path);
        s.started = Some((self.clock)());
        s.samples = 0;
        s.markers = 0;
        let meta = dict([
            ("kind", "meta".into()),
            ("schema", SCHEMA_VERSION.into()),
            ("started_utc", (self.utc_now)().isoformat().into()),
            ("label", safe.into()),
        ]);
        Self::write_locked(&mut s, &meta)?;
        Ok(self.status_locked(&s))
    }

    fn elapsed(&self, s: &RecState, digits: i32) -> f64 {
        py_round((self.clock)() - s.started.unwrap_or(0.0), digits)
    }

    /// Record one R2 packet with its telemetry. `Ok(false)` when not recording;
    /// `Err(ValueError)` for a non-finite float, which Python's `allow_nan=False`
    /// refuses (and its road listener does not catch).
    pub fn record(
        &self,
        packet: &Value,
        telemetry: &Value,
        audio_event: Option<&Value>,
    ) -> Result<bool, PyErr> {
        let mut s = self.state.lock();
        if s.file.is_none() {
            return Ok(false);
        }
        let row = dict([
            ("kind", "sample".into()),
            ("t", self.elapsed(&s, 6).into()),
            ("wall_time", (self.wall_clock)().into()),
            ("road", packet.clone()),
            ("telemetry", telemetry.clone()),
            ("audio", or_empty_dict(audio_event)),
        ]);
        Self::write_locked(&mut s, &row)?;
        s.samples += 1;
        Ok(true)
    }

    /// Add a marker note to the running session.
    pub fn mark(&self, note: &str) -> Result<Value, PyErr> {
        let note = py_strip(note);
        if note.is_empty() {
            return Err(PyErr::ValueError("marker note is required".to_string()));
        }
        let mut s = self.state.lock();
        if s.file.is_none() {
            return Err(PyErr::RuntimeError(
                "no road diagnostic recording is active".to_string(),
            ));
        }
        let row = dict([
            ("kind", "marker".into()),
            ("t", self.elapsed(&s, 6).into()),
            ("wall_time", (self.wall_clock)().into()),
            ("note", pyjson::take_chars(note, 500).into()),
        ]);
        Self::write_locked(&mut s, &row)?;
        s.markers += 1;
        Ok(self.status_locked(&s))
    }

    /// Close the session, write its `.summary.json`, and return the summary with
    /// `summary_path` added.
    pub fn stop(&self) -> Result<Value, PyErr> {
        let path = {
            let mut s = self.state.lock();
            if s.file.is_none() {
                return Err(PyErr::RuntimeError(
                    "no road diagnostic recording is active".to_string(),
                ));
            }
            let path = s.path.clone().unwrap_or_default();
            let row = dict([
                ("kind", "end".into()),
                ("t", self.elapsed(&s, 6).into()),
                ("wall_time", (self.wall_clock)().into()),
            ]);
            Self::write_locked(&mut s, &row)?;
            s.file = None;
            s.started = None;
            path
        };
        let mut summary = analyze_file(&path)?;
        let summary_path = with_suffix(&path, ".summary.json");
        let text = pyjson::dumps(&summary, &SUMMARY_OPTS)? + "\n";
        pyfs::write_text(&summary_path, &text)?;
        if let Value::Dict(d) = &mut summary {
            d.insert("summary_path".into(), path_str(&summary_path).into());
        }
        Ok(summary)
    }

    fn status_locked(&self, s: &RecState) -> Value {
        let active = s.file.is_some();
        dict([
            ("active", active.into()),
            ("path", s.path.as_deref().map(path_str).into()),
            (
                "elapsed_s",
                (if active { self.elapsed(s, 1) } else { 0.0 }).into(),
            ),
            ("samples", s.samples.into()),
            ("markers", s.markers.into()),
        ])
    }

    pub fn status(&self) -> Value {
        let s = self.state.lock();
        self.status_locked(&s)
    }

    /// Analyse the current session, the newest one, or a bare `.ndjson` name.
    pub fn review(&self, session: Option<&str>) -> Result<Value, PyErr> {
        let path = {
            let mut s = self.state.lock();
            if let Some(f) = s.file.as_mut() {
                f.flush()?;
            }
            match session.filter(|n| !n.is_empty()) {
                Some(name) => {
                    if !pyfs::is_bare_name(name) || !name.ends_with(".ndjson") {
                        return Err(PyErr::ValueError(
                            "session must be a bare .ndjson file name".to_string(),
                        ));
                    }
                    Some(self.directory.join(name))
                }
                None => match &s.path {
                    Some(p) => Some(p.clone()),
                    None => glob_sorted(&self.directory, "road_guidance_", ".ndjson").pop(),
                },
            }
        };
        match path {
            Some(p) if p.is_file() => analyze_file(&p),
            _ => Err(PyErr::RuntimeError(
                "no road diagnostic session is available".to_string(),
            )),
        }
    }

    /// The newest sessions first; `limit` through `int()`, clamped to 1..=100.
    pub fn list_sessions(&self, limit: &Value) -> Result<Value, PyErr> {
        std::fs::create_dir_all(&self.directory)?;
        let mut paths = glob_sorted(&self.directory, "road_guidance_", ".ndjson");
        paths.reverse();
        let n = limit.py_int()?.clamp(i64::MIN, 100).max(1) as usize;
        let mut out = Vec::new();
        for path in paths.iter().take(n) {
            let bytes = std::fs::metadata(path)?.len();
            out.push(dict([
                (
                    "session",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                        .into(),
                ),
                ("bytes", (bytes as i64).into()),
                (
                    "summary",
                    with_suffix(path, ".summary.json").is_file().into(),
                ),
            ]));
        }
        Ok(Value::List(out))
    }
}
