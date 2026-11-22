//! Protocol and wording helpers for road awareness, from road_guidance.py.
//!
//! The UDP listener lives in the engine; this module has no audio or speech
//! dependencies so its compatibility and phrasing rules can be exercised without
//! starting the application.

use beam_base::pyerr::ValueError;
use beam_base::pyfmt::{py_f, py_mod, py_round, py_round0, py_strip};
use indexmap::IndexMap;
use parking_lot::Mutex;

use crate::pyjson::{self, Dict, PyErr, Value, char_len, dict, loads};
use crate::pytime;

pub const R2_PREFIX: &str = "R2|";
pub const R2_STALE_SECONDS: f64 = 1.0;
const STATES: &[&str] = &["dormant", "offRoad", "onRoad"];
const PHASES: &[&str] = &["approach", "near"];
const CORRECTION_PHASES: &[&str] = &["idle", "correct", "unwind"];
const SIGNAL_STATES: &[&str] = &[
    "stop",
    "red",
    "yellow",
    "green",
    "redYellow",
    "flashingRed",
    "flashingYellow",
    "flashingGreen",
    "off",
    "unknown",
];
const DIAGNOSTIC_FLOATS: &[&str] = &[
    "edgeT",
    "roadRadius",
    "signedLateral",
    "lateralDistance",
    "lateralRatio",
    "lateralSpeed",
    "predictedLateral",
    "predictedRatio",
    "predictionSeconds",
    "targetSide",
    "targetOffset",
    "targetError",
    "targetTolerance",
    "settledHeadingTolerance",
    "settledLateralSpeedTolerance",
    "headingError",
    "correctionBearing",
    "secondsToTarget",
    "outwardSpeed",
    "timeToEdge",
    "speed",
    "clearTicksBefore",
    "clearTicksAfter",
    "rearmTicks",
    "steeringInput",
    "steering",
];
const DIAGNOSTIC_BOOLS: &[&str] = &[
    "activeBefore",
    "shouldEnter",
    "withinLateral",
    "withinHeading",
    "withinLateralSpeed",
    "settledCandidate",
    "inDecisionZone",
    "correctionArmed",
];

// ---------------------------------------------------------------------------------
// The parsed packet

/// `offRoad`: where the nearest compatible road is.
#[derive(Debug, Clone, PartialEq)]
pub struct OffRoad {
    pub bearing: f64,
    pub distance: f64,
}

/// `correction`: the lane-keeping correction.
#[derive(Debug, Clone, PartialEq)]
pub struct Correction {
    pub active: bool,
    pub bearing: f64,
    pub severity: f64,
    pub phase: String,
    pub settled: bool,
    pub lateral_ratio: Option<f64>,
    pub heading_error: Option<f64>,
    pub time_to_edge: Option<f64>,
}

/// `junction`: the next junction. `phase` is `"approach"` or `"near"` on the wire, and
/// `"entered"` in a feed event.
#[derive(Debug, Clone, PartialEq)]
pub struct Junction {
    pub id: String,
    pub phase: String,
    pub entered: bool,
    pub kind: String,
    pub distance: f64,
    pub exits: Vec<f64>,
}

/// `signal`: a stop sign or traffic light ahead.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub phase: String,
    pub distance: f64,
    pub group_id: String,
    pub preview: bool,
}

/// `proximity`: the stop-bar target.
#[derive(Debug, Clone, PartialEq)]
pub struct Proximity {
    pub id: String,
    pub distance: f64,
    pub speed: f64,
    pub action: String,
    pub source: String,
    pub signed_distance: f64,
    pub kind: String,
    pub private_entry: String,
}

/// A validated R2 datagram: Python's dict from `parse_r2_packet`.
#[derive(Debug, Clone, PartialEq)]
pub struct R2Packet {
    pub state: String,
    pub speed_limit: Option<f64>,
    pub proximity: Option<Proximity>,
    pub one_way: bool,
    pub road_directions: Vec<f64>,
    pub off_road: Option<OffRoad>,
    pub correction: Option<Correction>,
    /// Only the recognised fields, in Python's insertion order.
    pub diagnostic: Option<Dict>,
    pub junction: Option<Junction>,
    pub signal: Option<Signal>,
    pub signal_status: String,
    pub signal_context: String,
    pub orientation: bool,
}

impl Default for R2Packet {
    /// The packet a bare `{"state": "onRoad"}` parses to.
    fn default() -> Self {
        R2Packet {
            state: "onRoad".to_string(),
            speed_limit: None,
            proximity: None,
            one_way: false,
            road_directions: Vec::new(),
            off_road: None,
            correction: None,
            diagnostic: None,
            junction: None,
            signal: None,
            signal_status: "unsupported".to_string(),
            signal_context: String::new(),
            orientation: false,
        }
    }
}

fn floats(v: &[f64]) -> Value {
    Value::List(v.iter().map(|x| Value::Float(*x)).collect())
}

impl OffRoad {
    pub fn to_value(&self) -> Value {
        dict([
            ("bearing", self.bearing.into()),
            ("distance", self.distance.into()),
        ])
    }
}

impl Correction {
    pub fn to_value(&self) -> Value {
        let mut d = pyjson::dict_of([
            ("active", self.active.into()),
            ("bearing", self.bearing.into()),
            ("severity", self.severity.into()),
            ("phase", self.phase.as_str().into()),
            ("settled", self.settled.into()),
        ]);
        if let Some(v) = self.lateral_ratio {
            d.insert("lateralRatio".into(), v.into());
        }
        if let Some(v) = self.heading_error {
            d.insert("headingError".into(), v.into());
        }
        if let Some(v) = self.time_to_edge {
            d.insert("timeToEdge".into(), v.into());
        }
        Value::Dict(d)
    }
}

