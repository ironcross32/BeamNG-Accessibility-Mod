//! diagnostic/route_beacon_sim.py: route-beacon protocol, bearing sign and phrasing.
//!
//! Every scenario also asserts what the NAIVE form answers, so no check can pass for
//! free. That matters more here than usual: this feature's failure mode is a beacon
//! that pans confidently to the wrong side, which sounds exactly like one that works.
//!
//! Scenario numbers and order follow the Python sim. Parts that exercise other crates
//! are not here yet:
//! - 3 (the engine half): beamtel's callers of `relative_bearing` live in beam-engine;
//!   `scenario_3_engine_uses_shared_helper` is ignored until they are ported.
//! - 4 (rate map) and 5 (audio gating and teardown) test `audio.py` and belong in
//!   beam-audio's tests.
//! - 6, the one check against `road_guidance.format_road_distance` (the naive
//!   formatter), belongs in beam-road's tests.

use beam_base::route_beacon as rb;
use beam_testutil::repo_root;

fn expect_bad(payload: &str) {
    assert!(
        rb::parse_route_packet(payload).is_err(),
        "accepted malformed packet: {payload:?}"
    );
}

// =============================================================================
// 1. Protocol
// =============================================================================
#[test]
fn scenario_1_protocol() {
    let packet = rb::parse_route_packet("ROUTE:1200.5,-340.25,61.0,4821.5").unwrap();
    assert_eq!(
        packet,
        Some(rb::RoutePacket {
            dest: (1200.5, -340.25, 61.0),
            route_m: 4821.5
        }),
        "a well-formed packet round-trips"
    );

    // CLEAR is a MESSAGE, not an absence. None is the "no route" answer and is
    // distinguishable from a parse failure, which errors.
    assert_eq!(
        rb::parse_route_packet("ROUTE:CLEAR"),
        Ok(None),
        "CLEAR parses to None"
    );

    expect_bad("ROUTE:1,2,3"); // short tail
    expect_bad("ROUTE:a,b,c,d"); // non-numeric
    expect_bad("ROUTE:1,2,nan,4"); // non-finite
    expect_bad("TRAILER:0,1,x"); // another extension's packet
    expect_bad("");

    // A negative remaining distance is clamped rather than trusted.
    assert_eq!(
        rb::parse_route_packet("ROUTE:0,0,0,-5")
            .unwrap()
            .unwrap()
            .route_m,
        0.0,
        "a negative route distance clamps to zero"
    );
}

// =============================================================================
// 2. Bearing sign -- the expensive one to get wrong
// =============================================================================
#[test]
fn scenario_2_bearing() {
    // BeamTel's heading is 180 deg off a true compass heading, and relative_bearing's
    // atan2(-dx, -dy) carries the matching +180 so the two cancel. Facing TRUE NORTH
    // therefore means heading == 180 here.
    const NORTH: f64 = 180.0;

    let (dist, brg) = rb::relative_bearing(0.0, 0.0, NORTH, 0.0, 100.0);
    assert!(
        brg.abs() < 1e-6 && dist == 100.0,
        "dest ahead reads ~0: {brg}"
    );

    let (_, brg) = rb::relative_bearing(0.0, 0.0, NORTH, -100.0, 0.0);
    assert!(
        (brg - 90.0).abs() < 1e-6,
        "dest to the WEST while facing north reads +90 (left): {brg}"
    );

    let (_, brg) = rb::relative_bearing(0.0, 0.0, NORTH, 100.0, 0.0);
    assert!(
        (brg + 90.0).abs() < 1e-6,
        "dest to the EAST while facing north reads -90 (right): {brg}"
    );

    let (_, brg) = rb::relative_bearing(0.0, 0.0, NORTH, 0.0, -100.0);
    assert!(
        (brg.abs() - 180.0).abs() < 1e-6,
        "dest behind reads +/-180: {brg}"
    );

    // Facing east (heading 270 in this convention), north is on the driver's left.
    let (_, brg) = rb::relative_bearing(0.0, 0.0, 270.0, 0.0, 100.0);
    assert!(
        (brg - 90.0).abs() < 1e-6,
        "facing east, a northern dest is on the left: {brg}"
    );

    // The naive form -- dropping the two cancelling 180s, i.e. a plain compass bearing
    // subtracted from BeamTel's heading -- mirrors the answer front to back. Asserting
    // that is what stops this whole scenario passing for free.
    fn naive(px: f64, py: f64, heading: f64, dx: f64, dy: f64) -> f64 {
        use beam_base::pyfmt::py_mod;
        let target = py_mod((dx - px).atan2(dy - py).to_degrees(), 360.0);
        let err = heading - target;
        py_mod(err + 180.0, 360.0) - 180.0
    }
    let n = naive(0.0, 0.0, NORTH, 0.0, 100.0);
    assert!(
        (n.abs() - 180.0).abs() < 1e-6,
        "the naive (un-offset) form answers 180 deg away from the truth: {n}"
    );

    // Wrap at the 0/360 seam must not produce a 359-degree "turn".
    for heading in [0.0, 1.0, 359.0, 360.0] {
        let (_, b) = rb::relative_bearing(0.0, 0.0, heading, 5.0, 5.0);
        assert!(
            (-180.0..=180.0).contains(&b),
            "heading {heading} stays within +/-180: {b}"
        );
    }
}

