//! Golden cases from `fixtures/gen/wheel_slip.py`.

mod common;

use beam_reports::pyops::PyValue;
use beam_reports::wheel_slip::*;
use beam_testutil::{cases, f, load, same_f64};
use common::*;
use serde_json::Value;

fn same_result(r: &SlipResult, w: &Value) -> bool {
    r.active == w["active"].as_bool().unwrap()
        && r.kind as i64 == w["kind"].as_i64().unwrap()
        && same_f64(r.magnitude_ms, f(&w["magnitude_ms"]))
        && r.slide_active == w["slide_active"].as_bool().unwrap()
        && same_f64(r.slide_deg, f(&w["slide_deg"]))
        && same_f64(r.azimuth_deg, f(&w["azimuth_deg"]))
        && same_f64(r.v_long_ms, f(&w["v_long_ms"]))
        && same_f64(r.filtered_ms, f(&w["filtered_ms"]))
        && r.legacy == w["legacy"].as_bool().unwrap()
        && same_f64(r.ratio, f(&w["ratio"]))
}

#[test]
fn constants_and_inactive() {
    let fx = load("wheel_slip");
    let c = &fx["constants"];
    for (name, v) in [
        ("SLIP_TAU_S", SLIP_TAU_S),
        ("SLIP_SUSTAIN_S", SLIP_SUSTAIN_S),
        ("SLIP_ABS_THRESHOLD_MS", SLIP_ABS_THRESHOLD_MS),
        ("SLIP_REL_THRESHOLD", SLIP_REL_THRESHOLD),
        ("SLIP_MIN_GROUND_MS", SLIP_MIN_GROUND_MS),
        ("LEGACY_TAU_S", LEGACY_TAU_S),
        ("LEGACY_SUSTAIN_S", LEGACY_SUSTAIN_S),
        ("SLIDE_MIN_GROUND_MS", SLIDE_MIN_GROUND_MS),
        ("SLIDE_ENTER_DEG", SLIDE_ENTER_DEG),
        ("SLIDE_EXIT_DEG", SLIDE_EXIT_DEG),
        ("SLIDE_SUSTAIN_S", SLIDE_SUSTAIN_S),
        ("AZIMUTH_SNAP_DEG", AZIMUTH_SNAP_DEG),
        ("DIR_MIN_MS", DIR_MIN_MS),
        ("DIR_FULL_MS", DIR_FULL_MS),
    ] {
        assert!(same_f64(v, f(&c[name])), "{name}");
    }
    assert!(same_result(&INACTIVE, &fx["inactive"]));
}

#[test]
fn pure_functions_match_python() {
    let fx = load("wheel_slip");
    let mut failures = Vec::new();
    let mut total = 0;
    for case in cases(&fx, "slip_ratio") {
        total += 1;
        let kind = case["kind"].as_i64().unwrap() as i32;
        let got = slip_ratio(kind, f(&case["mag"]), f(&case["v"]));
        if !same_f64(got, f(&case["out"])) {
            failures.push(format!("slip_ratio {case}: got {got:?}"));
        }
    }
    for case in cases(&fx, "travel_bearing") {
        total += 1;
        let got = travel_bearing(f(&case["vx"]), f(&case["vy"]), f(&case["h"]));
        if !same_f64(got, f(&case["out"])) {
            failures.push(format!("travel_bearing {case}: got {got:?}"));
        }
    }
    for case in cases(&fx, "sideslip_deg") {
        total += 1;
        let got = sideslip_deg(f(&case["b"]));
        if !same_f64(got, f(&case["out"])) {
            failures.push(format!("sideslip_deg {case}: got {got:?}"));
        }
    }
    for case in cases(&fx, "voice_azimuth") {
        total += 1;
        let got = voice_azimuth(
            opt_f(&case["b"]),
            f(&case["g"]),
            case["rev"].as_bool().unwrap(),
        );
        if !same_f64(got, f(&case["out"])) {
            failures.push(format!("voice_azimuth {case}: got {got:?}"));
        }
    }
    for case in cases(&fx, "threshold") {
        total += 1;
        let got = threshold_for_tests(f(&case["v"]));
        if !same_f64(got, f(&case["out"])) {
            failures.push(format!("_threshold {case}: got {got:?}"));
        }
    }
    check_failures(failures, total);
}

#[test]
fn classify_matches_python() {
    let fx = load("wheel_slip");
    let all = cases(&fx, "classify");
    let mut failures = Vec::new();
    for case in all {
        let sample = dec_dict(&case["sample"]);
        let got = classify(&sample);
        let ok = matches_outcome(&got, case, true, |c, want| {
            let want = dec_dict(want);
            let fl = |k: &str| match want.get(k) {
                Some(PyValue::Float(x)) => *x,
                other => panic!("{k}: {other:?}"),
            };
            want.get("active") == Some(&PyValue::Bool(c.active))
                && want.get("kind") == Some(&PyValue::Str(c.kind.to_string()))
                && same_f64(c.magnitude_mps, fl("magnitude_mps"))
                && same_f64(c.raw_mps, fl("raw_mps"))
                && same_f64(c.ratio, fl("ratio"))
                && want.get("slide") == Some(&PyValue::Bool(c.slide))
                && same_f64(c.slide_deg, fl("slide_deg"))
        });
        if !ok {
            failures.push(format!(
                "classify {}: got {got:?}, want {case}",
                case["sample"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn detector_sequences_match_python() {
    let fx = load("wheel_slip");
    let seqs = cases(&fx, "sequences");
    let mut failures = Vec::new();
    let mut total = 0;
    for (si, seq) in seqs.iter().enumerate() {
        let mut det = SlipDetector::new();
        for (i, st) in seq.as_array().unwrap().iter().enumerate() {
            if st.get("reset").is_some() {
                det.reset();
                continue;
            }
            total += 1;
            let got = det.step(
                f(&st["now"]),
                f(&st["ground"]),
                opt_f(&st["bearing"]),
                opt_f(&st["wmax"]),
                opt_f(&st["wmin"]),
                opt_f(&st["legacy_wheel"]),
                st["reverse"].as_bool().unwrap(),
            );
            if !same_result(&got, &st["out"]) {
                failures.push(format!("sequence {si} step {i}: got {got:?}, want {st}"));
                break;
            }
        }
    }
    check_failures(failures, total);
}