impl Junction {
    pub fn to_value(&self) -> Value {
        dict([
            ("id", self.id.as_str().into()),
            ("phase", self.phase.as_str().into()),
            ("entered", self.entered.into()),
            ("kind", self.kind.as_str().into()),
            ("distance", self.distance.into()),
            ("exits", floats(&self.exits)),
        ])
    }
}

impl Signal {
    pub fn to_value(&self) -> Value {
        dict([
            ("id", self.id.as_str().into()),
            ("kind", self.kind.as_str().into()),
            ("state", self.state.as_str().into()),
            ("phase", self.phase.as_str().into()),
            ("distance", self.distance.into()),
            ("groupId", self.group_id.as_str().into()),
            ("preview", self.preview.into()),
        ])
    }
}

impl Proximity {
    pub fn to_value(&self) -> Value {
        dict([
            ("id", self.id.as_str().into()),
            ("distance", self.distance.into()),
            ("speed", self.speed.into()),
            ("action", self.action.as_str().into()),
            ("source", self.source.as_str().into()),
            ("signedDistance", self.signed_distance.into()),
            ("kind", self.kind.as_str().into()),
            ("privateEntry", self.private_entry.as_str().into()),
        ])
    }
}

impl R2Packet {
    /// The packet as Python's dict, key for key in the same order, for the recorders.
    pub fn to_value(&self) -> Value {
        dict([
            ("state", self.state.as_str().into()),
            ("speedLimit", self.speed_limit.into()),
            (
                "proximity",
                self.proximity
                    .as_ref()
                    .map(Proximity::to_value)
                    .unwrap_or(Value::Null),
            ),
            ("oneWay", self.one_way.into()),
            ("roadDirections", floats(&self.road_directions)),
            (
                "offRoad",
                self.off_road
                    .as_ref()
                    .map(OffRoad::to_value)
                    .unwrap_or(Value::Null),
            ),
            (
                "correction",
                self.correction
                    .as_ref()
                    .map(Correction::to_value)
                    .unwrap_or(Value::Null),
            ),
            (
                "diagnostic",
                self.diagnostic
                    .clone()
                    .map(Value::Dict)
                    .unwrap_or(Value::Null),
            ),
            (
                "junction",
                self.junction
                    .as_ref()
                    .map(Junction::to_value)
                    .unwrap_or(Value::Null),
            ),
            (
                "signal",
                self.signal
                    .as_ref()
                    .map(Signal::to_value)
                    .unwrap_or(Value::Null),
            ),
            ("signalStatus", self.signal_status.as_str().into()),
            ("signalContext", self.signal_context.as_str().into()),
            ("orientation", self.orientation.into()),
        ])
    }
}

// ---------------------------------------------------------------------------------
// Validation

fn verr(msg: impl Into<String>) -> PyErr {
    PyErr::ValueError(msg.into())
}

/// `_finite_number`: any `float()`-able non-bool, finite. A missing key is Python's
/// `None`.
fn finite_number(value: Option<&Value>, field: &str) -> Result<f64, PyErr> {
    let value = value.unwrap_or(&Value::Null);
    if matches!(value, Value::Bool(_)) {
        return Err(verr(format!("{field} must be a number")));
    }
    let number = match value.py_float() {
        Ok(n) => n,
        Err(PyErr::TypeError(_)) | Err(PyErr::ValueError(_)) => {
            return Err(verr(format!("{field} must be a number")));
        }
        Err(other) => return Err(other),
    };
    if !number.is_finite() {
        return Err(verr(format!("{field} must be finite")));
    }
    Ok(number)
}

fn bearing_of(value: Option<&Value>, field: &str) -> Result<f64, PyErr> {
    let value = finite_number(value, field)?;
    Ok(py_mod(value + 180.0, 360.0) - 180.0)
}

/// `max(0.0, x)`.
fn nonneg(x: f64) -> f64 {
    pyjson::py_max(0.0, x)
}

/// `_short_array`: BeamNG's Lua `jsonEncode` writes an empty array as `{}`.
fn short_array<'a>(value: &'a Value, field: &str, max_items: usize) -> Result<&'a [Value], PyErr> {
    if let Value::Dict(d) = value
        && d.is_empty()
    {
        return Ok(&[]);
    }
    match value {
        Value::List(l) if l.len() <= max_items => Ok(l),
        _ => Err(verr(format!("{field} must be a short array"))),
    }
}

/// `x in {...}` for a possibly missing value (missing is `None`, hashable).
fn in_set(value: Option<&Value>, set: &[&str]) -> Result<bool, PyErr> {
    match value {
        None => Ok(false),
        Some(v) => v.in_str_set(set),
    }
}

/// `d.get(key)` that treats JSON null as missing, as `is not None` checks do.
fn present<'a>(d: &'a Dict, key: &str) -> Option<&'a Value> {
    d.get(key).filter(|v| !v.is_null())
}

fn short_str(value: Option<&Value>, max: usize) -> Option<&str> {
    match value {
        Some(Value::Str(s)) if char_len(s) <= max => Some(s),
        _ => None,
    }
}

