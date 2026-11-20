//! `diagnostic/cannon_shot_sim.py`: the cannon shot outcome readout, against the real
//! `cannon_shot_phrase` (and, for scenario 4's session comparison, the real listener).
//!
//! This readout fires unprompted, once, right after a crash, and it is the whole record of
//! a shot that costs a reset and a re-drive to repeat. Scenario numbers follow the sim;
//! scenario 7 greps the Lua mod exactly as the sim does.

use beam_engine::listeners::cannon::handle_cannon_text;
use beam_engine::listeners::dock_phrases::{
    CANNON_SHOT_APEX_SAY_M, CANNON_SHOT_CENTRE_M, CANNON_SHOT_COMPARE_M, cannon_shot_phrase,
};
use beam_engine::state::{CannonShot, State, state};
use beam_engine::{net, speech_out, units};
use beam_testutil::repo_root;

fn setup() -> parking_lot::MutexGuard<'static, ()> {
    let g = speech_out::TEST_SERIAL.lock();
    speech_out::set_speak_override(Some(Box::new(|_t: &str, _i: bool| {})));
    speech_out::reset_for_tests();
    *state() = State::default();
    units::set_units_mode("metric");
    g
}

fn shot() -> CannonShot {
    CannonShot {
        downrange: 180.0,
        lateral: 0.0,
        apex: 0.0,
        flight: 0.0,
        ramp_pitch: 0.0,
        strength: 0,
        settled: true,
        index: 1,
        near_dist: -1.0,
        near_name: String::new(),
        vehicle: String::new(),
        stamp: 0.0,
        prev_downrange: None,
    }
}

fn with(f: impl FnOnce(&mut CannonShot)) -> String {
    let mut s = shot();
    f(&mut s);
    cannon_shot_phrase(&s)
}

#[test]
fn scenario_1_positive_lateral_is_left() {
    let _g = setup();
    let left = with(|s| s.lateral = 20.0);
    let right = with(|s| s.lateral = -20.0);
    assert!(left.contains("left") && !left.contains("right"), "{left}");
    assert!(
        right.contains("right") && !right.contains("left"),
        "{right}"
    );
    assert_eq!(left.replace("left", "X"), right.replace("right", "X"));
    let mirrored = if -20.0 > 0.0 { "left" } else { "right" };
    assert_eq!(
        mirrored, "right",
        "a mirrored implementation would FAIL this check"
    );
}

#[test]
fn scenario_2_unsettled_shot_has_no_distance() {
    let _g = setup();
    let p = with(|s| {
        s.downrange = 412.0;
        s.lateral = 30.0;
        s.apex = 88.0;
        s.settled = false
    });
    assert_eq!(p, "Shot did not settle");
    assert!(!p.chars().any(|c| c.is_ascii_digit()));
}

#[test]
fn scenario_3_two_numbers_then_notable() {
    let _g = setup();
    let plain = with(|s| {
        s.lateral = 0.5;
        s.apex = 4.0
    });
    assert_eq!(plain.matches('.').count(), 0, "{plain}");
    assert_eq!(plain, "180 meters, straight");
    assert!(with(|s| s.lateral = CANNON_SHOT_CENTRE_M - 0.01).contains("straight"));
    assert!(!with(|s| s.lateral = CANNON_SHOT_CENTRE_M + 0.01).contains("straight"));
    assert!(!with(|s| s.apex = CANNON_SHOT_APEX_SAY_M - 0.01).contains("peaked"));
    assert!(with(|s| s.apex = CANNON_SHOT_APEX_SAY_M + 0.01).contains("peaked"));
    for word in ["good", "on target", "nice", "well", "success"] {
        assert!(
            !plain.to_lowercase().contains(word),
            "it never says {word:?}"
        );
    }
}

