//! Port of `diagnostic/camera_lineup_sim.py`: the free-camera line-up guide's
//! conventions -- which side a miss is on, that the ratio dips for LEFT, that the sound
//! sits on the side of the error, that a target behind the nose reads as full error from
//! behind, and the spoken wording.
//!
//! World: +x east, +y north. Camera at origin looking north. Up x forward for north is
//! WEST, so the camera's left is -x.

use beam_reports::camera_lineup as cl;

fn geom(cam: (f64, f64), cam_f: (f64, f64), nose: (f64, f64), veh_f: (f64, f64)) -> cl::Geometry {
    cl::Geometry {
        cam_x: cam.0,
        cam_y: cam.1,
        cam_fx: cam_f.0,
        cam_fy: cam_f.1,
        nose_x: nose.0,
        nose_y: nose.1,
        veh_fx: veh_f.0,
        veh_fy: veh_f.1,
    }
}

fn g() -> cl::Geometry {
    geom((0.0, 0.0), (0.0, 1.0), (0.0, 50.0), (0.0, -1.0))
}
fn g_nose(nose: (f64, f64)) -> cl::Geometry {
    geom((0.0, 0.0), (0.0, 1.0), nose, (0.0, -1.0))
}
fn g_veh(veh_f: (f64, f64)) -> cl::Geometry {
    geom((0.0, 0.0), (0.0, 1.0), (0.0, 50.0), veh_f)
}

#[test]
fn scenario_1_head_on_dead_centre() {
    let (st, m) = cl::miss_left_m(&g(), cl::HEAD_ON, 0.0, 10.0);
    assert!(
        st == "ok" && m.unwrap().abs() < 1e-9,
        "1 head-on dead centre is ok and zero: {st} {m:?}"
    );
    assert_eq!(
        cl::tone_for(st, m, None).amp,
        0.0,
        "1 dead centre is silent"
    );
}

#[test]
fn scenario_2_target_on_cameras_left_misses_left() {
    let (st, m) = cl::miss_left_m(&g(), cl::HEAD_ON, 3.0, 10.0);
    assert!(
        st == "ok" && (m.unwrap() - 3.0).abs() < 1e-9,
        "2 target on camera's left: path misses LEFT by 3 m: {m:?}"
    );
    let t = cl::tone_for(st, m, None);
    assert!(t.ratio < 1.0, "2 ratio dips below 1 for left: {t:?}");
    assert!(0.0 < t.az && t.az < 180.0, "2 sound on the left: {}", t.az);
    assert!(
        cl::INDEX_MIN < t.index && t.index < cl::INDEX_MAX,
        "2 index between 1.0 and 1.3: {}",
        t.index
    );
}

#[test]
fn scenario_3_car_moved_onto_the_offset_line_is_aligned() {
    let (_st, m) = cl::miss_left_m(&g_nose((-3.0, 50.0)), cl::HEAD_ON, 3.0, 10.0);
    assert!(
        m.unwrap().abs() < 1e-9,
        "3 car moved onto the offset line is aligned: {m:?}"
    );
}

#[test]
fn scenario_4_right_miss() {
    let (st, m) = cl::miss_left_m(&g_nose((-6.0, 50.0)), cl::HEAD_ON, 3.0, 10.0);
    let t = cl::tone_for(st, m, None);
    assert!(m.unwrap() < 0.0, "4 path misses RIGHT: {m:?}");
    assert!(t.ratio > 1.0, "4 ratio above 1 for right: {t:?}");
    assert!(
        180.0 < t.az && t.az < 360.0,
        "4 sound on the right: {}",
        t.az
    );
}

#[test]
fn scenario_5_full_scale_and_deadzone() {
    let t = cl::tone_for("ok", Some(cl::FULL_M + 5.0), None);
    assert!(
        (t.amp - 1.0).abs() < 1e-9
            && (t.index - cl::INDEX_MAX).abs() < 1e-9
            && (t.ratio - (1.0 - cl::RATIO_DRIFT)).abs() < 1e-9
            && (t.az - 90.0).abs() < 1e-9,
        "5 full scale clamps: {t:?}"
    );
    assert_eq!(
        cl::tone_for("ok", Some(cl::DEADZONE_M * 0.9), None).amp,
        0.0,
        "5 inside deadzone silent"
    );
}

#[test]
fn scenario_6_heading_error_grows_with_distance() {
    let yaw = 5.0f64.to_radians();
    let vf = (yaw.sin(), -yaw.cos()); // turned toward +x (east) = the car's LEFT when facing south
    let (_st, m) = cl::miss_left_m(&g_veh(vf), cl::HEAD_ON, 0.0, 10.0);
    let m = m.unwrap();
    assert!(
        (m - 50.0 * yaw.sin()).abs() < 1e-6,
        "6 heading error shows as miss: {m}"
    );
    assert!(m > 0.0, "6 turned to its left misses left: {m}");
}

