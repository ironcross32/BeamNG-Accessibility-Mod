//! Port of `diagnostic/wheel_slip_sim.py`: wheel slip / side-slide detection and voice
//! placement.
//!
//! Scenario 1 replays a real recording: an ETK C on the Ice Pool, three clutch dumps and
//! several brake lockups (`diagnostic/data/slip_ice_etkc.csv`). The same recording is
//! run through the LEGACY rule, which must show the ~1 s latency the driver reported, so
//! no check can pass for free.
//!
//! Scenario 7 (audio: voices render, localise, fade, pitch follows the ratio) drives
//! `audio.AudioController` and belongs to the beam-audio port; it is not here.

use beam_reports::wheel_slip as ws;
use beam_testutil::repo_root;

struct Row {
    t: f64,
    ground: f64,
    elec: f64,
    wmax: f64,
    wmin: f64,
    reverse: bool,
}

fn load_rows() -> Vec<Row> {
    let path = repo_root()
        .join("diagnostic")
        .join("data")
        .join("slip_ice_etkc.csv");
    let text = std::fs::read_to_string(&path).expect("read slip_ice_etkc.csv");
    let mut rows = Vec::new();
    for line in text.lines() {
        let p: Vec<&str> = line.trim().split(',').collect();
        if p.len() < 10 {
            continue;
        }
        let num = |s: &str| s.parse::<f64>().expect("number in recording");
        let wheels: Vec<f64> = p[9].split('|').map(num).collect();
        let wmin = wheels
            .iter()
            .copied()
            .fold(f64::INFINITY, |a, b| if b < a { b } else { a });
        rows.push(Row {
            t: num(p[0]),
            ground: num(p[1]),
            elec: num(p[2]),
            wmax: num(p[3]),
            wmin,
            reverse: p[7] == "-1",
        });
    }
    rows
}

/// (kind, start, end) episodes of activity.
fn episodes(rows: &[Row], legacy: bool) -> Vec<(i32, f64, f64)> {
    let mut det = ws::SlipDetector::new();
    let mut eps = Vec::new();
    let mut cur: Option<(i32, f64)> = None;
    for r in rows {
        // +1 s: the detector treats a zero timestamp as "no previous sample".
        let res = det.step(
            r.t + 1.0,
            r.ground,
            None, // the probe recorded speed only; straight-line dumps
            Some(if legacy { -1.0 } else { r.wmax }),
            Some(if legacy { -1.0 } else { r.wmin }),
            Some(r.elec),
            r.reverse,
        );
        let kind = if res.active { res.kind } else { 0 };
        if kind != cur.map(|c| c.0).unwrap_or(0) {
            if let Some((k, start)) = cur {
                eps.push((k, start, r.t));
            }
            cur = if kind != 0 { Some((kind, r.t)) } else { None };
        }
    }
    if let Some((k, start)) = cur {
        eps.push((k, start, rows.last().unwrap().t));
    }
    eps
}

fn first(eps: &[(i32, f64, f64)], kind: i32, after: f64, before: f64) -> Option<f64> {
    eps.iter()
        .find(|(k, start, _)| *k == kind && after <= *start && *start <= before)
        .map(|e| e.1)
}

fn onset(rows: &[Row], after: f64, test: impl Fn(&Row) -> bool) -> Option<f64> {
    rows.iter().find(|r| r.t >= after && test(r)).map(|r| r.t)
}