/// Parse and validate one R2 datagram.
///
/// `Err(ValueError)` on bad input, which the listener logs and skips. Python also
/// raises `TypeError` (an unhashable `state`, `junction.phase` or `correction.phase`)
/// and `OverflowError` (an integer too large for a float), which its listener does
/// not catch; those come back as the matching [`PyErr`] variant.
pub fn parse_r2_packet(text: &str) -> Result<R2Packet, PyErr> {
    let Some(body) = text.strip_prefix(R2_PREFIX) else {
        return Err(verr("not an R2 packet"));
    };
    let packet = loads(body).map_err(|_| verr("invalid R2 JSON"))?;
    let Value::Dict(packet) = packet else {
        return Err(verr("R2 payload must be an object"));
    };

    let state_value = packet.get("state");
    if !in_set(state_value, STATES)? {
        return Err(verr("invalid road state"));
    }
    let state = state_value
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let one_way = match packet.get("oneWay") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(verr("oneWay must be boolean")),
    };

    let empty = Value::List(Vec::new());
    let directions = short_array(
        packet.get("roadDirections").unwrap_or(&empty),
        "roadDirections",
        8,
    )?;
    let directions = directions
        .iter()
        .map(|v| bearing_of(Some(v), "roadDirections"))
        .collect::<Result<Vec<_>, _>>()?;

    let off_road = match present(&packet, "offRoad") {
        None => None,
        Some(Value::Dict(o)) => Some(OffRoad {
            bearing: bearing_of(o.get("bearing"), "offRoad.bearing")?,
            distance: nonneg(finite_number(o.get("distance"), "offRoad.distance")?),
        }),
        Some(_) => return Err(verr("offRoad must be an object or null")),
    };

    let correction = match present(&packet, "correction") {
        None => None,
        Some(c) => {
            let (Value::Dict(c), true) = (c, matches!(c.get("active"), Some(Value::Bool(_))))
            else {
                return Err(verr("correction must contain a boolean active field"));
            };
            let active = matches!(c.get("active"), Some(Value::Bool(true)));
            let default_phase = Value::from(if active { "correct" } else { "idle" });
            let phase = c.get("phase").unwrap_or(&default_phase);
            if !phase.in_str_set(CORRECTION_PHASES)? {
                return Err(verr("invalid correction phase"));
            }
            let settled = match c.get("settled") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(verr("correction.settled must be boolean")),
            };
            let zero = Value::Float(0.0);
            let bearing = bearing_of(
                Some(c.get("bearing").unwrap_or(&zero)),
                "correction.bearing",
            )?;
            let severity = finite_number(
                Some(c.get("severity").unwrap_or(&zero)),
                "correction.severity",
            )?;
            let mut out = Correction {
                active,
                bearing,
                severity: pyjson::py_min(1.0, nonneg(severity)),
                phase: phase.as_str().unwrap_or("").to_string(),
                settled,
                lateral_ratio: None,
                heading_error: None,
                time_to_edge: None,
            };
            if let Some(v) = present(c, "lateralRatio") {
                out.lateral_ratio = Some(finite_number(Some(v), "correction.lateralRatio")?);
            }
            if let Some(v) = present(c, "headingError") {
                out.heading_error = Some(finite_number(Some(v), "correction.headingError")?);
            }
            if let Some(v) = present(c, "timeToEdge") {
                out.time_to_edge = Some(finite_number(Some(v), "correction.timeToEdge")?);
            }
            out.lateral_ratio = out.lateral_ratio.map(nonneg);
            if let Some(h) = out.heading_error {
                out.heading_error = Some(bearing_of(
                    Some(&Value::Float(h)),
                    "correction.headingError",
                )?);
            }
            out.time_to_edge = out.time_to_edge.map(nonneg);
            Some(out)
        }
    };

    let diagnostic = match present(&packet, "diagnostic") {
        None => None,
        Some(Value::Dict(d)) => {
            let mut parsed = Dict::new();
            for field in DIAGNOSTIC_FLOATS {
                if let Some(v) = d.get(*field) {
                    let n = finite_number(Some(v), &format!("diagnostic.{field}"))?;
                    parsed.insert(field.to_string(), Value::Float(n));
                }
            }
            for field in DIAGNOSTIC_BOOLS {
                if let Some(v) = d.get(*field) {
                    let Value::Bool(b) = v else {
                        return Err(verr(format!("diagnostic.{field} must be boolean")));
                    };
                    parsed.insert(field.to_string(), Value::Bool(*b));
                }
            }
            if let Some(v) = present(d, "edgeId") {
                let Some(s) = short_str(Some(v), 200) else {
                    return Err(verr("diagnostic.edgeId must be a short string"));
                };
                parsed.insert("edgeId".into(), s.into());
            }
            if let Some(v) = present(d, "contactMaterials") {
                let Some(s) = short_str(Some(v), 500) else {
                    return Err(verr("diagnostic.contactMaterials must be a short string"));
                };
                parsed.insert("contactMaterials".into(), s.into());
            }
            Some(parsed)
        }
        Some(_) => return Err(verr("diagnostic must be an object or null")),
    };

    let junction = match present(&packet, "junction") {
        None => None,
        Some(Value::Dict(j)) => {
            let id = match j.get("id") {
                Some(Value::Str(s)) if !py_strip(s).is_empty() && char_len(s) <= 160 => s,
                _ => return Err(verr("junction.id must be a non-empty string")),
            };
            let phase = j.get("phase");
            if !in_set(phase, PHASES)? {
                return Err(verr("invalid junction phase"));
            }
            let entered = match j.get("entered") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(verr("junction.entered must be boolean")),
            };
            let exits = short_array(j.get("exits").unwrap_or(&empty), "junction.exits", 16)?;
            let kind = match j.get("kind") {
                None => "intersection",
                Some(Value::Str(s)) if !py_strip(s).is_empty() && char_len(s) <= 40 => s,
                Some(_) => return Err(verr("junction.kind must be a short string")),
            };
            let zero = Value::Float(0.0);
            let distance = nonneg(finite_number(
                Some(j.get("distance").unwrap_or(&zero)),
                "junction.distance",
            )?);
            let exits = exits
                .iter()
                .map(|v| bearing_of(Some(v), "junction.exits"))
                .collect::<Result<Vec<_>, _>>()?;
            Some(Junction {
                id: py_strip(id).to_string(),
                phase: phase.and_then(Value::as_str).unwrap_or("").to_string(),
                entered,
                kind: py_strip(kind).to_string(),
                distance,
                exits,
            })
        }
        Some(_) => return Err(verr("junction must be an object or null")),
    };

    let signal_status = match packet.get("signalStatus") {
        None => "unsupported",
        Some(Value::Str(s))
            if ["unsupported", "inactive", "none", "available"].contains(&s.as_str()) =>
        {
            s
        }
        Some(_) => return Err(verr("invalid signalStatus")),
    };
    let signal_context = match packet.get("signalContext") {
        None => "",
        Some(Value::Str(s)) if char_len(s) <= 200 => s,
        Some(_) => return Err(verr("invalid signalContext")),
    };
    let signal = match present(&packet, "signal") {
        Some(Value::Dict(s)) => {
            let id = match s.get("id") {
                Some(Value::Str(v)) if !v.is_empty() && char_len(v) <= 160 => v,
                _ => return Err(verr("invalid signal.id")),
            };
            let kind = match s.get("kind") {
                Some(Value::Str(k)) if k == "stopSign" || k == "trafficLight" => k,
                _ => return Err(verr("invalid signal kind or state")),
            };
            let sstate = match s.get("state") {
                Some(Value::Str(v)) if SIGNAL_STATES.contains(&v.as_str()) => v,
                _ => return Err(verr("invalid signal kind or state")),
            };
            if (kind == "stopSign") != (sstate == "stop") {
                return Err(verr("signal state does not match kind"));
            }
            let phase = match s.get("phase") {
                Some(Value::Str(p)) if PHASES.contains(&p.as_str()) => p,
                _ => return Err(verr("invalid signal.phase")),
            };
            let distance = finite_number(s.get("distance"), "signal.distance")?;
            if distance < 0.0 || distance > 1000.0 {
                return Err(verr("invalid signal.distance"));
            }
            if signal_status != "available" || state != "onRoad" {
                return Err(verr("signal requires an available on-road feed"));
            }
            let group_id = match s.get("groupId") {
                None => id,
                Some(Value::Str(g)) if !g.is_empty() && char_len(g) <= 160 => g,
                Some(_) => return Err(verr("invalid signal.groupId")),
            };
            let preview = match s.get("preview") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(verr("invalid signal.preview")),
            };
            if preview && kind == "trafficLight" && sstate != "unknown" {
                return Err(verr(
                    "preview cannot claim an approach-specific light state",
                ));
            }
            Some(Signal {
                id: id.clone(),
                kind: kind.clone(),
                state: sstate.clone(),
                phase: phase.clone(),
                distance,
                group_id: group_id.clone(),
                preview,
            })
        }
        Some(_) => return Err(verr("signal must be an object or null")),
        None if signal_status == "available" => {
            return Err(verr("available signal is missing"));
        }
        None => None,
    };

    let proximity = match present(&packet, "proximity") {
        None => None,
        Some(p) => {
            let Value::Dict(p) = p else {
                return Err(verr("proximity requires an on-road object"));
            };
            if state != "onRoad" {
                return Err(verr("proximity requires an on-road object"));
            }
            let id = match p.get("id") {
                Some(Value::Str(v)) if !v.is_empty() && char_len(v) <= 200 => v,
                _ => return Err(verr("invalid proximity.id")),
            };
            let action = p.get("action").and_then(Value::as_str);
            let source = p.get("source").and_then(Value::as_str);
            let (Some(action), Some(source)) = (
                action.filter(|a| *a == "stop" || *a == "approach"),
                source.filter(|s| *s == "stopPoint" || *s == "junctionBoundary"),
            ) else {
                return Err(verr("invalid proximity action or source"));
            };
            if source == "junctionBoundary" && action == "stop" {
                return Err(verr(
                    "junction geometry cannot establish a stop requirement",
                ));
            }
            let distance = finite_number(p.get("distance"), "proximity.distance")?;
            let speed = finite_number(p.get("speed"), "proximity.speed")?;
            if !(0.0..=1000.0).contains(&distance) || speed < 0.0 {
                return Err(verr("invalid proximity distance or speed"));
            }
            let signed = match p.get("signedDistance") {
                None => distance,
                Some(v) => finite_number(Some(v), "proximity.signedDistance")?,
            };
            if !(-50.0..=1000.0).contains(&signed) {
                return Err(verr("invalid proximity.signedDistance"));
            }
            let target_kind = match p.get("kind") {
                None => "",
                Some(Value::Str(k)) if char_len(k) <= 40 => k,
                Some(_) => return Err(verr("invalid proximity.kind")),
            };
            let private_entry = match p.get("privateEntry") {
                None => "",
                Some(Value::Str(k)) if char_len(k) <= 200 => k,
                Some(_) => return Err(verr("invalid proximity.privateEntry")),
            };
            Some(Proximity {
                id: id.clone(),
                distance,
                speed,
                action: action.to_string(),
                source: source.to_string(),
                signed_distance: signed,
                kind: target_kind.to_string(),
                private_entry: private_entry.to_string(),
            })
        }
    };

    let speed_limit = match present(&packet, "speedLimit") {
        None => None,
        Some(v) => {
            let limit = finite_number(Some(v), "speedLimit")?;
            if limit <= 0.0 || state != "onRoad" {
                return Err(verr("speedLimit requires a positive on-road value"));
            }
            Some(limit)
        }
    };

    Ok(R2Packet {
        state,
        speed_limit,
        proximity,
        one_way,
        road_directions: directions,
        off_road,
        correction,
        diagnostic,
        junction,
        signal,
        signal_status: signal_status.to_string(),
        signal_context: signal_context.to_string(),
        // Optional extension that re-arms orientation after a vehicle/world reload
        // even when the wire state stays onRoad throughout.
        orientation: matches!(packet.get("orientation"), Some(Value::Bool(true))),
    })
}

