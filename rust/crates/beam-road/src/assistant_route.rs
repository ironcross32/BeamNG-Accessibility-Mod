//! Spoken guidance while the driving assistant follows a map route, from
//! assistant_route.py.
//!
//! No engine, audio or speech dependencies, so the callout rules and wording can be
//! exercised on their own. Distances arrive in metres; units are decided only here,
//! at the speech boundary.
//!
//! Reports are the decoded JSON dicts the mod sends, so a field of the wrong type
//! fails the way Python's would: `TypeError`/`ValueError` (which the assistant
//! swallows) or `AttributeError` (which it does not).

use beam_base::pyfmt::{py_f, py_round, py_round0, py_strip};
use beam_base::route_beacon::format_long_distance;

use crate::pyjson::{PyErr, Value, py_eq, py_max};

const FEET_PER_METRE: f64 = 3.28084;
const METRES_PER_MILE: f64 = 1609.344;

/// Largest first. Each is spoken at most once per stretch of road.
fn imperial_callouts() -> [(f64, &'static str); 5] {
    [
        (2.0 * METRES_PER_MILE, "2 miles"),
        (METRES_PER_MILE, "1 mile"),
        (0.5 * METRES_PER_MILE, "half a mile"),
        (1000.0 / FEET_PER_METRE, "1000 feet"),
        (500.0 / FEET_PER_METRE, "500 feet"),
    ]
}
const METRIC_CALLOUTS: [(f64, &str); 4] = [
    (2000.0, "2 kilometers"),
    (1000.0, "1 kilometer"),
    (500.0, "500 meters"),
    (200.0, "200 meters"),
];

/// "Turn left now" comes this early: three seconds' notice, never under 40 m.
const NOW_MIN_M: f64 = 40.0;
const NOW_SECONDS: f64 = 3.0;
/// A callout just under the distance already spoken would only repeat it.
const CALLOUT_MARGIN: f64 = 0.8;

/// Only the two toasts the assistant says better itself, matched by wording and by
/// key because the UI bridge can deliver either (and only in this language).
const ARRIVAL_TOASTS: [&str; 2] = [
    "you have reached your destination",
    "bigmap.info.reachedtarget",
];
const U_TURN_TOASTS: [&str; 2] = [
    "please make a u-turn when possible",
    "ui.groundmarkers.wrongdirection",
];
const ARRIVAL_TOAST_WINDOW_S: f64 = 3.0;

fn imperial(units: &str) -> bool {
    units.to_lowercase().starts_with("imp")
}

/// `max(50, round(x / 50.0) * 50)` formatted with `:.0f`.
fn round_to_fifty(x: f64) -> String {
    let n = py_round0(x / 50.0) * 50;
    let n = if 50 > n { 50 } else { n };
    py_f(n as f64, 0)
}

/// A distance fit to speak: no "1437 feet", and no "1.0 miles".
pub fn spoken_distance(metres: f64, units: &str) -> String {
    let value = py_max(0.0, metres);
    if imperial(units) {
        let feet = value * FEET_PER_METRE;
        if feet < 1000.0 {
            return format!("{} feet", round_to_fifty(feet));
        }
        let miles = feet / 5280.0;
        if py_round(miles, 1) == 1.0 {
            return "1 mile".to_string();
        }
    } else if value < 1000.0 {
        return format!("{} meters", round_to_fifty(value));
    } else if py_round(value / 1000.0, 1) == 1.0 {
        return "1 kilometer".to_string();
    }
    format_long_distance(value, units).replace(".0 ", " ")
}

/// `left` -> `turn left`; forks and roundabouts have their own verbs.
pub fn turn_instruction(name: &str) -> String {
    if name == "roundabout" {
        return "take the roundabout".to_string();
    }
    if name.starts_with("keep") {
        return name.to_string();
    }
    format!("turn {name}")
}

/// [`turn_instruction`] on a decoded value: a non-string is Python's
/// `AttributeError` from `name.startswith`.
fn turn_instruction_value(name: &Value) -> Result<String, PyErr> {
    match name {
        Value::Str(s) => Ok(turn_instruction(s)),
        other => Err(PyErr::AttributeError(format!(
            "'{}' object has no attribute 'startswith'",
            other.type_name()
        ))),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Positive is the driver's left.
pub fn relative_direction(bearing: f64) -> String {
    let size = bearing.abs();
    if size < 5.0 {
        return "straight ahead".to_string();
    }
    if size > 175.0 {
        return "directly behind you".to_string();
    }
    format!(
        "{} degrees to your {}",
        py_f(size, 0),
        if bearing > 0.0 { "left" } else { "right" }
    )
}

/// `bearing > 0` on a decoded value: numbers compare, anything else is a `TypeError`.
fn positive(v: &Value) -> Result<bool, PyErr> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Int(i) => Ok(*i > 0),
        Value::BigInt(t) => Ok(!t.starts_with('-')),
        Value::Float(f) => Ok(*f > 0.0),
        other => Err(PyErr::TypeError(format!(
            "'>' not supported between instances of '{}' and 'int'",
            other.type_name()
        ))),
    }
}

fn relative_direction_value(bearing: &Value) -> Result<String, PyErr> {
    let size = bearing.py_float()?.abs();
    if size < 5.0 {
        return Ok("straight ahead".to_string());
    }
    if size > 175.0 {
        return Ok("directly behind you".to_string());
    }
    Ok(format!(
        "{} degrees to your {}",
        py_f(size, 0),
        if positive(bearing)? { "left" } else { "right" }
    ))
}

/// The sentence that ends a route. `arrival` is the decoded `arrival` dict.
pub fn arrival_phrase(arrival: &Value, units: &str) -> Result<String, PyErr> {
    let off_road = arrival.py_get("offRoad")?.is_some_and(Value::truthy);
    let distance = arrival.get("distance").filter(|v| !v.is_null());
    let bearing = arrival.get("bearing").filter(|v| !v.is_null());
    if let (true, Some(distance), Some(bearing)) = (off_road, distance, bearing) {
        return Ok(format!(
            "Destination off road, {}, {}. Waypoint marked. Assistant off",
            spoken_distance(distance.py_float()?, units),
            relative_direction_value(bearing)?
        ));
    }
    Ok("Destination reached. Assistant off".to_string())
}

/// Whether a stock toast would only contradict or talk over the assistant.
///
/// The arrival toast lands just after the assistant's own arrival sentence and would
/// interrupt it. The U-turn toast is simply wrong while following a route: that route
/// is forward only, and goes round the block rather than turning in the road.
pub fn suppressed_toast(
    text: &str,
    following_route: bool,
    seconds_since_arrival: Option<f64>,
) -> bool {
    let lowered = py_strip(text).to_lowercase();
    if let Some(s) = seconds_since_arrival
        && s <= ARRIVAL_TOAST_WINDOW_S
        && ARRIVAL_TOASTS.iter().any(|t| lowered.contains(t))
    {
        return true;
    }
    following_route && U_TURN_TOASTS.iter().any(|t| lowered.contains(t))
}

/// Which piece of road is being counted down.
#[derive(Debug, Clone)]
enum Target {
    Turn(Value),
    Destination,
}

impl Target {
    fn same(&self, other: &Target) -> bool {
        match (self, other) {
            (Target::Destination, Target::Destination) => true,
            (Target::Turn(a), Target::Turn(b)) => py_eq(a, b),
            _ => false,
        }
    }
}

/// `float(report.get(key) or 0.0)`.
fn float_or_zero(report: &Value, key: &str) -> Result<f64, PyErr> {
    match report.get(key) {
        Some(v) if v.truthy() => v.py_float(),
        _ => Ok(0.0),
    }
}

/// Decides what to say from the assistant's twice-a-second route reports.
pub struct RouteCallouts {
    units: Box<dyn Fn() -> String + Send + Sync>,
    pub following: bool,
    target: Option<Target>,
    callouts: Vec<(f64, &'static str)>,
    now_said: bool,
}

impl RouteCallouts {
    /// `units` is asked each time, so a units change applies to the next sentence.
    pub fn new(units: impl Fn() -> String + Send + Sync + 'static) -> Self {
        RouteCallouts {
            units: Box::new(units),
            following: false,
            target: None,
            callouts: Vec::new(),
            now_said: false,
        }
    }

    /// `RouteCallouts()`: imperial.
    pub fn imperial() -> Self {
        Self::new(|| "imperial".to_string())
    }

    pub fn reset(&mut self) {
        self.following = false;
        self.target = None;
        self.callouts.clear();
        self.now_said = false;
    }

    fn begin_stretch(&mut self, target: Target, distance: f64) {
        self.target = Some(target);
        self.now_said = false;
        let table: Vec<(f64, &'static str)> = if imperial(&(self.units)()) {
            imperial_callouts().to_vec()
        } else {
            METRIC_CALLOUTS.to_vec()
        };
        self.callouts = table
            .into_iter()
            .filter(|row| row.0 < distance * CALLOUT_MARGIN)
            .collect();
    }

    fn stretch(report: &Value) -> Result<(Target, f64), PyErr> {
        let turn_id = report.py_get("turnId")?.filter(|v| !v.is_null());
        let turn_distance = report.get("turnDistance").filter(|v| !v.is_null());
        if let (Some(id), Some(d)) = (turn_id, turn_distance) {
            return Ok((Target::Turn(id.clone()), d.py_float()?));
        }
        Ok((Target::Destination, float_or_zero(report, "destDistance")?))
    }

    fn now_distance(report: &Value) -> Result<f64, PyErr> {
        Ok(py_max(NOW_MIN_M, float_or_zero(report, "speed")? * NOW_SECONDS))
    }

    fn continue_phrase(&mut self, report: &Value, distance: f64) -> Result<String, PyErr> {
        let units = (self.units)();
        if let Some(turn) = report.get("turn").filter(|v| v.truthy()) {
            if distance <= Self::now_distance(report)? {
                self.now_said = true;
                self.callouts.clear();
                return Ok(format!("{} now", capitalised(&turn_instruction_value(turn)?)));
            }
            return Ok(format!(
                "Continue {}, then {}",
                spoken_distance(distance, &units),
                turn_instruction_value(turn)?
            ));
        }
        let place = if report.get("offRoad").is_some_and(Value::truthy) {
            "the closest road point"
        } else {
            "the destination"
        };
        Ok(format!(
            "Continue {} to {place}",
            spoken_distance(distance, &units)
        ))
    }

    /// A new route: the whole distance, then the first thing to do on it.
    pub fn route_set(&mut self, report: &Value) -> Result<String, PyErr> {
        self.following = true;
        let (target, distance) = Self::stretch(report)?;
        self.begin_stretch(target, distance);
        let units = (self.units)();
        let total = spoken_distance(float_or_zero(report, "destDistance")?, &units);
        let mut parts = vec![format!("Route set. {total} by road.")];
        if report.get("offRoad").is_some_and(Value::truthy) {
            parts.push(
                "Destination is off the road. Going to the closest road point.".to_string(),
            );
        }
        parts.push(self.continue_phrase(report, distance)?);
        Ok(parts.join(" "))
    }

    /// The sentence this report calls for, or `None`.
    pub fn update(&mut self, report: &Value) -> Result<Option<String>, PyErr> {
        if !self.following {
            // A receiver that missed route_set picks up the count without a speech.
            self.following = true;
            let (target, distance) = Self::stretch(report)?;
            self.begin_stretch(target, distance);
            return Ok(None);
        }
        let (target, distance) = Self::stretch(report)?;
        let same = self.target.as_ref().is_some_and(|t| t.same(&target));
        if !same {
            self.begin_stretch(target, distance);
            return self.continue_phrase(report, distance).map(Some);
        }
        let turn = report.get("turn").filter(|v| v.truthy());
        if let Some(turn) = turn
            && !self.now_said
            && distance <= Self::now_distance(report)?
        {
            self.now_said = true;
            self.callouts.clear();
            return Ok(Some(format!(
                "{} now",
                capitalised(&turn_instruction_value(turn)?)
            )));
        }
        let mut passed = None;
        while !self.callouts.is_empty() && distance <= self.callouts[0].0 {
            passed = Some(self.callouts.remove(0).1);
        }
        let Some(passed) = passed else {
            return Ok(None);
        };
        if let Some(turn) = turn {
            return Ok(Some(format!(
                "In {passed}, {}",
                turn_instruction_value(turn)?
            )));
        }
        let what = if report.get("offRoad").is_some_and(Value::truthy) {
            "Closest road point"
        } else {
            "Destination"
        };
        Ok(Some(format!("{what} in {passed}")))
    }
}