#[test]
fn scenario_1_ice_pool_recording_detection_latency() {
    let rows = load_rows();
    let new = episodes(&rows, false);
    let old = episodes(&rows, true);
    let spin = |r: &Row| r.wmax - r.ground > ws::SLIP_ABS_THRESHOLD_MS;

    for (label, after) in [
        ("1st gear dump", 16.0),
        ("2nd gear dump", 22.5),
        ("rolling dump", 36.5),
    ] {
        let onset = onset(&rows, after, spin).expect("onset");
        let fired = first(&new, ws::KIND_SPIN, onset, onset + 2.0);
        let legacy = first(&old, ws::KIND_SPIN, onset, onset + 2.0);
        assert!(
            fired.is_some_and(|f| f - onset <= 0.12),
            "{label}: new fires within 0.12 s of onset: onset {onset} fired {fired:?}"
        );
        assert!(
            legacy.is_some_and(|l| l - onset >= 0.2),
            "{label}: legacy is slower (the reported bug): onset {onset} legacy {legacy:?}"
        );
    }
    // The two standing starts are where the ground gate cost a full second.
    for after in [16.0, 22.5] {
        let onset = onset(&rows, after, spin).expect("onset");
        let legacy = first(&old, ws::KIND_SPIN, onset, onset + 2.0).expect("legacy fired");
        assert!(
            legacy - onset > 0.9,
            "legacy standing start at {after}s is ~1 s late: {:.3}",
            legacy - onset
        );
    }

    // Brake lockup at ~19.7 s, released again by ~20.03 s. The legacy rule only reported
    // it as the wheels spun up.
    let release = 20.03;
    let lock_new = first(&new, ws::KIND_LOCK, 19.5, 20.5);
    let lock_old = first(&old, ws::KIND_LOCK, 19.5, 20.5);
    assert!(
        lock_new.is_some_and(|l| l < release),
        "brake lockup detected while the wheels are locked: {lock_new:?}"
    );
    assert!(
        lock_old.is_some_and(|l| l >= release - 0.01),
        "legacy reported it only after release: {lock_old:?}"
    );

    // Nothing before the first dump: idling, clutch in, revving at a standstill.
    assert!(
        new.iter().all(|(_k, start, _e)| *start >= 16.7),
        "no activation before the first dump: {:?}",
        &new[..new.len().min(2)]
    );
}

#[test]
fn scenario_2_sideways_slide_is_a_slide_not_a_lockup() {
    let mut det = ws::SlipDetector::new();
    // 10 m/s at 45 degrees off the nose, wheels rolling with the longitudinal component.
    let v_long = 10.0 * 45.0f64.to_radians().cos();
    let mut res = ws::INACTIVE;
    for i in 0..30 {
        res = det.step(
            1.0 + i as f64 / 60.0,
            10.0,
            Some(45.0),
            Some(v_long),
            Some(v_long),
            Some(v_long),
            false,
        );
    }
    assert!(!res.active, "no lockup: {res:?}");
    assert!(
        res.slide_active && (res.slide_deg - 45.0).abs() < 1e-6,
        "slide active: {res:?}"
    );
    assert!(
        (res.azimuth_deg - 45.0).abs() < 1e-6,
        "placed toward the travel (left): {}",
        res.azimuth_deg
    );
    let naive = 10.0 - v_long;
    assert!(
        naive > ws::threshold_for_tests(10.0),
        "naive total-speed rule would call it lockup: {naive:.2}"
    );

    // Hysteresis: 10 degrees keeps a slide on, and cannot start one.
    for i in 0..30 {
        res = det.step(
            2.0 + i as f64 / 60.0,
            10.0,
            Some(10.0),
            Some(10.0),
            Some(10.0),
            Some(10.0),
            false,
        );
    }
    assert!(res.slide_active, "held at 10 deg once sliding");
    let mut fresh = ws::SlipDetector::new();
    for i in 0..30 {
        res = fresh.step(
            1.0 + i as f64 / 60.0,
            10.0,
            Some(10.0),
            Some(10.0),
            Some(10.0),
            Some(10.0),
            false,
        );
    }
    assert!(!res.slide_active, "10 deg does not start a slide");
}

