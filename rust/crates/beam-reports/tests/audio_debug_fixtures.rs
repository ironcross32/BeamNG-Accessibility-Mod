//! Golden cases from `rust/fixtures/gen/audio_debug.py`.

mod common;

use beam_reports::audio_debug::*;
use beam_testutil::{cases, f, load, s, same_f64};
use common::*;
use serde_json::Value;

fn row(v: &Value) -> Row {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
        .collect()
}

#[test]
fn parse_fields_matches_python() {
    let fx = load("audio_debug");
    for case in cases(&fx, "parse_fields") {
        let got = parse_fields(s(&case["payload"]));
        let want = row(&case["out"]);
        assert_eq!(
            got.iter().collect::<Vec<_>>(),
            want.iter().collect::<Vec<_>>(),
            "{case}"
        );
    }
}

#[test]
fn row_line_and_spoken_value_match_python() {
    let fx = load("audio_debug");
    let all = cases(&fx, "row_line");
    let mut failures = Vec::new();
    for case in all {
        let r = row(&case["row"]);
        let line = row_line(&r);
        let spoken = spoken_value(&r);
        if !matches_outcome(&line, case, true, |p, w| p == s(w))
            || !matches_outcome(&spoken, &case["spoken"], true, |p, w| p == s(w))
        {
            failures.push(format!("{case}: got {line:?} / {spoken:?}"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn stepped_value_matches_python() {
    let fx = load("audio_debug");
    let all = cases(&fx, "stepped_value");
    let mut failures = Vec::new();
    for case in all {
        let got = stepped_value(&row(&case["row"]), case["delta"].as_i64().unwrap());
        let ok = match (got, &case["out"]) {
            (None, Value::Null) => true,
            (Some(x), w) if !w.is_null() => same_f64(x, f(w)),
            _ => false,
        };
        if !ok {
            failures.push(format!("{case}: got {got:?}"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn wire_select_sfx_match_python() {
    let fx = load("audio_debug");
    for case in cases(&fx, "wire_number") {
        assert_eq!(wire_number(f(&case["v"])), s(&case["out"]), "{case}");
    }
    for case in cases(&fx, "select_target") {
        let got = select_target(&row(&case["row"]), case["delta"].as_i64().unwrap());
        let want = case["out"].as_i64().map(i128::from);
        assert_eq!(got, want, "{case}");
    }
    for case in cases(&fx, "sfx_line") {
        assert_eq!(sfx_line(&row(&case["row"])), s(&case["out"]), "{case}");
    }
}
