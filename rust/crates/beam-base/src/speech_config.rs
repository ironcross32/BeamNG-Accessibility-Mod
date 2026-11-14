//! Speech config migration, from speech.py's migrate_config.
//!
//! The rest of speech.py (Prism, backends, voices) is in `beam-speech`; this half lives
//! here because both config loaders call it before anything else touches the dict.

use crate::pyval::{PyDict, PyInt, PyValue};

/// "auto" lets Prism pick by backend priority (NVDA outranks JAWS outranks SAPI...).
/// Anything else is matched against a Prism backend name.
pub const AUTO_BACKEND: &str = "auto";

fn setdefault(cfg: &mut PyDict, key: &str, value: PyValue) {
    if !cfg.contains_key(key) {
        cfg.insert(key.to_string(), value);
    }
}

/// Fold pre-Prism SAPI keys into the speech_* keys, in place.
///
/// The old keys were written by every configurator release up to the SRAL removal
/// but never read by beamtel, so this mostly exists to stop stale files carrying dead
/// settings forward. Returns true if anything changed.
pub fn migrate_config(cfg: &mut PyDict) -> bool {
    let mut changed = false;
    if let Some(v) = cfg.shift_remove("force_sapi") {
        if v.truthy() {
            setdefault(cfg, "speech_backend", PyValue::from("SAPI"));
        }
        changed = true;
    }
    if let Some(name) = cfg.shift_remove("sapi_voice_name") {
        if name.truthy() {
            setdefault(cfg, "speech_voice_name", name);
        }
        changed = true;
    }
    if let Some(v) = cfg.shift_remove("sapi_rate") {
        // SAPI's -10..10 becomes a 0..100 percentage. A value int() rejects is dropped.
        if let Ok(n) = v.py_int() {
            let rate = n.clamp_py(-10, 10);
            setdefault(cfg, "speech_rate", PyValue::Int(PyInt::Small((rate + 10) * 5)));
        }
        changed = true;
    }
    if let Some(v) = cfg.shift_remove("sapi_volume") {
        if let Ok(n) = v.py_int() {
            setdefault(
                cfg,
                "speech_volume",
                PyValue::Int(PyInt::Small(n.clamp_py(0, 100))),
            );
        }
        changed = true;
    }
    changed
}
