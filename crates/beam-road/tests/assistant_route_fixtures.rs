//! Golden cases from `fixtures/gen/assistant_route.py`.

use std::sync::{Arc, Mutex};

use beam_road::assistant_route::*;
use beam_road::pyjson::{PyErr, loads};
use beam_testutil::{cases, f, load, s};

fn check(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn same<T: PartialEq<String> + std::fmt::Debug>(
    got: &Result<T, PyErr>,
    case: &serde_json::Value,
) -> bool {
    match (got, case.get("ok"), case.get("err")) {
        (Ok(g), Some(w), _) => {
            if w.is_null() {
                format!("{g:?}") == "None"
            } else {
                *g == s(w).to_string()
            }
        }
        (Err(e), None, Some(t)) => e.type_name() == s(t) && e.message() == s(&case["msg"]),
        _ => false,
    }
}

#[test]
fn wording_matches_python() {
    let fx = load("assistant_route");
    let mut failures = Vec::new();
    for case in cases(&fx, "spoken_distance") {
        let m = f(&case["metres"]);
        let u = s(&case["units"]);
        if spoken_distance(m, u) != s(&case["out"]) {
            failures.push(format!(
                "spoken_distance({m:?}, {u:?}) = {:?}, want {}",
                spoken_distance(m, u),
                case["out"]
            ));
        }
    }
    for case in cases(&fx, "turn_instruction") {
        if turn_instruction(s(&case["name"])) != s(&case["out"]) {
            failures.push(format!("turn_instruction({})", case["name"]));
        }
    }
    for case in cases(&fx, "relative_direction") {
        let b = f(&case["bearing"]);
        if relative_direction(b) != s(&case["out"]) {
            failures.push(format!(
                "relative_direction({b:?}) = {:?}",
                relative_direction(b)
            ));
        }
    }
    for case in cases(&fx, "arrival_phrase") {
        let a = loads(s(&case["arrival"])).unwrap();
        let got = arrival_phrase(&a, s(&case["units"]));
        if !same(&got, case) {
            failures.push(format!(
                "arrival_phrase({}) = {got:?}, want {case}",
                case["arrival"]
            ));
        }
    }
    for case in cases(&fx, "suppressed_toast") {
        let since = case["since"].as_object().map(|_| f(&case["since"]));
        let got = suppressed_toast(
            s(&case["text"]),
            case["following"].as_bool().unwrap(),
            since,
        );
        if got != case["out"].as_bool().unwrap() {
            failures.push(format!("suppressed_toast {case}"));
        }
    }
    check(failures);
}

#[test]
fn callout_scripts_match_python() {
    let fx = load("assistant_route");
    let mut failures = Vec::new();
    for (n, script) in cases(&fx, "callout_scripts").iter().enumerate() {
        let units = Arc::new(Mutex::new("imperial".to_string()));
        let u = units.clone();
        let mut callouts = RouteCallouts::new(move || u.lock().unwrap().clone());
        for (i, step) in script.as_array().unwrap().iter().enumerate() {
            *units.lock().unwrap() = s(&step["units"]).to_string();
            let kind = s(&step["kind"]);
            if kind == "reset" {
                callouts.reset();
                continue;
            }
            let report = loads(s(&step["report"])).unwrap();
            let ok = if kind == "route_set" {
                let got = callouts.route_set(&report);
                let ok = same(&got, step);
                if !ok {
                    failures.push(format!("script {n} step {i}: {got:?}, want {step}"));
                }
                ok
            } else {
                let got = callouts.update(&report);
                let ok = match (&got, step.get("ok")) {
                    (Ok(None), Some(w)) => w.is_null(),
                    (Ok(Some(g)), Some(w)) => !w.is_null() && g == s(w),
                    (Err(e), None) => {
                        e.type_name() == s(&step["err"]) && e.message() == s(&step["msg"])
                    }
                    _ => false,
                };
                if !ok {
                    failures.push(format!("script {n} step {i}: {got:?}, want {step}"));
                }
                ok
            };
            if ok && callouts.following != step["following"].as_bool().unwrap() {
                failures.push(format!("script {n} step {i}: following differs"));
            }
        }
    }
    check(failures);
}