// ---------------------------------------------------------------------------------
// Wording

/// The travel-relative direction band for a signed bearing (positive LEFT).
pub fn direction_label(bearing: f64, u_turn: bool) -> Result<String, ValueError> {
    if !bearing.is_finite() {
        return Err(ValueError::new("bearing must be finite"));
    }
    let bearing = py_mod(bearing + 180.0, 360.0) - 180.0;
    let magnitude = bearing.abs();
    if magnitude <= 15.0 {
        return Ok("straight".to_string());
    }
    let side = if bearing > 0.0 { "left" } else { "right" };
    if magnitude <= 45.0 {
        return Ok(format!("slight {side}"));
    }
    if magnitude <= 120.0 {
        return Ok(side.to_string());
    }
    if magnitude <= 150.0 {
        return Ok(format!("sharp {side}"));
    }
    Ok(if u_turn { "U-turn" } else { "behind" }.to_string())
}

fn joined(items: &[String], conjunction: &str) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} {conjunction} {}", items[0], items[1]),
        n => format!(
            "{}, {conjunction} {}",
            items[..n - 1].join(", "),
            items[n - 1]
        ),
    }
}

/// `sorted(bearings, reverse=True)`: driver's left through right. Every element is
/// labelled afterwards, so a NaN raises whatever order it lands in.
fn sorted_desc(bearings: &[f64]) -> Vec<f64> {
    let mut v = bearings.to_vec();
    v.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Exit directions, left to right, without U-turns or repeats.
pub fn direction_list(bearings: &[f64], conjunction: &str) -> Result<String, ValueError> {
    let mut labels: Vec<String> = Vec::new();
    for bearing in sorted_desc(bearings) {
        let label = direction_label(bearing, true)?;
        if label != "U-turn" && !labels.contains(&label) {
            labels.push(label);
        }
    }
    Ok(joined(&labels, conjunction))
}

/// Legal road directions, left to right, without repeats.
pub fn road_direction_list(bearings: &[f64], conjunction: &str) -> Result<String, ValueError> {
    let mut labels: Vec<String> = Vec::new();
    for bearing in sorted_desc(bearings) {
        let label = direction_label(bearing, false)?;
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    Ok(joined(&labels, conjunction))
}

pub fn one_way_phrase(bearing: f64) -> String {
    if bearing.abs() <= 10.0 {
        return "One-way road. Legal travel is ahead".to_string();
    }
    let side = if bearing > 0.0 { "left" } else { "right" };
    format!(
        "One-way road. Legal travel is {} degrees to your {side}",
        py_f(bearing.abs(), 0)
    )
}

pub fn format_road_distance(metres: f64, units: &str) -> String {
    let value = pyjson::py_max(0.0, metres);
    if units.to_lowercase().starts_with("imp") {
        return format!("{} feet", py_f(value * 3.28084, 0));
    }
    format!("{} meters", py_f(value, 0))
}

pub fn junction_phrase(junction: &Junction, units: &str) -> Result<String, ValueError> {
    let kind = junction.kind.as_str();
    let distance = format_road_distance(junction.distance, units);
    let exits = direction_list(
        &junction.exits,
        if kind == "fork" || kind == "tJunction" {
            "or"
        } else {
            "and"
        },
    )?;
    if kind == "deadEnd" {
        return Ok(format!("Road ends in {distance}."));
    }
    // "gate" only where a gate mesh stands on the road. "private" is a road the map
    // keeps traffic off, almost always with nothing across it.
    if kind == "gate" {
        return Ok(format!("Gate in {distance}."));
    }
    if kind == "private" {
        return Ok(format!("Private road in {distance}."));
    }
    let lead = match kind {
        "fork" => "Road forks",
        "tJunction" => "T-junction",
        "crossroads" => "Crossroads",
        "complex" => "Complex intersection",
        _ => "Intersection",
    };
    if !exits.is_empty() {
        return Ok(format!("{lead} in {distance}: {exits}."));
    }
    Ok(format!("{lead} in {distance}."))
}

pub fn signal_phrase(signal: &Signal, units: &str, change: bool) -> String {
    if signal.preview {
        let control = if signal.kind == "stopSign" {
            "Stop sign"
        } else {
            "Traffic lights"
        };
        return format!(
            "{control} ahead, about {}.",
            format_road_distance(signal.distance, units)
        );
    }
    if signal.kind == "stopSign" {
        return format!(
            "Stop sign in {}.",
            format_road_distance(signal.distance, units)
        );
    }
    let state = match signal.state.as_str() {
        "red" => "red",
        "yellow" => "yellow",
        "green" => "green",
        "redYellow" => "red and yellow",
        "flashingRed" => "flashing red",
        "flashingYellow" => "flashing yellow",
        "flashingGreen" => "flashing green",
        "off" => "off",
        _ => "state unknown",
    };
    if change {
        return format!("Traffic light {state}.");
    }
    format!(
        "Traffic light {state}, in {}.",
        format_road_distance(signal.distance, units)
    )
}

/// `SURFACE|Dirt|1` from roadDetector: the new surface and whether it is a mapped
/// road. `None` for anything malformed. Optional tails may follow.
///
/// `isalpha` is approximated by `char::is_alphabetic`, which also admits a few
/// combining marks Python's Unicode letter categories do not.
pub fn parse_surface_packet(text: &str) -> Option<(String, bool)> {
    let parts: Vec<&str> = text.split('|').collect();
    if parts.len() < 3 || parts[0] != "SURFACE" {
        return None;
    }
    let word = parts[1];
    if word.is_empty()
        || !word.chars().all(char::is_alphabetic)
        || char_len(word) > 20
        || (parts[2] != "0" && parts[2] != "1")
    {
        return None;
    }
    Some((word.to_string(), parts[2] == "1"))
}

pub fn surface_phrase(word: &str, on_road: bool) -> String {
    if on_road {
        format!("{word} road")
    } else {
        word.to_string()
    }
}

/// The map limit, or "unavailable". `label` is `"Map speed limit"` for the automatic
/// announcement and `"Speed limit"` for F9 S.
pub fn map_speed_limit_phrase(speed_ms: Option<f64>, units: &str, label: &str) -> String {
    let Some(speed_ms) = speed_ms else {
        return format!("{label} unavailable.");
    };
    let value = py_round0(
        speed_ms
            * if units == "imperial" {
                2.2369362920544
            } else {
                3.6
            },
    );
    let unit = if units == "imperial" {
        "mph"
    } else {
        "kilometers per hour"
    };
    format!("{label} {value} {unit}.")
}

// ---------------------------------------------------------------------------------
// The feed

/// The legacy (pre-R2) road detector's last datagram.
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyState {
    pub state: String,
    pub bearing: f64,
    pub distance: f64,
    pub directions: Vec<f64>,
}

/// A signal announcement: the signal plus whether its state changed.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalEvent {
    pub signal: Signal,
    pub change: bool,
}

/// `{"value": limit}`; `value` is `None` when the limit became unknown.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedLimitEvent {
    pub value: Option<f64>,
}

