//! Golden cases from `fixtures/gen/route_beacon.py`.

use beam_base::route_beacon::*;
use beam_testutil::{cases, f, load, s, same_f64};
use serde_json::Value;

fn check_failures(failures: Vec<String>, total: usize) {
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

#[test]
fn constants() {
    let fx = load("route_beacon");
    let c = &fx["constants"];
    assert_eq!(ROUTE_PREFIX, s(&c["ROUTE_PREFIX"]));
    assert_eq!(ROUTE_CLEAR, s(&c["ROUTE_CLEAR"]));
    assert!(same_f64(AT_DESTINATION_M, f(&c["AT_DESTINATION_M"])));
}

#[test]
fn parse_route_packet_matches_python() {
    let fx = load("route_beacon");
    let all = cases(&fx, "packets");
    let mut failures = Vec::new();
    for case in all {
        let text = s(&case["text"]);
        let got = parse_route_packet(text);
        match (case.get("ok"), case.get("msg")) {
            (Some(Value::Null), _) => {
                if got != Ok(None) {
                    failures.push(format!("{text:?}: got {got:?}, want None"));
                }
            }
            (Some(ok), _) => {
                let d = ok["dest"].as_array().unwrap();
                let want = (f(&d[0]), f(&d[1]), f(&d[2]), f(&ok["route_m"]));
                match got {
                    Ok(Some(p))
                        if same_f64(p.dest.0, want.0)
                            && same_f64(p.dest.1, want.1)
                            && same_f64(p.dest.2, want.2)
                            && same_f64(p.route_m, want.3) => {}
                    other => failures.push(format!("{text:?}: got {other:?}, want {want:?}")),
                }
            }
            (None, Some(msg)) => match got {
                Err(e) if e.message() == s(msg) => {}
                other => failures.push(format!("{text:?}: got {other:?}, want ValueError({msg})")),
            },
            _ => panic!("bad case {case}"),
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn bearings_match_python() {
    let fx = load("route_beacon");
    let all = cases(&fx, "bearings");
    let mut failures = Vec::new();
    for case in all {
        let (px, py) = (f(&case["pos"][0]), f(&case["pos"][1]));
        let (dx, dy) = (f(&case["dest"][0]), f(&case["dest"][1]));
        let h = f(&case["heading"]);
        let target = target_bearing(px, py, dx, dy);
        let (dist, err) = relative_bearing(px, py, h, dx, dy);
        let ok = same_f64(target, f(&case["target"]))
            && same_f64(dist, f(&case["distance"]))
            && same_f64(err, f(&case["err"]));
        if !ok {
            failures.push(format!(
                "pos ({px}, {py}) dest ({dx}, {dy}) heading {h}: got target {target:?} dist {dist:?} err {err:?}, want {} {} {}",
                case["target"]["repr"], case["distance"]["repr"], case["err"]["repr"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn normalize_bearing_matches_python() {
    let fx = load("route_beacon");
    let all = cases(&fx, "normalize");
    let mut failures = Vec::new();
    for case in all {
        let x = f(&case["x"]);
        let got = normalize_bearing(x);
        if !same_f64(got, f(&case["out"])) {
            failures.push(format!(
                "normalize_bearing({x:?}) = {got:?}, want {}",
                case["out"]["repr"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn format_long_distance_matches_python() {
    let fx = load("route_beacon");
    let all = cases(&fx, "format_long_distance");
    let mut failures = Vec::new();
    for case in all {
        let m = f(&case["metres"]);
        let units = s(&case["units"]);
        let got = format_long_distance(m, units);
        if got != s(&case["out"]) {
            failures.push(format!(
                "format_long_distance({m:?}, {units:?}) = {got:?}, want {}",
                case["out"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn route_beacon_phrase_matches_python() {
    let fx = load("route_beacon");
    let all = cases(&fx, "phrases");
    let mut failures = Vec::new();
    for case in all {
        let (d, b, r) = (
            f(&case["distance"]),
            f(&case["bearing"]),
            f(&case["route_m"]),
        );
        let units = s(&case["units"]);
        let got = route_beacon_phrase(d, b, r, units);
        let ok = match (case.get("ok"), case.get("msg")) {
            (Some(want), _) => got.as_deref() == Ok(s(want)),
            (None, Some(msg)) => matches!(&got, Err(e) if e.message() == s(msg)),
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "route_beacon_phrase({d:?}, {b:?}, {r:?}, {units:?}) = {got:?}, want {case}"
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn direction_label_copy_matches_road_guidance() {
    let fx = load("route_beacon");
    let all = cases(&fx, "direction_label");
    let mut failures = Vec::new();
    for case in all {
        let b = f(&case["bearing"]);
        let u = case["u_turn"].as_bool().unwrap();
        let got = direction_label_for_tests(b, u);
        let ok = match (case.get("ok"), case.get("msg")) {
            (Some(want), _) => got.as_deref() == Ok(s(want)),
            (None, Some(msg)) => matches!(&got, Err(e) if e.message() == s(msg)),
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "direction_label({b:?}, {u}) = {got:?}, want {case}"
            ));
        }
    }
    check_failures(failures, all.len());
}