#[test]
fn scenario_3_voice_placement() {
    assert_eq!(
        ws::voice_azimuth(Some(30.0), 10.0, false),
        30.0,
        "+30 travel -> +30 (left)"
    );
    assert_eq!(
        ws::voice_azimuth(Some(-30.0), 10.0, false),
        -30.0,
        "-30 travel -> -30 (right)"
    );
    assert_eq!(
        ws::voice_azimuth(Some(5.0), 10.0, false),
        0.0,
        "5 deg snaps dead ahead"
    );
    assert_eq!(
        ws::voice_azimuth(Some(175.0), 10.0, true),
        180.0,
        "175 deg snaps dead behind"
    );
    assert_eq!(
        ws::voice_azimuth(Some(90.0), 0.1, false),
        0.0,
        "standstill forward gear -> ahead"
    );
    assert_eq!(
        ws::voice_azimuth(Some(90.0), 0.1, true),
        180.0,
        "standstill reverse -> behind"
    );
    assert_eq!(
        ws::voice_azimuth(None, 5.0, true),
        180.0,
        "no direction -> gear"
    );
    // A sweep behind the car must stay behind: never pass through the front hemisphere.
    let worst = [150.0, 160.0, 170.0, 180.0, -170.0, -160.0, -150.0]
        .iter()
        .map(|b| ws::voice_azimuth(Some(*b), 10.0, true).abs())
        .fold(f64::INFINITY, f64::min);
    assert!(worst >= 150.0, "sweep across 180 stays behind: {worst}");
    // Blend region stays between the two ends rather than flipping sides.
    let mid = ws::voice_azimuth(Some(60.0), 1.25, false);
    assert!(
        0.0 < mid && mid < 60.0,
        "blend is between gear and travel: {mid}"
    );
}

#[test]
fn scenario_4_travel_bearing_sign() {
    // Facing north is heading 180 here (see route_beacon::relative_bearing).
    assert!(
        ws::travel_bearing(0.0, 5.0, 180.0).abs() < 1e-6,
        "north-facing, moving north -> 0"
    );
    assert!(
        (ws::travel_bearing(-5.0, 0.0, 180.0) - 90.0).abs() < 1e-6,
        "north-facing, moving west -> +90"
    );
    assert!(
        (ws::travel_bearing(5.0, 0.0, 180.0) + 90.0).abs() < 1e-6,
        "north-facing, moving east -> -90"
    );
    assert!(
        (ws::travel_bearing(0.0, -5.0, 180.0).abs() - 180.0).abs() < 1e-6,
        "north-facing, moving south -> 180"
    );
    // Naive: a true-compass bearing of the velocity against BeamTel's offset heading
    // mirrors it front to back -- driving forward would place the voice behind you.
    let naive = beam_base::route_beacon::normalize_bearing(180.0 - 0.0f64.atan2(5.0).to_degrees());
    assert!(
        (naive.abs() - 180.0).abs() < 1e-6,
        "naive un-offset form reads forward travel as behind: {naive}"
    );
}

#[test]
fn scenario_5_standstill_spin_standstill_lockup_legacy_fallback() {
    let mut det = ws::SlipDetector::new();
    let mut res = ws::INACTIVE;
    for i in 0..12 {
        // 0.2 s of 10 m/s wheelspin on a car doing 0.1 m/s
        res = det.step(
            1.0 + i as f64 / 60.0,
            0.1,
            None,
            Some(10.0),
            Some(0.1),
            Some(1.0),
            false,
        );
    }
    assert!(
        res.active && res.kind == ws::KIND_SPIN,
        "standstill spin detected (no ground gate): {res:?}"
    );
    assert_eq!(res.azimuth_deg, 0.0, "standstill spin placed ahead");

    let mut det = ws::SlipDetector::new();
    for i in 0..30 {
        // parked: every wheel 'locked', ground 0
        res = det.step(
            1.0 + i as f64 / 60.0,
            0.0,
            None,
            Some(0.0),
            Some(0.0),
            Some(0.0),
            false,
        );
    }
    assert!(!res.active, "parked car is not a lockup");

    let mut det = ws::SlipDetector::new();
    for i in 0..12 {
        // old mod: same spin, legacy rule gated on ground speed
        res = det.step(
            1.0 + i as f64 / 60.0,
            0.1,
            None,
            Some(-1.0),
            Some(-1.0),
            Some(10.0),
            false,
        );
    }
    assert!(
        res.legacy,
        "legacy path engaged when the wheel fields are -1"
    );
    assert!(!res.active, "legacy keeps its ground gate");
}

