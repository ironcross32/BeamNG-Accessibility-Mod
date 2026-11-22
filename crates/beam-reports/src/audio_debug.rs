//! Row parsing, speech and stepping for the audio debug browser (F9 then J).
//!
//! The game half is `audioDebugAccessible.lua` (GE) and `beamtelAudioDebug.lua` (vehicle
//! VM); see docs/lua-audio-debug.md. The browser glue that owns sockets, speech and the
//! virtual browser lives in the engine.

use beam_base::pyfmt::{py_float_parse, py_g, py_round, py_strip};
use indexmap::IndexMap;

use crate::pyops::{
    PyErr, int_text_to_i128, py_max, py_min, py_round_to_int, py_str_to_float, py_str_to_int,
};

/// A row: the `k=v` fields in wire order.
pub type Row = IndexMap<String, String>;

/// Parse a `k=v;k=v` row body.
///
/// Split on the FIRST '=' per field and keep unknown keys: the Lua side sanitizes ';'
/// and '=' out of every value, and a field this build does not know about must be
/// ignored rather than shifting anything after it. A bare field (the leading count on a
/// BEGIN line) is kept as `count`.
pub fn parse_fields(payload: &str) -> Row {
    let mut out = Row::new();
    for field in payload.split(';') {
        if field.is_empty() {
            continue;
        }
        if let Some((key, value)) = field.split_once('=') {
            out.insert(py_strip(key).to_string(), py_strip(value).to_string());
        } else if !out.contains_key("count") {
            out.insert("count".to_string(), py_strip(field).to_string());
        }
    }
    out
}

fn get<'a>(row: &'a Row, key: &str, default: &'a str) -> &'a str {
    row.get(key).map(String::as_str).unwrap_or(default)
}

/// `float(row.get(key, ""))`, `None` where Python raises.
fn float(row: &Row, key: &str) -> Option<f64> {
    py_str_to_float(get(row, key, "")).ok()
}

/// A number as speech reads it best: no trailing zeros, no "-0", minus spelled out.
fn num(v: f64) -> String {
    let v = py_round(v, 2);
    if v == 0.0 {
        return "0".to_string();
    }
    let text = py_g(v.abs(), 6);
    if v < 0.0 {
        format!("minus {text}")
    } else {
        text
    }
}

/// `f"{float(f'{x:.3g}'):g}"`.
fn three_sig(x: f64) -> String {
    let short = py_g(x, 3);
    py_g(py_float_parse(&short).unwrap_or(f64::NAN), 6)
}

/// The value part of a number row, in the unit the user hears.
///
/// `Err` only for a `count` row whose value is NaN or infinite, where Python's
/// `round()` raises.
pub fn spoken_value(row: &Row) -> Result<String, PyErr> {
    let unit = get(row, "unit", "");
    if unit == "Hz" {
        // The wire value is FMOD's 0-1000 scale; the Lua side sends the Hz it maps to.
        let Some(hz) = float(row, "hz") else {
            return Ok(String::new());
        };
        if hz >= 1000.0 {
            return Ok(format!("{} kilohertz", three_sig(hz / 1000.0)));
        }
        return Ok(format!("{} hertz", three_sig(hz)));
    }
    let Some(value) = float(row, "value") else {
        return Ok(String::new());
    };
    if unit == "dB" {
        return Ok(format!("{} decibels", num(value)));
    }
    if unit == "count" {
        return py_round_to_int(value);
    }
    Ok(num(value))
}

/// `int(row.get(key, default))`. Python's int is unbounded; outside `i128` this is
/// treated like an unparseable value.
fn int_of(row: &Row, key: &str, default: &str) -> Option<i128> {
    py_str_to_int(get(row, key, default))
        .ok()
        .and_then(|t| int_text_to_i128(&t))
}

/// One spoken line for an engine-sound row. `Err` as [`spoken_value`].
pub fn row_line(row: &Row) -> Result<String, PyErr> {
    let kind = get(row, "kind", "");
    let label = get(row, "label", "");
    if kind == "select" {
        let (index, count) = match (int_of(row, "index", "0"), int_of(row, "count", "1")) {
            (Some(i), Some(c)) => (i + 1, c),
            _ => (1, 1),
        };
        let parts = [
            get(row, "ref", ""),
            get(row, "device", ""),
            get(row, "blend", ""),
        ];
        let name: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
        let mut line = format!("Sound {index} of {count}: {}", name.join(", "));
        if row.get("modified").map(String::as_str) == Some("1") {
            line.push_str(", changed");
        }
        return Ok(line);
    }
    if kind == "number" {
        let value = spoken_value(row)?;
        return Ok(if value.is_empty() {
            label.to_string()
        } else {
            format!("{label}, {value}")
        });
    }
    if kind == "action"
        && row.get("key").map(String::as_str) == Some("restore")
        && row.get("editable").map(String::as_str) == Some("0")
    {
        return Ok(format!("{label}, nothing changed"));
    }
    Ok(label.to_string())
}

/// The value one press of left/right (`delta` steps) moves a number row to.
///
/// `None` when the row cannot move that way: not a number row, or already at the end of
/// its range. Not snapped to the step grid -- a value the vehicle shipped with (382.35
/// on the FMOD scale, say) stays reachable by stepping back.
pub fn stepped_value(row: &Row, delta: i64) -> Option<f64> {
    if row.get("kind").map(String::as_str) != Some("number") {
        return None;
    }
    let value = float(row, "value");
    let step = float(row, "step");
    // `not step`: None and both zeros are falsy.
    let (Some(value), Some(step)) = (value, step.filter(|s| *s != 0.0)) else {
        return None;
    };
    let mut target = value + delta as f64 * step;
    if let Some(lo) = float(row, "min") {
        target = py_max(lo, target);
    }
    if let Some(hi) = float(row, "max") {
        target = py_min(hi, target);
    }
    let target = py_round(target, 4);
    if target == py_round(value, 4) {
        return None;
    }
    Some(target)
}

/// A number for a SET command: short, and never in exponent form for these ranges.
pub fn wire_number(v: f64) -> String {
    py_g(v, 6)
}

/// The sound index left/right moves to on the selector row, wrapping.
pub fn select_target(row: &Row, delta: i64) -> Option<i128> {
    let index = int_of(row, "index", "0")?;
    let count = int_of(row, "count", "0")?;
    if count <= 1 {
        return None;
    }
    Some((index + if delta > 0 { 1 } else { -1 }).rem_euclid(count))
}

/// One spoken line for a sound-library row.
pub fn sfx_line(row: &Row) -> String {
    let name = get(row, "name", "");
    if row.get("kind").map(String::as_str) == Some("dir") {
        let count = get(row, "count", "");
        let noun = if count == "1" { "item" } else { "items" };
        return if count.is_empty() {
            format!("{name}, folder")
        } else {
            format!("{name}, folder, {count} {noun}")
        };
    }
    name.to_string()
}
