//! Golden cases from `rust/fixtures/gen/wheel_report.py`.

mod common;

use beam_reports::pyops::{PyDict, PyValue};
use beam_reports::wheel_report::*;
use beam_testutil::{cases, f, load, s, same_f64};
use common::*;

fn opt(v: Option<f64>) -> PyValue {
    v.map(PyValue::Float).unwrap_or(PyValue::None)
}

fn dict(pairs: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Dict(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<PyDict>())
}

fn vehicle_pv(v: &Vehicle) -> PyValue {
    dict(vec![
        ("role", PyValue::Int(v.role.to_string())),
        ("id", PyValue::Int(v.id.to_string())),
        ("name", PyValue::Str(v.name.clone())),
        ("dual", PyValue::Bool(v.dual)),
        (
            "wheels",
            PyValue::List(
                v.wheels
                    .iter()
                    .map(|w| {
                        dict(vec![
                            ("lon", PyValue::Float(w.lon)),
                            ("lat", PyValue::Float(w.lat)),
                            ("width", PyValue::Float(w.width)),
                            ("psi", opt(w.psi)),
                            ("brake_c", opt(w.brake_c)),
                            ("broken", PyValue::Bool(w.broken)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn rows_pv(rows: &[WheelRow]) -> PyValue {
    PyValue::List(
        rows.iter()
            .map(|r| {
                dict(vec![
                    ("label", PyValue::Str(r.label.clone())),
                    ("psi", opt(r.psi)),
                    ("brake_c", opt(r.brake_c)),
                    ("broken", PyValue::Bool(r.broken)),
                ])
            })
            .collect(),
    )
}

#[test]
fn constants() {
    let fx = load("wheel_report");
    let c = &fx["constants"];
    assert!(same_f64(AXLE_GAP_M, f(&c["AXLE_GAP_M"])));
    assert!(same_f64(CENTRE_LAT_M, f(&c["CENTRE_LAT_M"])));
    assert!(same_f64(DUAL_WIDTH_RATIO, f(&c["DUAL_WIDTH_RATIO"])));
    assert!(same_pv(&rows_pv(&all_rows(None)), &dec(&fx["all_rows_none"])));
}

#[test]
fn packets_match_python() {
    let fx = load("wheel_report");
    let all = cases(&fx, "packets");
    let mut failures = Vec::new();
    for case in all {
        let text = s(&case["text"]);
        let parsed = parse_packet(text);
        let got = match &parsed {
            None => PyValue::None,
            Some(vs) => PyValue::List(vs.iter().map(vehicle_pv).collect()),
        };
        if !same_pv(&got, &dec(&case["parsed"])) {
            failures.push(format!("parse_packet({text:?}) = {got:?}, want {}", case["parsed"]));
            continue;
        }
        let Some(vs) = parsed else { continue };
        let rows = rows_pv(&all_rows(Some(&vs)));
        if !same_pv(&rows, &dec(&case["all_rows"])) {
            failures.push(format!("all_rows({text:?}) = {rows:?}, want {}", case["all_rows"]));
        }
        let mut i = 0;
        for v in &vs {
            for tc in [0usize, 1, 2] {
                let want = &case["vehicle_rows"][i];
                assert_eq!(want["trailer_count"].as_u64().unwrap() as usize, tc);
                let got = rows_pv(&vehicle_rows(v, tc));
                if !same_pv(&got, &dec(&want["rows"])) {
                    failures.push(format!("vehicle_rows({text:?}, {tc}) = {got:?}, want {}", want["rows"]));
                }
                i += 1;
            }
        }
    }
    check_failures(failures, all.len());
}
