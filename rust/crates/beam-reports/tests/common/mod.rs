//! Shared helpers for beam-reports' fixture tests: decoding the tagged Python values
//! written by `rust/fixtures/gen/_pyvalue.py`, and collecting failures.
#![allow(dead_code)]

use beam_reports::pyops::{PyDict, PyErr, PyValue};
use beam_testutil::{f, s, same_f64};
use serde_json::Value;

/// Decode `{"t": tag, "v": ...}` into a `PyValue`.
pub fn dec(v: &Value) -> PyValue {
    match s(&v["t"]) {
        "none" => PyValue::None,
        "bool" => PyValue::Bool(v["v"].as_bool().expect("bool")),
        "int" => PyValue::Int(s(&v["v"]).to_string()),
        "float" => PyValue::Float(f(&v["v"])),
        "str" => PyValue::Str(s(&v["v"]).to_string()),
        "list" => PyValue::List(v["v"].as_array().expect("list").iter().map(dec).collect()),
        "dict" => PyValue::Dict(dec_pairs(&v["v"])),
        other => panic!("unknown tag {other}"),
    }
}

pub fn dec_pairs(v: &Value) -> PyDict {
    v.as_array()
        .expect("pairs")
        .iter()
        .map(|p| (s(&p[0]).to_string(), dec(&p[1])))
        .collect()
}

/// A tagged dict as a `PyDict` (panics if it is not a dict).
pub fn dec_dict(v: &Value) -> PyDict {
    match dec(v) {
        PyValue::Dict(d) => d,
        other => panic!("not a dict: {other:?}"),
    }
}

/// Equality with float bits (NaN equals NaN, -0.0 differs from 0.0).
pub fn same_pv(a: &PyValue, b: &PyValue) -> bool {
    match (a, b) {
        (PyValue::Float(x), PyValue::Float(y)) => same_f64(*x, *y),
        (PyValue::List(x), PyValue::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_pv(p, q))
        }
        (PyValue::Dict(x), PyValue::Dict(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && same_pv(v1, v2))
        }
        _ => a == b,
    }
}

/// Does a Rust `Result<T, PyErr>` match a fixture outcome `{"ok": ...}` /
/// `{"err": type, "msg": ...}`? `ok_eq` compares the success value.
pub fn matches_outcome<T>(
    got: &Result<T, PyErr>,
    case: &Value,
    check_msg: bool,
    ok_eq: impl Fn(&T, &Value) -> bool,
) -> bool {
    match (got, case.get("ok"), case.get("err")) {
        (Ok(v), Some(want), _) => ok_eq(v, want),
        (Err(e), None, Some(kind)) => {
            e.kind.name() == s(kind) && (!check_msg || e.msg == s(&case["msg"]))
        }
        _ => false,
    }
}

/// An optional float from a fixture (`null` or `fbits`).
pub fn opt_f(v: &Value) -> Option<f64> {
    if v.is_null() { None } else { Some(f(v)) }
}

pub fn same_opt(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => same_f64(x, y),
        _ => false,
    }
}

pub fn check_failures(failures: Vec<String>, total: usize) {
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        total,
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