#[test]
fn scenario_4_comparison_clause() {
    let _g = setup();
    let first = with(|_| {});
    assert!(!first.contains("further") && !first.contains("shorter"));
    let further = with(|s| {
        s.downrange = 240.0;
        s.prev_downrange = Some(180.0)
    });
    let shorter = with(|s| {
        s.downrange = 120.0;
        s.prev_downrange = Some(180.0)
    });
    assert!(further.contains("60 meters further"), "{further}");
    assert!(shorter.contains("60 meters shorter"), "{shorter}");
    let same = with(|s| {
        s.downrange = 180.0 + CANNON_SHOT_COMPARE_M - 0.01;
        s.prev_downrange = Some(180.0)
    });
    assert!(
        !same.contains("further") && !same.contains("shorter"),
        "{same}"
    );

    // The listener takes the previous shot from the mod's twelfth field when it sends one,
    // and from its own session list otherwise.
    handle_cannon_text("SHOT:180,0,0,1,1,1,1,1,-1,,car", 1.0);
    handle_cannon_text("SHOT:240,0,0,1,1,1,1,2,-1,,car", 2.0);
    handle_cannon_text("SHOT:100,0,0,1,1,1,1,3,-1,,car,40", 3.0);
    let log = speech_out::get_speech_log(None, Some(0), None, None, false);
    let said: Vec<&str> = log.entries.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(
        said,
        vec![
            "180 meters, straight",
            "240 meters, straight. 60 meters further",
            "100 meters, straight. 60 meters further",
        ]
    );
}

#[test]
fn scenario_5_long_distances_are_whole_numbers() {
    let _g = setup();
    units::set_units_mode("imperial");
    let imp = with(|s| s.downrange = 300.0);
    units::set_units_mode("metric");
    assert_eq!(imp, "984 feet, straight");
    assert!(!imp.split(',').next().unwrap().contains('.'), "{imp}");
}

#[test]
fn scenario_6_backwards() {
    let _g = setup();
    let back = with(|s| s.downrange = -40.0);
    assert!(back.contains("backwards"), "{back}");
    assert!(!back.contains("left") && !back.contains("right"), "{back}");
}

fn value_of(text: &str, name: &str) -> Option<String> {
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.starts_with(&format!("local {name} "))
            || stripped.starts_with(&format!("local {name}="))
        {
            let v = stripped.split_once('=')?.1;
            return Some(v.split("--").next().unwrap_or("").trim().to_string());
        }
    }
    None
}

#[test]
fn scenario_7_mod_agrees_with_the_rest_of_the_mod() {
    let ext = repo_root().join("bng_mod/lua/ge/extensions");
    let m = std::fs::read_to_string(ext.join("cannonShot.lua")).expect("cannonShot.lua");
    let spawner = std::fs::read_to_string(ext.join("vehicleSpawnerAccessible.lua"))
        .expect("vehicleSpawnerAccessible.lua");
    assert!(m.contains("frame.left") && !m.contains("fwd:cross"));
    assert!(
        m.contains("axis      = vec3(frame.axis)") && m.contains("left      = vec3(frame.left)"),
        "the launch frame is copied, not held by reference"
    );
    for c in [
        "LAUNCH_TRACK_TIMEOUT",
        "LAUNCH_SETTLE_SPEED",
        "LAUNCH_SETTLE_TIME",
    ] {
        let (a, b) = (value_of(&m, c), value_of(&spawner, c));
        assert!(
            a.is_some() && a == b,
            "{c} matches the spawner's copy: {a:?} vs {b:?}"
        );
    }
    assert!(m.contains("rg.isCannon(id)") && !m.contains("rg.has("));
    assert!(m.contains("simAcc = simAcc + dtSim") && m.contains("local dt = simAcc"));
    assert!(m.contains("if dt <= 0 then return end"));
    assert!(spawner.contains("pcall(updateLaunchTrack, dtSim)"));
    assert!(
        !m.contains("CMD_LISTEN_PORT"),
        "the mod opens no command socket"
    );
    assert!(m.contains("PYTHON_PORT_DATA = 4473"));
    assert_eq!(
        net::CANNON_SHOT_LISTEN_PORT,
        4473,
        "...and sends on the port beamtel listens on"
    );
}
