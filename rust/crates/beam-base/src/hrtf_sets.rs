//! The selectable HRTF datasets, from hrtf_sets.py.
//!
//! Every set is a pre-baked `.npz` of the measurement rings near ear level, shipped
//! next to the executable (see `paths::find_data_file`). Kept free of the audio stack
//! because the configurator fills its dropdown and validates the saved choice from it.

use serde_json::Value;

use crate::pyfmt::py_float_parse;

/// One selectable set: Python's `HRTFSetInfo(set_id, label, filename)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HRTFSetInfo {
    pub set_id: &'static str,
    pub label: &'static str,
    pub filename: &'static str,
}

/// Order is the dropdown order. Licences and attribution: HRTF_DATA_LICENSES.md.
pub const BUNDLED_SETS: [HRTFSetInfo; 3] = [
    HRTFSetInfo {
        set_id: "mit_kemar",
        label: "MIT KEMAR",
        filename: "hrtf_mit_kemar.npz",
    },
    HRTFSetInfo {
        set_id: "thk_ku100",
        label: "Neumann KU100 (TH Köln)",
        filename: "hrtf_thk_ku100.npz",
    },
    HRTFSetInfo {
        set_id: "sadie_kemar",
        label: "KEMAR (SADIE II, University of York)",
        filename: "hrtf_sadie_kemar.npz",
    },
];

/// Unchanged from before the choice existed.
pub const DEFAULT_HRTF_SET: &str = "mit_kemar";

/// The elevation every spatialised cue is rendered at, degrees, negative = below ear
/// level. The baked sets carry the measured rings from ELEVATION_MIN_DEG to
/// ELEVATION_MAX_DEG (plus one either side where the grid needs it to bracket). -3 is
/// roughly the road 20 m ahead from a driver's ear, well inside the ~10 degree vertical
/// blur of hearing near straight ahead: expect a slight timbre change, not "lower".
pub const ELEVATION_MIN_DEG: f64 = -20.0;
pub const ELEVATION_MAX_DEG: f64 = 10.0;
pub const DEFAULT_ELEVATION_DEG: f64 = -3.0;

/// Equalise out the tone colour the measurements share in every direction.
pub const DEFAULT_EQUALISE: bool = true;

/// `clamp_elevation` for a value that is already a float. NaN gives the default.
pub fn clamp_elevation(value: f64) -> f64 {
    if value.is_nan() {
        return DEFAULT_ELEVATION_DEG;
    }
    // max(ELEVATION_MIN_DEG, min(ELEVATION_MAX_DEG, value)), first-wins on ties.
    let inner = if value < ELEVATION_MAX_DEG {
        value
    } else {
        ELEVATION_MAX_DEG
    };
    if inner > ELEVATION_MIN_DEG {
        inner
    } else {
        ELEVATION_MIN_DEG
    }
}

/// `clamp_elevation` for a raw config value, with Python's `float(value)` coercion:
/// numbers as they are, `true`/`false` as 1/0, numeric strings parsed, anything else
/// (null, lists, objects, unparsable text) the default.
pub fn clamp_elevation_value(value: &Value) -> f64 {
    let number = match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => py_float_parse(s),
        _ => None,
    };
    match number {
        Some(v) => clamp_elevation(v),
        None => DEFAULT_ELEVATION_DEG,
    }
}

/// Every selectable set, in display order.
pub fn available_sets() -> Vec<HRTFSetInfo> {
    BUNDLED_SETS.to_vec()
}

/// The set for `set_id`, or the default when the id is unknown (a stale or
/// hand-edited config). A config value that is not a string matches nothing: pass `""`.
pub fn resolve(set_id: &str) -> HRTFSetInfo {
    let sets = available_sets();
    if let Some(info) = sets.iter().find(|info| info.set_id == set_id) {
        return *info;
    }
    if let Some(info) = sets.iter().find(|info| info.set_id == DEFAULT_HRTF_SET) {
        return *info;
    }
    sets[0]
}
