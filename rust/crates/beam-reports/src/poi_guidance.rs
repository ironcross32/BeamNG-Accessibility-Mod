//! Parsing and speech wording for the nearest BeamNG point of interest.

use beam_base::pyfmt::{py_f, py_is_space, py_mod};

use crate::pyops::{PyErr, PyErrKind, PyValue, html_unescape, json_loads, py_max};

pub const POI_PACKET_PREFIX: &str = "POI:";
pub const POI_PARKED_RADIUS_M: f64 = 8.0;

/// A parsed POI (Python's dict of the same keys).
#[derive(Debug, Clone, PartialEq)]
pub struct Poi {
    pub name: String,
    pub description: String,
    pub distance: f64,
    pub bearing: f64,
    pub radius: f64,
}

/// `re.sub(r"<\s*br\s*/?\s*>", ". ", value, flags=re.IGNORECASE)`.
fn replace_br(chars: &[char]) -> Vec<char> {
    let n = chars.len();
    let skip_ws = |mut j: usize| {
        while j < n && py_is_space(chars[j]) {
            j += 1;
        }
        j
    };
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        if chars[i] == '<' {
            let mut j = skip_ws(i + 1);
            if j + 1 < n
                && chars[j].eq_ignore_ascii_case(&'b')
                && chars[j + 1].eq_ignore_ascii_case(&'r')
            {
                j = skip_ws(j + 2);
                if j < n && chars[j] == '/' {
                    j += 1;
                }
                j = skip_ws(j);
                if j < n && chars[j] == '>' {
                    out.extend(". ".chars());
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `re.sub(r"<[^>]*>", " ", value)`.
fn replace_tags(chars: &[char]) -> Vec<char> {
    let n = chars.len();
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        if chars[i] == '<' {
            if let Some(off) = chars[i + 1..].iter().position(|&c| c == '>') {
                out.push(' ');
                i = i + 1 + off + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `re.sub(r"\s+", " ", value)`.
fn collapse_space(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if py_is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Turn the small amount of HTML used by POI descriptions into plain speech.
fn spoken_text(value: Option<&PyValue>) -> String {
    let Some(PyValue::Str(value)) = value else {
        return String::new();
    };
    let chars: Vec<char> = value.chars().collect();
    let chars = replace_tags(&replace_br(&chars));
    let text: String = chars.into_iter().collect();
    collapse_space(&html_unescape(&text))
        .trim_matches([' ', '.'])
        .to_string()
}

#[doc(hidden)]
/// The private `_spoken_text`, for the fixtures.
pub fn spoken_text_for_tests(value: &str) -> String {
    spoken_text(Some(&PyValue::Str(value.to_string())))
}

/// Parse and validate one `POI:` JSON datagram from terrainScanner.lua.
///
/// `Err` with kind `ValueError` and the Python message for every rejection (the listener
/// logs "Malformed nearest POI packet: {msg}" and says the response was invalid). A
/// navigation value given as an int too large for a float raises `OverflowError`, and
/// nesting too deep for the decoder `RecursionError`; neither was caught there.
pub fn parse_poi_packet(packet: &str) -> Result<Poi, PyErr> {
    let Some(body) = packet.strip_prefix(POI_PACKET_PREFIX) else {
        return Err(PyErr::value("not a POI packet"));
    };
    let row = json_loads(body).map_err(|e| {
        if e.kind == PyErrKind::ValueError {
            PyErr::value("invalid POI JSON")
        } else {
            e
        }
    })?;
    let PyValue::Dict(row) = row else {
        return Err(PyErr::value("POI payload is not an object"));
    };

    let mut name = spoken_text(row.get("name"));
    if name.is_empty() {
        name = "Point of interest".to_string();
    }
    let description = spoken_text(row.get("description"));
    let invalid = || PyErr::value("POI payload has invalid navigation values");
    let nav = |v: Option<&PyValue>| -> Result<f64, PyErr> {
        let v = v.ok_or_else(invalid)?;
        v.py_float()
            .map_err(|e| if e.is_type_or_value() { invalid() } else { e })
    };
    let distance = nav(row.get("distance"))?;
    let bearing = nav(row.get("bearing"))?;
    let zero = PyValue::Float(0.0);
    let radius = nav(Some(row.get("radius").unwrap_or(&zero)))?;
    if !(distance.is_finite() && bearing.is_finite() && radius.is_finite()) {
        return Err(PyErr::value("POI payload has non-finite navigation values"));
    }
    if distance < 0.0 {
        return Err(PyErr::value("POI distance is negative"));
    }
    Ok(Poi {
        name,
        description,
        distance,
        bearing: py_mod(bearing + 180.0, 360.0) - 180.0,
        radius: py_max(0.0, radius),
    })
}

/// Describe a POI, ending immediately after the parked status when applicable.
/// Python's default `units_mode` is `"imperial"`.
pub fn poi_phrase(poi: &Poi, units_mode: &str) -> String {
    let mut parts = vec![poi.name.clone()];
    if !poi.description.is_empty() {
        parts.push(poi.description.clone());
    }

    let centre_distance = py_max(0.0, poi.distance);
    let radius = py_max(POI_PARKED_RADIUS_M, poi.radius);
    if centre_distance <= radius {
        parts.push("Parked on it".to_string());
        return parts.join(". ") + ".";
    }

    let distance = centre_distance - radius;
    let distance_text = if units_mode == "imperial" {
        format!("{} feet", py_f(distance * 3.28084, 0))
    } else {
        format!("{} meters", py_f(distance, 0))
    };

    let bearing = poi.bearing;
    let bearing_text = if bearing.abs() < 0.5 {
        "bearing 0 degrees, straight ahead".to_string()
    } else {
        let direction = if bearing > 0.0 { "left" } else { "right" };
        format!("bearing {} degrees {direction}", py_f(bearing.abs(), 0))
    };
    parts.push(format!("{distance_text}, {bearing_text}"));
    parts.join(". ") + "."
}