/// What `accept_r2` returns.
#[derive(Debug, Clone, PartialEq)]
pub struct R2Events {
    pub orientation: bool,
    pub junction: Option<Junction>,
    pub signal: Option<SignalEvent>,
    pub speed_limit: Option<SpeedLimitEvent>,
}

/// What `snapshot` returns.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedSnapshot {
    /// `"unavailable"`, `"r2"`, `"legacy"` or `"stale"`.
    pub mode: String,
    pub packet: Option<R2Packet>,
    pub legacy: Option<LegacyState>,
    pub junction: Option<Junction>,
}

impl LegacyState {
    /// `legacy_state` as the Python dict, in its key order.
    pub fn to_value(&self) -> Value {
        dict([
            ("state", self.state.as_str().into()),
            ("bearing", self.bearing.into()),
            ("distance", self.distance.into()),
            ("directions", floats(&self.directions)),
        ])
    }
}

impl FeedSnapshot {
    /// The dict Python's `snapshot()` returns, in its key order.
    pub fn to_value(&self) -> Value {
        dict([
            ("mode", self.mode.as_str().into()),
            (
                "packet",
                self.packet
                    .as_ref()
                    .map(R2Packet::to_value)
                    .unwrap_or(Value::Null),
            ),
            (
                "legacy",
                self.legacy
                    .as_ref()
                    .map(LegacyState::to_value)
                    .unwrap_or(Value::Null),
            ),
            (
                "junction",
                self.junction
                    .as_ref()
                    .map(Junction::to_value)
                    .unwrap_or(Value::Null),
            ),
        ])
    }
}