#[test]
fn scenario_7_pointed_away_is_behind() {
    let (st, m) = cl::miss_left_m(&g_veh((0.0, 1.0)), cl::HEAD_ON, 0.0, 10.0);
    let t = cl::tone_for(st, m, None);
    assert!(
        st == "behind" && m.is_none(),
        "7 pointed away is behind: {st}"
    );
    assert!(
        t.amp == 1.0 && t.az == 180.0,
        "7 behind is full and from behind: {t:?}"
    );
}

#[test]
fn scenario_8_crossing() {
    let gg = geom((0.0, 0.0), (0.0, 1.0), (-30.0, 10.0), (1.0, 0.0));
    let (st, m) = cl::miss_left_m(&gg, cl::CROSSING, 0.0, 10.0);
    assert!(
        st == "ok" && m.unwrap().abs() < 1e-9,
        "8 crossing at depth is aligned: {m:?}"
    );
    let gg = geom((0.0, 0.0), (0.0, 1.0), (-30.0, 14.0), (1.0, 0.0));
    let (_st, m) = cl::miss_left_m(&gg, cl::CROSSING, 0.0, 10.0);
    // Driving east, the car's left is NORTH. Path at y=14 is 4 m north of the target: LEFT.
    assert!(
        (m.unwrap() - 4.0).abs() < 1e-9,
        "8 crossing too far out misses LEFT: {m:?}"
    );
    assert_eq!(
        cl::miss_left_m(&gg, cl::CROSSING, 7.0, 10.0).1,
        m,
        "8 crossing ignores the head-on offset"
    );
    let gg = geom(
        (0.0, 0.0),
        (0.0, 1.0),
        (-30.0, 0.0),
        (0.70710678, 0.70710678),
    );
    let (_st, m) = cl::miss_left_m(&gg, cl::CROSSING, 0.0, 30.0);
    assert!(
        m.unwrap().abs() < 1e-6,
        "8 diagonal pass through the depth point counts: {m:?}"
    );
}

#[test]
fn scenario_9_degenerate_frames_are_unknown_and_silent() {
    let (st, m) = cl::miss_left_m(
        &geom((0.0, 0.0), (0.0, 0.0), (0.0, 50.0), (0.0, -1.0)),
        cl::HEAD_ON,
        0.0,
        10.0,
    );
    assert!(
        st == "unknown" && cl::tone_for(st, m, None).amp == 0.0,
        "9 degenerate camera is unknown and silent"
    );
}

#[test]
fn scenario_10_stepping_and_clamping() {
    let (o, d) = cl::step_value(cl::HEAD_ON, 0.0, 10.0, -1);
    assert!(
        o == -1.0 && d == 10.0,
        "10 head-on steps through zero to the right: {o} {d}"
    );
    let (_o, d) = cl::step_value(cl::CROSSING, 0.0, cl::DEPTH_MIN_M, -1);
    assert_eq!(d, cl::DEPTH_MIN_M, "10 depth clamps at minimum");
}

#[test]
fn scenario_11_wording() {
    assert_eq!(
        cl::value_phrase(cl::HEAD_ON, 3.0, 10.0, "metric").unwrap(),
        "3 meters left",
        "11 left"
    );
    assert_eq!(
        cl::value_phrase(cl::HEAD_ON, -1.0, 10.0, "metric").unwrap(),
        "1 meter right",
        "11 right"
    );
    assert_eq!(
        cl::value_phrase(cl::HEAD_ON, 0.0, 10.0, "metric").unwrap(),
        "through the camera",
        "11 zero"
    );
    assert_eq!(
        cl::value_phrase(cl::CROSSING, 0.0, 12.0, "metric").unwrap(),
        "12 meters out",
        "11 depth"
    );
    assert_eq!(
        cl::value_phrase(cl::CROSSING, 0.0, 3.0, "imperial").unwrap(),
        "10 feet out",
        "11 feet"
    );
    assert_eq!(
        cl::target_phrase(cl::HEAD_ON, 3.0, 10.0, "metric").unwrap(),
        "Head-on, 3 meters left of the camera",
        "11 target"
    );
}

