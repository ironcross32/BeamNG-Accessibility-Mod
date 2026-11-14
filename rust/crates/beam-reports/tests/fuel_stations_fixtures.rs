//! Golden cases from `rust/fixtures/gen/fuel_stations.py`.

mod common;

use beam_reports::fuel_stations::*;
use beam_reports::pyops::PyValue;
use beam_testutil::{cases, f, load, s, same_f64};
use common::*;
use serde_json::Value;

fn same_row(r: &StationRow, w: &Value) -> bool {
    let energy: Vec<String> = w["energy"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| s(e).to_string())
        .collect();
    r.id == s(&w["id"])
        && r.name == s(&w["name"])
        && r.official == w["official"].as_bool().unwrap()
        && r.energy == energy
        && same_f64(r.distance, f(&w["distance"]))
        && same_f64(r.bearing, f(&w["bearing"]))
        && r.pumps.to_string() == s(&w["pumps"])
        && same_pv(&r.compatible, &dec(&w["compatible"]))
        && r.target == w["target"].as_bool().unwrap()
}

#[test]
fn constants() {
    let fx = load("fuel_stations");
    let c = &fx["constants"];
    assert_eq!(ROW_PREFIX, s(&c["ROW_PREFIX"]));
    assert_eq!(EVENT_PREFIX, s(&c["EVENT_PREFIX"]));
    let want: Vec<(String, String)> = c["ENERGY_WORDS"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
        .collect();
    let ours: Vec<(String, String)> = ENERGY_WORDS
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    assert_eq!(ours, want);
}

#[test]
fn parse_row_and_row_line_match_python() {
    let fx = load("fuel_stations");
    let all = cases(&fx, "parse_row");
    let mut failures = Vec::new();
    for case in all {
        let p = s(&case["packet"]);
        let got = parse_row(p);
        if !matches_outcome(&got, case, true, same_row) {
            failures.push(format!("parse_row({p:?}) = {got:?}, want {case}"));
        }
    }
    for case in cases(&fx, "row_line") {
        let row = parse_row(s(&case["packet"])).unwrap();
        let got = row_line(&row, s(&case["units"]));
        if got != s(&case["out"]) {
            failures.push(format!("row_line {case}: got {got:?}"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn events_match_python() {
    let fx = load("fuel_stations");
    let all = cases(&fx, "events");
    let mut failures = Vec::new();
    for case in all {
        let p = s(&case["packet"]);
        let got = parse_event(p);
        let ok = match (&got, case.get("ok")) {
            (Ok(d), Some(w)) => {
                let phrase = event_phrase(d).unwrap_or(PyValue::None);
                same_pv(&PyValue::Dict(d.clone()), &dec(w))
                    && same_pv(&phrase, &dec(&case["phrase"]))
            }
            (Err(e), None) => e.kind.name() == s(&case["err"]) && e.msg == s(&case["msg"]),
            _ => false,
        };
        if !ok {
            failures.push(format!("parse_event({p:?}) = {got:?}, want {case}"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn energy_and_direction_match_python() {
    let fx = load("fuel_stations");
    for case in cases(&fx, "energy_phrase") {
        let input: Vec<String> = case["in"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| s(e).to_string())
            .collect();
        assert_eq!(energy_phrase(&input), s(&case["out"]), "{case}");
    }
    for case in cases(&fx, "direction_phrase") {
        assert_eq!(direction_phrase(f(&case["b"])), s(&case["out"]), "{case}");
    }
}