struct FeedState {
    mode: String,
    packet: Option<R2Packet>,
    last_r2_time: Option<f64>,
    last_legacy_time: Option<f64>,
    legacy_state: Option<LegacyState>,
    last_state: Option<String>,
    orientation_armed: bool,
    junction_id: Option<String>,
    junction_missing: u64,
    announced_phases: Vec<(String, String)>,
    last_known_junction: Option<Junction>,
    signal_context: Option<String>,
    /// `(kind, groupId) -> (state, near, time)`.
    signal_history: IndexMap<(String, String), (String, bool, f64)>,
    signal_options: Option<(bool, bool)>,
    speed_context: Option<(String, bool)>,
    speed_candidate: Option<f64>,
    speed_since: Option<f64>,
    speed_announced: Option<f64>,
}

impl FeedState {
    fn new() -> Self {
        FeedState {
            mode: "unavailable".to_string(),
            packet: None,
            last_r2_time: None,
            last_legacy_time: None,
            legacy_state: None,
            last_state: None,
            orientation_armed: true,
            junction_id: None,
            junction_missing: 0,
            announced_phases: Vec::new(),
            last_known_junction: None,
            signal_context: None,
            signal_history: IndexMap::new(),
            signal_options: None,
            speed_context: None,
            speed_candidate: None,
            speed_since: None,
            speed_announced: None,
        }
    }

    fn speed_limit_event(
        &mut self,
        packet: &R2Packet,
        now: f64,
        enabled: bool,
    ) -> Option<SpeedLimitEvent> {
        let context = (packet.signal_context.clone(), enabled);
        if Some(&context) != self.speed_context.as_ref()
            || self.last_r2_time.is_none_or(|t| now - t > R2_STALE_SECONDS)
        {
            self.speed_candidate = None;
            self.speed_announced = None;
            self.speed_since = None;
        }
        self.speed_context = Some(context);
        if !enabled {
            return None;
        }
        let value = if packet.state == "onRoad" {
            packet.speed_limit
        } else {
            None
        };
        let value = value.map(|v| py_round(v, 3));
        if value != self.speed_candidate || self.speed_since.is_none() {
            self.speed_candidate = value;
            self.speed_since = Some(now);
        }
        // Edge selection can flicker at junctions. A brief missing sample must
        // neither repeat the old limit nor announce that it disappeared.
        let dwell = if value.is_some() { 0.75 } else { 3.0 };
        let since = self.speed_since.unwrap_or(now);
        if now - since < dwell || value == self.speed_announced {
            return None;
        }
        self.speed_announced = value;
        if packet.state != "onRoad" {
            return None;
        }
        Some(SpeedLimitEvent { value })
    }
}