#[test]
fn scenario_12_packet_parsing() {
    assert_eq!(
        cl::parse_packet("LINEUP:OFF"),
        Some(cl::LineupPacket::Off),
        "12 OFF"
    );
    let p = cl::parse_packet("LINEUP:1,2,0,1,3,4,0,-1");
    assert!(
        matches!(p, Some(cl::LineupPacket::Geometry(g)) if g.nose_y == 4.0),
        "12 live: {p:?}"
    );
    assert!(
        cl::parse_packet("LINEUP:1,2,3").is_none(),
        "12 short packet rejected"
    );
    assert!(
        matches!(
            cl::parse_packet("LINEUP:1,2,0,1,3,4,0,-1,extra"),
            Some(cl::LineupPacket::Geometry(_))
        ),
        "12 tail tolerated"
    );
    assert!(
        cl::parse_packet("LINEUP:nan,2,0,1,3,4,0,-1").is_none(),
        "12 nan rejected"
    );
}

#[test]
fn scenario_13_position_follows_the_bearing() {
    // Target 3 m on the camera's left, car 50 m out: target is 3.4 deg to the car's
    // RIGHT, the path misses LEFT, so the sound sits 3.4 deg LEFT.
    let (st, m, b) = cl::solve(&g(), cl::HEAD_ON, 3.0, 10.0);
    let t = cl::tone_for(st, m, b);
    let b = b.unwrap();
    assert!(
        b < 0.0,
        "13 bearing negative for a target on the right: {b}"
    );
    assert!(
        (t.az - (-b)).abs() < 1e-9 && 0.0 < t.az && t.az < 90.0,
        "13 sound mirrored to the error side: {}",
        t.az
    );
    // Target abeam on the car's right (car beside the camera, driving past it).
    let (st, _m, b) = cl::solve(
        &geom((0.0, 0.0), (0.0, 1.0), (3.0, 0.0), (0.0, 1.0)),
        cl::HEAD_ON,
        0.0,
        10.0,
    );
    assert!(
        st == "behind" && (b.unwrap().abs() - 90.0).abs() < 1e-6,
        "13 target abeam is 'behind' at 90 degrees: {st} {b:?}"
    );
    // Pointed straight away: sound straight behind.
    let (st, m, b) = cl::solve(&g_veh((0.0, 1.0)), cl::HEAD_ON, 0.0, 10.0);
    let t = cl::tone_for(st, m, b);
    assert!(
        (t.az - 180.0).abs() < 1e-6,
        "13 pointed away sounds from behind: {}",
        t.az
    );
    // Continuity: sweeping the heading round, the azimuth never jumps more than the
    // heading moved (away from the exact rear, where left and right meet at 180).
    let mut prev: Option<f64> = None;
    let mut worst = 0.0f64;
    for deg10 in (-1700..=1700).step_by(5) {
        let yaw = (deg10 as f64 / 10.0).to_radians();
        let vf = (yaw.sin(), -yaw.cos());
        let (st, m, b) = cl::solve(&g_veh(vf), cl::HEAD_ON, 3.0, 10.0);
        let tone = cl::tone_for(st, m, b);
        let az = tone.az;
        if let Some(p) = prev {
            if tone.amp > 0.0 {
                let d = (beam_base::pyfmt::py_mod((az - p) + 180.0, 360.0) - 180.0).abs();
                worst = worst.max(d);
            }
        }
        prev = Some(az);
    }
    assert!(
        worst < 2.0,
        "13 azimuth continuous as the car turns: {worst}"
    );
}

#[test]
fn scenario_14_ratio_drift_follows_the_angle() {
    for yaw_deg in [10.0f64, 45.0, 90.0, 150.0] {
        let yaw = yaw_deg.to_radians();
        let vf = (yaw.sin(), -yaw.cos()); // turned to the car's LEFT, so it misses LEFT
        let (st, m, b) = cl::solve(&g_veh(vf), cl::HEAD_ON, 0.0, 10.0);
        let t = cl::tone_for(st, m, b);
        let want = 1.0 - cl::RATIO_DRIFT * b.unwrap().abs() / 180.0;
        assert!(
            (t.ratio - want).abs() < 1e-9,
            "14 ratio at {yaw_deg:.0} deg turn is {want:.4}: {}",
            t.ratio
        );
    }
    let (st, m, b) = cl::solve(&g_veh((0.001, 1.0)), cl::HEAD_ON, 0.0, 10.0);
    let t = cl::tone_for(st, m, b);
    assert!(
        ((t.ratio - 1.0).abs() - cl::RATIO_DRIFT).abs() < 1e-4,
        "14 ratio at the full drift straight behind: {}",
        t.ratio
    );
    let (st, m, b) = cl::solve(
        &g_veh((-(0.5f64.sin()), -(0.5f64.cos()))),
        cl::HEAD_ON,
        0.0,
        10.0,
    );
    assert!(
        cl::tone_for(st, m, b).ratio > 1.0,
        "14 right miss drifts above 1"
    );
}
