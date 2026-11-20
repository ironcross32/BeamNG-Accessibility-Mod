//! `diagnostic/route_beacon_sim.py`, the engine half. Scenarios 1, 2 and 6 (protocol,
//! bearing sign, wording) test `route_beacon.py` and live in beam-base's
//! `tests/route_beacon_sim.rs`; 4 and 5 (rate map, audio gating) are beam-audio's. What is
//! left is scenario 3's beamtel half (the engine calls the shared helper and never open-codes
//! the bearing) plus the listener and toggle that sit on top of the protocol.

use beam_audio::controller::AudioController;
use beam_engine::listeners::route_beacon::{handle_route_text, route_is_set, toggle_route_beacon};
use beam_engine::state::{State, state};
use beam_engine::timer::monotonic;
use beam_engine::{speech_out, units};
use beam_testutil::repo_root;

fn engine_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![repo_root().join("rust/crates/beam-engine/src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((
                    path.display().to_string(),
                    std::fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }
    out
}

/// The part of scenario 3 that is already decidable: the beacon toggle calls the shared
/// helper and no engine file open-codes `atan2(-dx, -dy)`.
#[test]
fn scenario_3_toggle_uses_shared_helper() {
    let src = std::fs::read_to_string(
        repo_root().join("rust/crates/beam-engine/src/listeners/route_beacon.rs"),
    )
    .unwrap();
    assert!(src.contains("relative_bearing("));
    let open: Vec<String> = engine_sources()
        .into_iter()
        .filter(|(_, t)| t.contains("(-dx).atan2(-dy)"))
        .map(|(p, _)| p)
        .collect();
    assert!(open.is_empty(), "open-coded bearing in {open:?}");
}

/// Scenario 3 as the sim states it: at least three callers (the waypoint readout, the beacon
/// toggle and the telemetry loop). The F9 W waypoint readout is E6's.
#[test]
fn scenario_3_engine_calls_relative_bearing_three_times() {
    let calls: usize = engine_sources()
        .iter()
        .map(|(_, t)| t.matches("relative_bearing(").count())
        .sum();
    assert!(calls >= 3, "found {calls}");
}

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let g = speech_out::TEST_SERIAL.lock();
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    speech_out::reset_for_tests();
    *state() = State::default();
    units::set_units_mode("imperial");
    g
}

fn said() -> Vec<String> {
    let log = speech_out::get_speech_log(None, Some(0), None, None, false);
    speech_out::reset_for_tests();
    log.entries.iter().map(|e| e.text.clone()).collect()
}

#[test]
fn listener_route_set_clear_and_age_out() {
    let _g = setup();
    let now = monotonic();
    handle_route_text("ROUTE:1200.5,-340.25,61.0,4821.5", now);
    {
        let st = state();
        assert_eq!(st.route.route_dest_x, Some(1200.5));
        assert_eq!(st.route.route_dest_y, Some(-340.25));
        assert_eq!(st.route.route_remaining_m, 4821.5);
        assert!(route_is_set(&st));
    }
    // A malformed packet is logged and changes nothing.
    handle_route_text("ROUTE:1,2,nan,4", now);
    assert_eq!(state().route.route_dest_x, Some(1200.5));
    // CLEAR is a message, not an absence: it takes effect at once.
    handle_route_text("ROUTE:CLEAR", now);
    assert!(!route_is_set(&state()));
    // ...and the feed ages out where it is read.
    handle_route_text("ROUTE:1,2,3,4", now - 2.0);
    assert!(!route_is_set(&state()));
}

#[test]
fn toggle_refuses_without_a_route_and_names_the_side() {
    let _g = setup();
    let ac = AudioController::new();
    toggle_route_beacon(ac.clone());
    assert_eq!(
        said(),
        vec!["No route set. Choose a destination on the map first."]
    );
    assert!(!state().route.route_beacon_active);
    assert!(!ac.lock.lock()._route_beacon_mode_active);

    // Facing north (heading 180 in BeamTel's frame), a destination due west is LEFT.
    {
        let mut st = state();
        st.tel.last_heading = 180.0;
    }
    handle_route_text("ROUTE:-5000.0,0.0,0.0,6000.0", monotonic());
    toggle_route_beacon(ac.clone());
    let s = said();
    assert_eq!(s.len(), 1);
    assert!(
        s[0].contains("left") && s[0].contains("by road"),
        "{}",
        s[0]
    );
    assert!(state().route.route_beacon_active);
    assert!(ac.lock.lock()._route_beacon_mode_active);

    toggle_route_beacon(ac.clone());
    assert_eq!(said(), vec!["Route beacon off"]);
    assert!(!ac.lock.lock()._route_beacon_mode_active);
}