/// Thread-safe R2/legacy feed state and once-per-junction event tracking.
pub struct RoadGuidanceFeed {
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    state: Mutex<FeedState>,
}

impl Default for RoadGuidanceFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl RoadGuidanceFeed {
    /// A feed on `time.monotonic()`.
    pub fn new() -> Self {
        Self::with_clock(pytime::monotonic)
    }

    pub fn with_clock(clock: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        RoadGuidanceFeed {
            clock: Box::new(clock),
            state: Mutex::new(FeedState::new()),
        }
    }

    pub fn reset(&self) {
        *self.state.lock() = FeedState::new();
    }

    /// Reintroduce manual road guidance without forgetting shared alerts.
    pub fn arm_orientation(&self) {
        let mut s = self.state.lock();
        s.orientation_armed = true;
        s.last_state = None;
    }

    /// `accept_r2(packet)` with every option on and the feed's own clock.
    pub fn accept_r2_default(&self, packet: &R2Packet) -> R2Events {
        self.accept_r2(packet, None, true, true, true)
    }

    pub fn accept_r2(
        &self,
        packet: &R2Packet,
        now: Option<f64>,
        stop_signs: bool,
        traffic_lights: bool,
        speed_limits: bool,
    ) -> R2Events {
        let now = now.unwrap_or_else(|| (self.clock)());
        let mut s = self.state.lock();
        let speed_event = s.speed_limit_event(packet, now, speed_limits);
        let context = packet.signal_context.clone();
        let options = (stop_signs, traffic_lights);
        if Some(&context) != s.signal_context.as_ref()
            || Some(options) != s.signal_options
            || s.last_r2_time.is_none_or(|t| now - t > R2_STALE_SECONDS)
        {
            s.signal_history.clear();
        }
        s.signal_context = Some(context);
        s.signal_options = Some(options);
        let previous_state = s.last_state.clone();
        s.mode = "r2".to_string();
        s.last_r2_time = Some(now);
        s.packet = Some(packet.clone());
        s.last_state = Some(packet.state.clone());

        let mut orientation = false;
        if packet.state == "onRoad" {
            orientation = packet.orientation
                || (s.orientation_armed && previous_state.as_deref() != Some("onRoad"));
            if orientation {
                s.orientation_armed = false;
            }
        } else if packet.state == "offRoad" {
            s.orientation_armed = true;
        }

        let mut junction_event = None;
        match &packet.junction {
            None => {
                s.junction_missing += 1;
                if s.junction_missing >= 3 {
                    s.junction_id = None;
                    s.announced_phases.clear();
                    s.last_known_junction = None;
                }
            }
            Some(junction) => {
                s.junction_missing = 0;
                if s.junction_id.as_deref() != Some(junction.id.as_str()) {
                    s.junction_id = Some(junction.id.clone());
                    s.announced_phases.clear();
                }
                s.last_known_junction = Some(junction.clone());
                let event_phase = if junction.entered {
                    "entered".to_string()
                } else {
                    junction.phase.clone()
                };
                let phase_key = (junction.id.clone(), event_phase.clone());
                if !s.announced_phases.contains(&phase_key) {
                    s.announced_phases.push(phase_key);
                    let mut event = junction.clone();
                    event.phase = event_phase;
                    junction_event = Some(event);
                }
            }
        }

        let mut signal_event = None;
        // Brief edge-selection gaps must not repeat an approach announcement.
        s.signal_history.retain(|_, v| now - v.2 < 5.0);
        if let Some(signal) = &packet.signal {
            let wanted = if signal.kind == "stopSign" {
                stop_signs
            } else {
                traffic_lights
            };
            if wanted {
                let key = (signal.kind.clone(), signal.group_id.clone());
                let previous = s.signal_history.get(&key).cloned();
                let near = signal.phase == "near";
                let changed = previous.as_ref().is_some_and(|p| p.0 != signal.state);
                let previous_near = previous.as_ref().is_some_and(|p| p.1);
                if previous.is_none() || changed || (near && !previous_near) {
                    signal_event = Some(SignalEvent {
                        signal: signal.clone(),
                        change: changed,
                    });
                }
                s.signal_history
                    .insert(key, (signal.state.clone(), near || previous_near, now));
            }
        }

        R2Events {
            orientation,
            junction: junction_event,
            signal: signal_event,
            speed_limit: speed_event,
        }
    }

    pub fn r2_recent(&self, now: Option<f64>) -> bool {
        let now = now.unwrap_or_else(|| (self.clock)());
        let s = self.state.lock();
        s.last_r2_time.is_some_and(|t| now - t <= R2_STALE_SECONDS)
    }

    /// Returns whether the legacy feed is now the one in use.
    pub fn accept_legacy(
        &self,
        state: &str,
        bearing: f64,
        distance: f64,
        directions: &[f64],
        now: Option<f64>,
    ) -> bool {
        let now = now.unwrap_or_else(|| (self.clock)());
        let mut s = self.state.lock();
        s.last_legacy_time = Some(now);
        s.legacy_state = Some(LegacyState {
            state: state.to_string(),
            bearing,
            distance: pyjson::py_max(0.0, distance),
            directions: directions.to_vec(),
        });
        if s.last_r2_time.is_none_or(|t| now - t > R2_STALE_SECONDS) {
            s.mode = "legacy".to_string();
            return true;
        }
        false
    }

