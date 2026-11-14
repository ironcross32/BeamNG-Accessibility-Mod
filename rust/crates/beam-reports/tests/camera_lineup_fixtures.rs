//! Golden cases from `rust/fixtures/gen/camera_lineup.py`.

mod common;

use beam_reports::camera_lineup::*;
use beam_testutil::{cases, f, load, s, same_f64};
use common::*;
use serde_json::Value;

fn same_tone(t: &Tone, w: &Value) -> bool {
    same_f64(t.amp, f(&w[0]))
        && same_f64(t.index, f(&w[1]))
        && same_f64(t.ratio, f(&w[2]))
        && same_f64(t.az, f(&w[3]))
}

fn geom(v: &Value) -> Geometry {
    let a: Vec<f64> = v.as_array().unwrap().iter().map(f).collect();
    Geometry {
        cam_x: a[0],
        cam_y: a[1],
        cam_fx: a[2],
        cam_fy: a[3],
        nose_x: a[4],
        nose_y: a[5],
        veh_fx: a[6],
        veh_fy: a[7],
    }
}

#[test]
fn constants() {
    let fx = load("camera_lineup");
    let c = &fx["constants"];
    assert_eq!(HEAD_ON, s(&c["HEAD_ON"]));
    assert_eq!(CROSSING, s(&c["CROSSING"]));
    for (name, v) in [
        ("DEFAULT_OFFSET_M", DEFAULT_OFFSET_M),
        ("DEFAULT_DEPTH_M", DEFAULT_DEPTH_M),
        ("STEP_M", STEP_M),
        ("OFFSET_LIMIT_M", OFFSET_LIMIT_M),
        ("DEPTH_MIN_M", DEPTH_MIN_M),
        ("DEPTH_MAX_M", DEPTH_MAX_M),
        ("DEADZONE_M", DEADZONE_M),
        ("FULL_M", FULL_M),
        ("INDEX_MIN", INDEX_MIN),
        ("INDEX_MAX", INDEX_MAX),
        ("RATIO_DRIFT", RATIO_DRIFT),
        ("CARRIER_HZ", CARRIER_HZ),
        ("SIDE_AZIMUTH_DEG", SIDE_AZIMUTH_DEG),
        ("BEHIND_AZIMUTH_DEG", BEHIND_AZIMUTH_DEG),
        ("FEET_PER_M", FEET_PER_M),
    ] {
        assert!(same_f64(v, f(&c[name])), "{name}");
    }
    assert!(same_tone(&SILENT, &fx["silent"]));
    for m in cases(&fx, "mode_name") {
        assert_eq!(mode_name(s(&m[0])), s(&m[1]));
    }
}

#[test]
fn parse_packet_matches_python() {
    let fx = load("camera_lineup");
    for case in cases(&fx, "packets") {
        let text = s(&case["text"]);
        let got = parse_packet(text);
        let ok = match (&got, &case["out"]) {
            (None, Value::Null) => true,
            (Some(LineupPacket::Off), Value::String(o)) => o == "OFF",
            (Some(LineupPacket::Geometry(g)), w @ Value::Array(_)) => *g == geom(w),
            _ => false,
        };
        assert!(ok, "parse_packet({text:?}) = {got:?}, want {}", case["out"]);
    }
}

#[test]
fn solve_and_tone_match_python() {
    let fx = load("camera_lineup");
    let all = cases(&fx, "solves");
    let mut failures = Vec::new();
    for case in all {
        let g = geom(&case["geom"]);
        let mode = s(&case["mode"]);
        let (off, depth) = (f(&case["offset"]), f(&case["depth"]));
        let (st, miss, brg) = solve(&g, mode, off, depth);
        let tp = target_point(&g, mode, off, depth);
        let tp_ok = match (tp, &case["target"]) {
            (None, Value::Null) => true,
            (Some((x, y)), Value::Array(w)) => same_f64(x, f(&w[0])) && same_f64(y, f(&w[1])),
            _ => false,
        };
        let (st2, miss2) = miss_left_m(&g, mode, off, depth);
        let ok = st == s(&case["status"])
            && same_opt(miss, opt_f(&case["miss"]))
            && same_opt(brg, opt_f(&case["bearing"]))
            && tp_ok
            && st2 == st
            && same_opt(miss2, miss)
            && same_tone(&tone_for(st, miss, brg), &case["tone"])
            && same_tone(&tone_for(st, miss, None), &case["tone_nobearing"]);
        if !ok {
            failures.push(format!("solve {case}: got {st} {miss:?} {brg:?} {tp:?}"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn tone_for_matches_python() {
    let fx = load("camera_lineup");
    let all = cases(&fx, "tones");
    let mut failures = Vec::new();
    for case in all {
        let t = tone_for(
            s(&case["status"]),
            opt_f(&case["miss"]),
            opt_f(&case["bearing"]),
        );
        if !same_tone(&t, &case["out"]) {
            failures.push(format!("tone_for {case}: got {t:?}"));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn step_value_matches_python() {
    let fx = load("camera_lineup");
    for case in cases(&fx, "steps") {
        let (o, d) = step_value(
            s(&case["mode"]),
            f(&case["offset"]),
            f(&case["depth"]),
            case["delta"].as_i64().unwrap(),
        );
        assert!(
            same_f64(o, f(&case["out"][0])) && same_f64(d, f(&case["out"][1])),
            "step_value {case}: got {o:?} {d:?}"
        );
    }
}

#[test]
fn phrases_match_python() {
    let fx = load("camera_lineup");
    let all = cases(&fx, "phrases");
    let mut failures = Vec::new();
    for case in all {
        let (mode, units) = (s(&case["mode"]), s(&case["units"]));
        let (off, depth) = (f(&case["offset"]), f(&case["depth"]));
        let v = value_phrase(mode, off, depth, units);
        let t = target_phrase(mode, off, depth, units);
        if !matches_outcome(&v, &case["value"], true, |p, w| p == s(w))
            || !matches_outcome(&t, &case["target"], true, |p, w| p == s(w))
        {
            failures.push(format!("phrases {case}: got {v:?} / {t:?}"));
        }
    }
    check_failures(failures, all.len());
}
