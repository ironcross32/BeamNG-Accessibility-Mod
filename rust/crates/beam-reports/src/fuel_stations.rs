//! Parsing and speech wording for fuelStations.lua (F9 then Shift+F).
//!
//! Pure helpers: no sockets, no state. The engine owns the listener and the browser.
//! Rows and events are what `json.loads` produced, with Python's dynamic rules applied
//! through [`crate::pyops`].

use beam_base::pyfmt::py_mod;
use beam_base::route_beacon::format_long_distance;

use crate::pyops::{PyDict, PyErr, PyErrKind, PyValue, int_text_to_i128, json_loads, py_max};

pub const ROW_PREFIX: &str = "FS:";
pub const EVENT_PREFIX: &str = "FS_EVENT:";

/// `_ENERGY_WORDS`.
pub const ENERGY_WORDS: [(&str, &str); 7] = [
    ("gasoline", "gasoline"),
    ("diesel", "diesel"),
    ("kerosine", "kerosene"),
    ("n2o", "nitrous"),
    ("electricEnergy", "EV charging"),
    ("any", "any fuel"),
    ("fuel", "fuel"),
];

/// One validated station row (Python's dict of the same keys).
#[derive(Debug, Clone, PartialEq)]
pub struct StationRow {
    pub id: String,
    pub name: String,
    pub official: bool,
    pub energy: Vec<String>,
    pub distance: f64,
    pub bearing: f64,
    /// `int(row.get("pumps") or 0)`; Python's int is unbounded, this saturates at
    /// `i128`.
    pub pumps: i128,
    /// The raw value; `None` when missing or null. Only `False` means "not for your
    /// vehicle".
    pub compatible: PyValue,
    pub target: bool,
}

/// `json.loads`, with Python's `except json.JSONDecodeError` turning a decode error into
/// the caller's `ValueError`. Other exceptions (nesting too deep) pass through.
fn loads(body: &str, msg: &str) -> Result<PyValue, PyErr> {
    json_loads(body).map_err(|e| {
        if e.kind == PyErrKind::ValueError {
            PyErr::value(msg)
        } else {
            e
        }
    })
}

/// One `FS:` station row, validated.
///
/// `Err` with kind `ValueError` and the Python message for every rejection the listener
/// logs ("Malformed fuel station row: {msg}"). `pumps` that `int()` cannot take raises
/// `TypeError`/`ValueError`/`OverflowError` with Python's own message, and a distance or
/// bearing given as an int too large for a float raises `OverflowError`; those escaped
/// the listener's `except ValueError` in Python unless they were `ValueError`s.
pub fn parse_row(packet: &str) -> Result<StationRow, PyErr> {
    let Some(body) = packet.strip_prefix(ROW_PREFIX) else {
        return Err(PyErr::value("not a station row"));
    };
    let row = loads(body, "invalid station JSON")?;
    let row = match row {
        PyValue::Dict(d) if d.get("id").is_some_and(PyValue::truthy) => d,
        _ => return Err(PyErr::value("station row has no id")),
    };
    let zero = PyValue::Float(0.0);
    let nav = |key: &str| -> Result<f64, PyErr> {
        row.get(key).unwrap_or(&zero).py_float().map_err(|e| {
            if e.is_type_or_value() {
                PyErr::value("station row has invalid navigation values")
            } else {
                e
            }
        })
    };
    let distance = nav("distance")?;
    let bearing = nav("bearing")?;
    if !(distance.is_finite() && bearing.is_finite()) {
        return Err(PyErr::value("station row has non-finite navigation values"));
    }
    let energy = match row.get("energy") {
        Some(PyValue::List(items)) => items.iter().map(PyValue::py_str).collect(),
        _ => Vec::new(),
    };
    let id = row.get("id").map(PyValue::py_str).unwrap_or_default();
    let name = match row.get("name").filter(|v| v.truthy()) {
        Some(v) => v.py_str(),
        None => "Fuel station".to_string(),
    };
    let official = row.get("official").is_none_or(PyValue::truthy);
    let pumps_value = match row.get("pumps").filter(|v| v.truthy()) {
        Some(v) => v.clone(),
        None => PyValue::Int("0".into()),
    };
    let pumps_text = pumps_value.py_int()?;
    let pumps = int_text_to_i128(&pumps_text).unwrap_or(if pumps_text.starts_with('-') {
        i128::MIN
    } else {
        i128::MAX
    });
    Ok(StationRow {
        id,
        name,
        official,
        energy,
        distance: py_max(0.0, distance),
        bearing: py_mod(bearing + 180.0, 360.0) - 180.0,
        pumps,
        compatible: row.get("compatible").cloned().unwrap_or(PyValue::None),
        target: row.get("target").is_some_and(PyValue::truthy),
    })
}

