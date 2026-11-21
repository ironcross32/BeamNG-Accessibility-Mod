//! The Configuration panel's round trip against the real wx `ConfigPanel`
//! (`rust/fixtures/gen/config_panel.py`): a config goes through `control_values` (what
//! `load_into_controls` shows) and back through `build_saved_config` (what
//! `controls_to_config` writes), and every key must come out as the Python's did, type
//! included. The speech backend, voice and output device are machine-dependent and left
//! out by the generator.

use std::collections::BTreeSet;

use beam_app::panel::{PANEL_KEYS, build_saved_config, control_values};
use beam_base::pyval::{PyDict, PyValue, from_json};
use beam_testutil::{f, load, same_f64};
use serde_json::Value;

fn config_from(fx: &Value) -> PyDict {
    let mut d = PyDict::new();
    for (k, v) in fx.as_object().unwrap() {
        let pv = if v.get("bits").is_some() {
            PyValue::Float(f(v))
        } else {
            from_json(v)
        };
        d.insert(k.clone(), pv);
    }
    d
}

fn matches(got: Option<&PyValue>, want: &Value) -> bool {
    let (kind, w) = want.as_object().unwrap().iter().next().unwrap();
    match (kind.as_str(), got) {
        ("bool", Some(PyValue::Bool(b))) => w.as_bool() == Some(*b),
        ("int", Some(PyValue::Int(i))) => i.to_i64() == w.as_i64(),
        ("float", Some(PyValue::Float(x))) => same_f64(*x, f(w)),
        ("str", Some(PyValue::Str(s))) => w.as_str() == Some(s.as_str()),
        _ => false,
    }
}

#[test]
fn owned_keys_are_the_ones_controls_to_config_writes() {
    let fx = load("config_panel");
    let python: BTreeSet<&str> = fx["written_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let rust: BTreeSet<&str> = PANEL_KEYS.iter().map(|(k, _)| *k).collect();
    assert_eq!(rust, python);
}

#[test]
fn round_trip_matches_the_wx_panel() {
    let fx = load("config_panel");
    let mut failures = Vec::new();
    for case in fx["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let cfg = config_from(&case["config"]);
        let values = control_values(&cfg).unwrap();
        let args = values["values"].as_object().unwrap().clone();
        let saved = build_saved_config(&cfg, &args, None);
        for (key, want) in case["saved"].as_object().unwrap() {
            let got = saved.get(key);
            if !matches(got, want) {
                failures.push(format!("{name}.{key}: got {got:?}, want {want}"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