    /// Update stale/fallback state. True only on a new R2 timeout.
    pub fn check_timeout(&self, now: Option<f64>) -> bool {
        let now = now.unwrap_or_else(|| (self.clock)());
        let mut s = self.state.lock();
        let Some(last) = s.last_r2_time else {
            return false;
        };
        if now - last <= R2_STALE_SECONDS {
            return false;
        }
        let newly_stale = s.mode == "r2";
        let legacy_recent = s
            .last_legacy_time
            .is_some_and(|t| now - t <= R2_STALE_SECONDS);
        s.mode = if legacy_recent { "legacy" } else { "stale" }.to_string();
        if newly_stale {
            s.signal_history.clear();
            s.packet = None;
            s.last_state = None;
            s.orientation_armed = true;
            s.last_known_junction = None;
        }
        newly_stale
    }

    pub fn snapshot(&self) -> FeedSnapshot {
        let s = self.state.lock();
        FeedSnapshot {
            mode: s.mode.clone(),
            packet: s.packet.clone(),
            legacy: s.legacy_state.clone(),
            junction: s.last_known_junction.clone(),
        }
    }

    /// Current map limit in m/s, or `None` when off, stale, off-road or unmapped.
    pub fn speed_limit(&self, enabled: bool) -> Option<f64> {
        let s = self.state.lock();
        let recent = s
            .last_r2_time
            .is_some_and(|t| (self.clock)() - t <= R2_STALE_SECONDS);
        match &s.packet {
            Some(p) if enabled && recent && p.state == "onRoad" => p.speed_limit,
            _ => None,
        }
    }

    pub fn speed_limit_phrase(&self, enabled: bool, units: &str, label: &str) -> String {
        map_speed_limit_phrase(self.speed_limit(enabled), units, label)
    }

    /// The F9 road status sentence. `Err` where Python's `direction_label` raises (a
    /// legacy OFF_ROAD datagram whose bearing parsed as NaN or infinity).
    pub fn status_phrase(&self, enabled: bool, units: &str) -> Result<String, ValueError> {
        if !enabled {
            return Ok("Road guidance is off.".to_string());
        }
        let snap = self.snapshot();
        match snap.mode.as_str() {
            "unavailable" => return Ok("Road guidance feed is unavailable.".to_string()),
            "stale" => return Ok("Road guidance feed is stale.".to_string()),
            "legacy" => {
                let Some(legacy) = snap.legacy else {
                    return Ok(
                        "Legacy road detector only; enhanced guidance is unavailable.".to_string(),
                    );
                };
                if legacy.state == "DORMANT" {
                    return Ok(
                        "No roads detected on this map. Legacy road detector only.".to_string()
                    );
                }
                if legacy.state == "OFF_ROAD" {
                    let side = direction_label(legacy.bearing, false)?;
                    let dist = format_road_distance(legacy.distance, units);
                    return Ok(format!(
                        "Off road. Road {side}, {dist} away. Legacy guidance only."
                    ));
                }
                return Ok(
                    "On road. Legacy road detector only; enhanced guidance is unavailable."
                        .to_string(),
                );
            }
            _ => {}
        }

        let Some(packet) = snap.packet else {
            // `packet or {}`: no state, so it falls through to the on-road report
            // with every field at its default.
            return self.on_road_status(&R2Packet::default(), None, enabled, units, true);
        };
        if packet.state == "dormant" {
            return Ok("No roads detected on this map.".to_string());
        }
        if packet.state == "offRoad" {
            let Some(off_road) = &packet.off_road else {
                return Ok(
                    "Off road. No vertically compatible road found within search range."
                        .to_string(),
                );
            };
            let side = direction_label(off_road.bearing, false)?;
            let dist = format_road_distance(off_road.distance, units);
            return Ok(format!("Off road. Road {side}, {dist} away."));
        }
        self.on_road_status(&packet, snap.junction.as_ref(), enabled, units, false)
    }

    fn on_road_status(
        &self,
        packet: &R2Packet,
        junction: Option<&Junction>,
        enabled: bool,
        units: &str,
        empty_packet: bool,
    ) -> Result<String, ValueError> {
        let mut parts = vec!["On road.".to_string()];
        parts.push(self.speed_limit_phrase(enabled, units, "Map speed limit"));
        let directions = road_direction_list(&packet.road_directions, "or")?;
        if packet.one_way {
            parts.push("One-way road.".to_string());
        }
        if !directions.is_empty() {
            let plural = if !packet.one_way { "s" } else { "" };
            parts.push(format!("Legal direction{plural}: {directions}."));
        }
        match &packet.correction {
            Some(c) if c.active => {
                if c.phase == "unwind" {
                    parts.push("Straighten steering now.".to_string());
                } else {
                    let side = if c.bearing > 0.0 { "left" } else { "right" };
                    let strength = if c.severity >= 0.67 {
                        "strong"
                    } else if c.severity >= 0.34 {
                        "moderate"
                    } else {
                        "small"
                    };
                    parts.push(format!("Apply a {strength} correction {side}."));
                }
            }
            _ => parts.push("No correction needed.".to_string()),
        }
        if let Some(signal) = &packet.signal {
            parts.push(signal_phrase(signal, units, false));
        } else if empty_packet || packet.signal_status == "unsupported" {
            parts.push("Traffic control data unavailable.".to_string());
        } else if packet.signal_status == "inactive" {
            parts.push("Traffic signal system inactive.".to_string());
        } else {
            parts.push("No stop sign or traffic light reported ahead.".to_string());
        }
        if let Some(junction) = junction {
            parts.push(format!("Next, {}", junction_phrase(junction, units)?));
        } else {
            parts.push("No intersection reported ahead.".to_string());
        }
        Ok(parts.join(" "))
    }
}