// =============================================================================
// 3. The shared helper really is shared
// =============================================================================
#[test]
fn scenario_3_shared_helper_lives_in_route_beacon() {
    let src =
        std::fs::read_to_string(repo_root().join("rust/crates/beam-base/src/route_beacon.rs"))
            .unwrap();
    assert!(
        src.contains("(-dx).atan2(-dy)"),
        "...and lives in route_beacon instead"
    );
}

/// The Python sim reads beamtel.py by AST: at least three calls to relative_bearing
/// (the waypoint readout, the beacon toggle and the telemetry loop), and the old
/// open-coded `atan2(-dx, -dy)` gone. The Rust equivalent reads beam-engine's sources.
#[test]
#[ignore = "enable once beam-engine ports the waypoint readout, beacon toggle and telemetry loop"]
fn scenario_3_engine_uses_shared_helper() {
    let mut calls = 0;
    let mut open_coded = Vec::new();
    let mut stack = vec![repo_root().join("rust/crates/beam-engine/src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                calls += text.matches("relative_bearing(").count();
                if text.contains("(-dx).atan2(-dy)") {
                    open_coded.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        calls >= 3,
        "the engine calls relative_bearing in at least three places, found {calls}"
    );
    assert!(
        open_coded.is_empty(),
        "the open-coded atan2(-dx, -dy) waypoint bearing is gone from the engine: {open_coded:?}"
    );
}

// 4. Rate map, and 5. Audio gating and teardown: beam-audio (see the module doc).

// =============================================================================
// 6. Distance wording
// =============================================================================
#[test]
fn scenario_6_wording() {
    assert_eq!(
        rb::format_long_distance(100.0, "imperial"),
        "328 feet",
        "short imperial stays in feet"
    );
    assert_eq!(
        rb::format_long_distance(5000.0, "imperial"),
        "3.1 miles",
        "long imperial switches to miles"
    );
    assert_eq!(
        rb::format_long_distance(500.0, "metric"),
        "500 meters",
        "short metric stays in meters"
    );
    assert_eq!(
        rb::format_long_distance(5000.0, "metric"),
        "5.0 kilometers",
        "long metric switches to km"
    );

    // The hinge must not produce an unreadable figure on either side of itself.
    let just_under = rb::format_long_distance(1000.0 / 3.28084 - 1.0, "imperial");
    let just_over = rb::format_long_distance(1000.0 / 3.28084 + 1.0, "imperial");
    assert!(
        just_under.ends_with("feet") && just_over.ends_with("miles"),
        "imperial hinge crosses feet -> miles exactly once: {just_under} / {just_over}"
    );

    // (The check that road_guidance.format_road_distance would answer "16404 feet"
    // belongs to beam-road.)

    let phrase = rb::route_beacon_phrase(3000.0, 30.0, 4200.0, "imperial").unwrap();
    assert!(
        phrase.contains("1.9 miles")
            && phrase.contains("left")
            && phrase.contains("2.6 miles by road"),
        "the toggle sentence names distance, side and the road distance: {phrase}"
    );

    assert!(
        rb::route_beacon_phrase(3000.0, 2.0, 3100.0, "metric")
            .unwrap()
            .contains("straight ahead"),
        "a target dead ahead is worded as such"
    );
    assert!(
        rb::route_beacon_phrase(3000.0, 179.0, 3100.0, "metric")
            .unwrap()
            .contains("behind you"),
        "a target behind is worded as such"
    );

    // Route distance is omitted rather than spoken as zero when the mod could not give one.
    let no_route = rb::route_beacon_phrase(500.0, 0.0, 0.0, "metric").unwrap();
    assert!(
        !no_route.contains("by road"),
        "an absent road distance is omitted, not spoken as zero: {no_route}"
    );

    assert!(
        rb::route_beacon_phrase(2.0, 0.0, 0.0, "metric")
            .unwrap()
            .contains("at the destination"),
        "standing on the destination says so"
    );
}
