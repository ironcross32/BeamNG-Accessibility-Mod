//! Golden cases from `rust/fixtures/gen/induction_report.py`.

mod common;

use beam_base::pyfmt::{py_float_str, py_round};
use beam_reports::induction_report::*;
use beam_reports::pyops::PyValue;
use beam_testutil::{cases, load, s};
use common::*;

/// The sim's `psi`: `round(float(value), 1), "psi"`, printed with `str()`.
pub fn psi(v: f64) -> (String, String) {
    (py_float_str(py_round(v, 1)), "psi".into())
}

/// The sim's `bar`: `round(float(value) / 14.504, 2), "bar"`.
pub fn bar(v: f64) -> (String, String) {
    (py_float_str(py_round(v / 14.504, 2)), "bar".into())
}

#[test]
fn constants() {
    let fx = load("induction_report");
    assert_eq!(REPLY_PREFIX, s(&fx["constants"]["REPLY_PREFIX"]));
    let names: Vec<(String, String)> = fx["constants"]["TYPE_NAMES"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
        .collect();
    let ours: Vec<(String, String)> =
        TYPE_NAMES.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    assert_eq!(ours, names);
}

#[test]
fn parse_reply_matches_python() {
    let fx = load("induction_report");
    let all = cases(&fx, "parse_reply");
    let mut failures = Vec::new();
    for case in all {
        let text = s(&case["text"]);
        let got = match parse_reply(text) {
            Ok(Some(d)) => PyValue::Dict(d),
            Ok(None) => PyValue::None,
            Err(e) => panic!("{text:?}: {e:?}"),
        };
        if !same_pv(&got, &dec(&case["out"])) {
            failures.push(format!("parse_reply({text:?}) = {got:?}, want {}", case["out"]));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn report_phrase_matches_python() {
    let fx = load("induction_report");
    let all = cases(&fx, "report_phrase");
    let mut failures = Vec::new();
    for case in all {
        let report = dec_dict(&case["report"]);
        for (name, fmt) in [("psi", &psi as &dyn Fn(f64) -> (String, String)), ("bar", &bar)] {
            let got = report_phrase(&report, fmt);
            // Messages too: they are Python's own for every exception this can raise.
            if !matches_outcome(&got, &case[name], true, |p, want| p == s(want)) {
                failures.push(format!(
                    "report_phrase({}, {name}) = {got:?}, want {}",
                    case["report"], case[name]
                ));
            }
        }
    }
    check_failures(failures, all.len());
}
