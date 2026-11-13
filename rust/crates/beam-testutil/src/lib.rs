//! Test helpers for the golden fixtures written by `rust/fixtures/gen/*.py`.
//!
//! Use it as a dev-dependency only. Floats in fixtures are written by
//! `_common.fbits()` as `{"bits": <u64>, "repr": "<python repr>"}` so they survive JSON
//! exactly, including NaN, infinities and negative zero; [`f`] reads either that form
//! or a plain JSON number.

use std::path::PathBuf;

use serde_json::Value;

/// `rust/fixtures/data`, found from this crate's own manifest directory, so it works
/// from any crate's tests regardless of the current directory.
pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("data")
}

/// The repository root (the directory holding `rust/`), for tests that read the Python
/// or Lua sources the way a Python sim does.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// Load `rust/fixtures/data/<name>.json`. Panics with the path on failure, since a
/// missing fixture is a broken test, not a runtime condition.
pub fn load(name: &str) -> Value {
    let path = fixture_dir().join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse fixture {}: {e}", path.display()))
}

/// A float from a fixture: `{"bits": u64, ...}` (exact) or a JSON number.
pub fn f(v: &Value) -> f64 {
    if let Some(bits) = v.get("bits") {
        return f64::from_bits(bits.as_u64().expect("fixture float bits must be a u64"));
    }
    v.as_f64()
        .unwrap_or_else(|| panic!("fixture value is not a float: {v}"))
}

/// A string from a fixture.
pub fn s(v: &Value) -> &str {
    v.as_str()
        .unwrap_or_else(|| panic!("fixture value is not a string: {v}"))
}

/// The `cases` array of a fixture object.
pub fn cases<'a>(fixture: &'a Value, key: &str) -> &'a Vec<Value> {
    fixture
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("fixture has no array {key:?}"))
}

/// Bitwise float equality, with every NaN equal to every NaN. Distinguishes 0.0 from
/// -0.0, which Python's arithmetic does too.
pub fn same_f64(a: f64, b: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits()
}
