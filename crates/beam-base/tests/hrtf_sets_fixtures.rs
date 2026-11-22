//! Golden cases from `fixtures/gen/hrtf_sets.py`.

use beam_base::hrtf_sets::*;
use beam_testutil::{cases, f, load, s, same_f64};

fn info_matches(info: &HRTFSetInfo, want: &serde_json::Value) -> bool {
    info.set_id == s(&want[0]) && info.label == s(&want[1]) && info.filename == s(&want[2])
}

#[test]
fn sets_and_constants() {
    let fx = load("hrtf_sets");
    let bundled = cases(&fx, "bundled");
    assert_eq!(BUNDLED_SETS.len(), bundled.len());
    for (info, want) in BUNDLED_SETS.iter().zip(bundled) {
        assert!(info_matches(info, want), "{info:?} vs {want}");
    }
    let available = cases(&fx, "available");
    let ours = available_sets();
    assert_eq!(ours.len(), available.len());
    for (info, want) in ours.iter().zip(available) {
        assert!(info_matches(info, want), "{info:?} vs {want}");
    }
    assert_eq!(DEFAULT_HRTF_SET, s(&fx["DEFAULT_HRTF_SET"]));
    assert!(same_f64(ELEVATION_MIN_DEG, f(&fx["ELEVATION_MIN_DEG"])));
    assert!(same_f64(ELEVATION_MAX_DEG, f(&fx["ELEVATION_MAX_DEG"])));
    assert!(same_f64(
        DEFAULT_ELEVATION_DEG,
        f(&fx["DEFAULT_ELEVATION_DEG"])
    ));
    assert_eq!(DEFAULT_EQUALISE, fx["DEFAULT_EQUALISE"].as_bool().unwrap());
}

#[test]
fn clamp_elevation_matches_python() {
    let fx = load("hrtf_sets");
    for case in cases(&fx, "clamp_elevation") {
        let want = f(&case["out"]);
        let got = if case["is_float"].as_bool().unwrap() {
            let x = f(&case["value"]);
            // A float config value goes through either entry point identically.
            let direct = clamp_elevation(x);
            if x.is_finite() {
                let via_value = clamp_elevation_value(&serde_json::json!(x));
                assert!(same_f64(direct, via_value), "{case}");
            }
            direct
        } else {
            clamp_elevation_value(&case["value"])
        };
        assert!(
            same_f64(got, want),
            "clamp_elevation({}) = {got:?}, want {want:?}",
            case["value"]
        );
    }
}

#[test]
fn resolve_matches_python() {
    let fx = load("hrtf_sets");
    for case in cases(&fx, "resolve") {
        let id = s(&case["set_id"]);
        assert_eq!(resolve(id).set_id, s(&case["out"]), "resolve({id:?})");
    }
}