/// One `FS_EVENT:` object. `Err` (`ValueError`, logged by the listener) as in Python.
pub fn parse_event(packet: &str) -> Result<PyDict, PyErr> {
    let Some(body) = packet.strip_prefix(EVENT_PREFIX) else {
        return Err(PyErr::value("not a fuel station event"));
    };
    match loads(body, "invalid event JSON")? {
        PyValue::Dict(d) if d.get("kind").is_some_and(PyValue::truthy) => Ok(d),
        _ => Err(PyErr::value("event has no kind")),
    }
}

/// 'gasoline and diesel', 'EV charging', 'gasoline, diesel and nitrous'.
pub fn energy_phrase(energy: &[String]) -> String {
    let mut words: Vec<&str> = Vec::new();
    for e in energy {
        let word = ENERGY_WORDS
            .iter()
            .find(|(k, _)| k == e)
            .map(|(_, v)| *v)
            .unwrap_or(e.as_str());
        if !word.is_empty() && !words.contains(&word) {
            words.push(word);
        }
    }
    match words.len() {
        0 => "fuel type not listed".to_string(),
        1 => words[0].to_string(),
        n => format!("{} and {}", words[..n - 1].join(", "), words[n - 1]),
    }
}

/// Eight-way direction relative to the car. Positive bearing is LEFT.
pub fn direction_phrase(bearing: f64) -> String {
    let b = py_mod(bearing + 180.0, 360.0) - 180.0;
    let side = if b > 0.0 { "left" } else { "right" };
    let a = b.abs();
    if a < 22.5 {
        return "ahead".to_string();
    }
    if a < 67.5 {
        return format!("ahead {side}");
    }
    if a < 112.5 {
        return format!("to the {side}");
    }
    if a < 157.5 {
        return format!("behind {side}");
    }
    "behind".to_string()
}

/// One browser line: name, distance and direction, what it offers, caveats.
pub fn row_line(row: &StationRow, units: &str) -> String {
    let mut parts = vec![row.name.clone()];
    if row.distance < 1.0 {
        parts.push("here".to_string());
    } else {
        parts.push(format!(
            "{} {}",
            format_long_distance(row.distance, units),
            direction_phrase(row.bearing)
        ));
    }
    parts.push(energy_phrase(&row.energy));
    if !row.official {
        parts.push("unofficial, the game can't refuel here".to_string());
    } else if row.compatible == PyValue::Bool(false) {
        parts.push("not for your vehicle".to_string());
    }
    if row.target {
        parts.push("current destination".to_string());
    }
    parts.join(", ")
}

fn pump_phrase(data: &PyDict) -> String {
    let pump = data.get("pump");
    let where_ = match data.get("side").and_then(PyValue::as_str) {
        Some(side @ ("left" | "right")) => format!(", on your {side}"),
        _ => String::new(),
    };
    if !data.get("official").is_none_or(PyValue::truthy) {
        return format!("the pumps{where_}");
    }
    match pump.filter(|p| p.truthy()) {
        Some(p) => format!("pump {}{where_}", p.py_str()),
        None => format!("a pump{where_}"),
    }
}

/// What to say for an event, or `None` to stay quiet.
pub fn event_phrase(data: &PyDict) -> Option<String> {
    let kind = data.get("kind").and_then(PyValue::as_str).unwrap_or("\u{0}none");
    let station = match data.get("station").filter(|v| v.truthy()) {
        Some(v) => v.py_str(),
        None => "the station".to_string(),
    };
    let flag = |key: &str| data.get(key).is_some_and(PyValue::truthy);
    match kind {
        "target" => Some(if flag("here") {
            format!("{station}. You are here")
        } else {
            format!("Route set to {station}. Fuel station assistance on")
        }),
        "in_range" => Some(format!(
            "Arriving at {station}. Press F9 then Shift F to park at a pump"
        )),
        "parking" => Some(format!("Parking at {}. Brake to cancel", pump_phrase(data))),
        "parked" => {
            if !data.get("official").is_none_or(PyValue::truthy) {
                return Some("Stopped by the pumps. The game has no refuelling here".to_string());
            }
            let action = if flag("electric") { "recharge" } else { "refuel" };
            if flag("inZone") {
                return Some(format!(
                    "Stopped at {}. Put the car in park, then press Shift E to {action}",
                    pump_phrase(data)
                ));
            }
            Some(format!(
                "Stopped beside {}, but the game has not registered the car at the pump. Move a little closer to it",
                pump_phrase(data)
            ))
        }
        "park_refused" | "park_failed" | "error" => {
            let message = match data.get("message").filter(|v| v.truthy()) {
                Some(v) => v.py_str(),
                None => "Parking failed".to_string(),
            };
            Some(if kind != "park_failed" {
                message
            } else {
                format!("Parking stopped. {message}")
            })
        }
        "park_cancelled" => Some("Parking cancelled".to_string()),
        "cancelled" => Some("Fuel station assistance off".to_string()),
        _ => None,
    }
}
