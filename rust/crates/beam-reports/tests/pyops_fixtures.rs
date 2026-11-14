//! Golden cases from `rust/fixtures/gen/pyops.py`.

mod common;

use beam_reports::pyops::*;
use beam_testutil::{cases, f, load, s, same_f64};
use common::*;

#[test]
fn json_loads_matches_python() {
    let fx = load("pyops");
    let all = cases(&fx, "json_loads");
    let mut failures = Vec::new();
    for case in all {
        let text = s(&case["text"]);
        let got = json_loads(text);
        // Python raises JSONDecodeError, a ValueError subclass, and every caller catches
        // it as such; the message is deliberately not Python's (callers replace it).
        let mut case = case.clone();
        if case.get("err").and_then(|e| e.as_str()) == Some("JSONDecodeError") {
            case["err"] = "ValueError".into();
        }
        if !matches_outcome(&got, &case, false, |v, want| same_pv(v, &dec(want))) {
            failures.push(format!("json_loads({text:?}) = {got:?}, want {case}"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn conversions_match_python() {
    let fx = load("pyops");
    let all = cases(&fx, "conversions");
    let mut failures = Vec::new();
    for case in all {
        let v = dec(&case["value"]);
        let fl = v.py_float();
        if !matches_outcome(&fl, &case["float"], true, |x, want| {
            same_pv(&PyValue::Float(*x), &dec(want))
        }) {
            failures.push(format!("float({v:?}) = {fl:?}, want {}", case["float"]));
        }
        let it = v.py_int();
        if !matches_outcome(&it, &case["int"], true, |x, want| {
            same_pv(&PyValue::Int(x.clone()), &dec(want))
        }) {
            failures.push(format!("int({v:?}) = {it:?}, want {}", case["int"]));
        }
        if v.py_str() != s(&case["str"]) {
            failures.push(format!(
                "str({v:?}) = {:?}, want {}",
                v.py_str(),
                case["str"]
            ));
        }
        if v.py_repr() != s(&case["repr"]) {
            failures.push(format!(
                "repr({v:?}) = {:?}, want {}",
                v.py_repr(),
                case["repr"]
            ));
        }
        if v.truthy() != case["truthy"].as_bool().unwrap() {
            failures.push(format!("bool({v:?}) wrong"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn float_to_int_matches_python() {
    let fx = load("pyops");
    let all = cases(&fx, "float_to_int");
    let mut failures = Vec::new();
    for case in all {
        let x = f(&case["x"]);
        let a = py_float_to_int(x);
        let b = py_round_to_int(x);
        let eq =
            |t: &String, want: &serde_json::Value| same_pv(&PyValue::Int(t.clone()), &dec(want));
        if !matches_outcome(&a, &case["int"], true, eq) {
            failures.push(format!("int({x:?}) = {a:?}, want {}", case["int"]));
        }
        if !matches_outcome(&b, &case["round_int"], true, eq) {
            failures.push(format!(
                "int(round({x:?})) = {b:?}, want {}",
                case["round_int"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn group_thousands_matches_python() {
    let fx = load("pyops");
    for case in cases(&fx, "group_thousands") {
        assert_eq!(group_thousands(s(&case["int"])), s(&case["out"]), "{case}");
    }
}

#[test]
fn html_unescape_matches_python() {
    let fx = load("pyops");
    let all = cases(&fx, "html_unescape");
    let mut failures = Vec::new();
    for case in all {
        let got = html_unescape(s(&case["in"]));
        if got != s(&case["out"]) {
            failures.push(format!(
                "unescape({}) = {got:?}, want {}",
                case["in"], case["out"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn hypot_matches_python() {
    let fx = load("pyops");
    let all = cases(&fx, "hypot");
    let mut failures = Vec::new();
    for case in all {
        let (x, y) = (f(&case["x"]), f(&case["y"]));
        let got = py_hypot(x, y);
        if !same_f64(got, f(&case["out"])) {
            failures.push(format!(
                "hypot({x:?}, {y:?}) = {got:?}, want {}",
                case["out"]["repr"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn min_max_match_python() {
    let fx = load("pyops");
    for case in cases(&fx, "minmax") {
        let (a, b) = (f(&case["a"]), f(&case["b"]));
        assert!(same_f64(py_min(a, b), f(&case["min"])), "{case}");
        assert!(same_f64(py_max(a, b), f(&case["max"])), "{case}");
    }
}

#[test]
fn sort_matches_python() {
    let fx = load("pyops");
    for case in cases(&fx, "sort") {
        let keys: Vec<f64> = case["keys"].as_array().unwrap().iter().map(f).collect();
        let mut idx: Vec<usize> = (0..keys.len()).collect();
        py_sort_by(&mut idx, |a, b| keys[*a] < keys[*b]);
        let want: Vec<usize> = case["order"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        assert_eq!(idx, want, "{case}");
    }
}

#[test]
fn isprintable_matches_python() {
    let fx = load("pyops");
    for case in cases(&fx, "isprintable_sample") {
        let cp = case["cp"].as_u64().unwrap() as u32;
        if let Some(c) = char::from_u32(cp) {
            assert_eq!(
                is_printable(c),
                case["printable"].as_bool().unwrap(),
                "{case}"
            );
        }
    }
}
