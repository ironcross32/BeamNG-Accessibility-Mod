//! Golden cases from `fixtures/gen/poi_guidance.py`.

mod common;

use beam_reports::poi_guidance::*;
use beam_testutil::{cases, f, load, s, same_f64};
use common::*;

#[test]
fn constants() {
    let fx = load("poi_guidance");
    assert_eq!(POI_PACKET_PREFIX, s(&fx["constants"]["POI_PACKET_PREFIX"]));
    assert!(same_f64(
        POI_PARKED_RADIUS_M,
        f(&fx["constants"]["POI_PARKED_RADIUS_M"])
    ));
}

#[test]
fn spoken_text_matches_python() {
    let fx = load("poi_guidance");
    let all = cases(&fx, "spoken_text");
    let mut failures = Vec::new();
    for case in all {
        let got = spoken_text_for_tests(s(&case["in"]));
        if got != s(&case["out"]) {
            failures.push(format!(
                "_spoken_text({}) = {got:?}, want {}",
                case["in"], case["out"]
            ));
        }
    }
    check_failures(failures, all.len());
}

#[test]
fn packets_and_phrases_match_python() {
    let fx = load("poi_guidance");
    let all = cases(&fx, "packets");
    let mut failures = Vec::new();
    for case in all {
        let p = s(&case["packet"]);
        let got = parse_poi_packet(p);
        let ok = matches_outcome(&got, case, true, |poi, w| {
            poi.name == s(&w["name"])
                && poi.description == s(&w["description"])
                && same_f64(poi.distance, f(&w["distance"]))
                && same_f64(poi.bearing, f(&w["bearing"]))
                && same_f64(poi.radius, f(&w["radius"]))
                && ["imperial", "metric", "Imperial"]
                    .iter()
                    .all(|u| poi_phrase(poi, u) == s(&case["phrases"][*u]))
        });
        if !ok {
            failures.push(format!("parse_poi_packet({p:?}) = {got:?}, want {case}"));
        }
    }
    check_failures(failures, all.len());
}