fn sample(pairs: &[(&str, f64)]) -> beam_reports::pyops::PyDict {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), beam_reports::pyops::PyValue::Float(*v)))
        .collect()
}

#[test]
fn scenario_6_classify_for_recorded_samples() {
    let new = ws::classify(&sample(&[
        ("ground_speed_ms", 10.0),
        ("travel_bearing_deg", 40.0),
        ("wheel_max_driven_ms", 7.7),
        ("wheel_min_ms", 7.6),
    ]))
    .unwrap();
    assert!(
        new.slide && new.kind == "none",
        "sideways sample: slide, no slip: {new:?}"
    );
    let spin = ws::classify(&sample(&[
        ("ground_speed_ms", 0.2),
        ("wheel_max_driven_ms", 9.0),
        ("wheel_min_ms", 0.2),
    ]))
    .unwrap();
    assert_eq!(spin.kind, "wheelspin", "standstill spin sample: {spin:?}");
    let old = ws::classify(&sample(&[
        ("ground_speed_ms", 5.0),
        ("wheel_speed_ms", 1.0),
    ]))
    .unwrap();
    assert_eq!(
        old.kind, "lockup",
        "old sample (no wheel fields) uses legacy rule: {old:?}"
    );
}

/// `audio.SLIP_MAG_FOR_FULL`, without depending on the audio crate here.
fn audio_mag_full() -> f64 {
    5.0
}

#[test]
fn scenario_6b_slip_ratio_is_measured_against_motionsim_ground_speed() {
    let mut det = ws::SlipDetector::new();
    let mut res = ws::INACTIVE;
    // Standstill, wheels at 22 m/s (the "50 mph on the speedo while parked" case).
    for i in 0..12 {
        res = det.step(
            1.0 + i as f64 / 60.0,
            0.05,
            None,
            Some(22.0),
            Some(0.05),
            Some(22.0),
            false,
        );
    }
    assert!(
        res.active && res.ratio > 0.99,
        "spin in place is ratio ~1: {}",
        res.ratio
    );
    let mut det = ws::SlipDetector::new();
    for i in 0..12 {
        // 27 m/s (60 mph) with 10 m/s of spin
        res = det.step(
            1.0 + i as f64 / 60.0,
            27.0,
            Some(0.0),
            Some(37.0),
            Some(27.0),
            Some(37.0),
            false,
        );
    }
    assert!(
        res.active && 0.2 < res.ratio && res.ratio < 0.35,
        "10 m/s spin at 60 mph is a modest ratio: {}",
        res.ratio
    );
    let naive = (res.magnitude_ms / audio_mag_full()).min(1.0);
    assert!(
        naive >= 1.0,
        "absolute magnitude would have pinned it at the top: {naive}"
    );
    let mut det = ws::SlipDetector::new();
    for i in 0..12 {
        // fully locked at 10 m/s
        res = det.step(
            1.0 + i as f64 / 60.0,
            10.0,
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
            false,
        );
    }
    assert!(
        res.active && res.kind == ws::KIND_LOCK && res.ratio > 0.99,
        "full lockup is ratio 1: {res:?}"
    );
    assert_eq!(ws::INACTIVE.ratio, 0.0, "inactive ratio is 0");
    let c = ws::classify(&sample(&[
        ("ground_speed_ms", 0.2),
        ("wheel_max_driven_ms", 9.0),
        ("wheel_min_ms", 0.2),
    ]))
    .unwrap();
    assert!(c.ratio > 0.95, "classify carries the ratio: {c:?}");
}
