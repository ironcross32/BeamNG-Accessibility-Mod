//! `diagnostic/route_beacon_sim.py`, the audio half: scenario 4 (rate map) and scenario 5
//! (audio gating and teardown), same numbers and order. Scenarios 1-3 and 6 (protocol,
//! bearing sign, the shared helper, wording) test `route_beacon.py` and `beamtel.py` and
//! live with those ports.
//!
//! Python's `FakeHRTF` records the bearing it is asked for; here a probe `HrtfSet` whose
//! right-ear gain at each whole degree is `degree / 360` names the azimuth through the
//! L/R ratio of the convolved pulse.

mod audio_harness;

use std::sync::Arc;

use audio_harness::harness;
use beam_audio::callback::common::Wave;
use beam_audio::consts::*;
use beam_audio::hrtf::HrtfSet;

fn check(label: &str, condition: bool, detail: &str) {
    assert!(condition, "{label} FAILED {detail}");
    eprintln!("  ok  {label}");
}

fn max_abs(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, &v| m.max(v.abs()))
}

// ==============================================================================
// 4. Rate map
// ==============================================================================
#[test]
fn scenario_4_rate() {
    eprintln!("4. route beacon pulse rate");
    let near = route_beacon_rate_hz(0.0);
    let mid = route_beacon_rate_hz(400.0);
    let far = route_beacon_rate_hz(5000.0);

    check(
        "rate falls monotonically with distance",
        near > mid && mid > far,
        &format!("{near} {mid} {far}"),
    );
    check(
        "closest rate is the maximum",
        (near - ROUTE_BEACON_MAX_RATE_HZ).abs() < 1e-9,
        "",
    );
    check(
        "far rate approaches the minimum",
        (far - ROUTE_BEACON_MIN_RATE_HZ).abs() < 0.01,
        "",
    );

    // On the road beacon's 20 m half-distance a 400 m destination is pinned at the floor;
    // on the route scale it still has most of its range left.
    let road_at_400 = road_beacon_rate_hz(400.0);
    check(
        "at 400 m the road scale is already pinned at its floor",
        (road_at_400 - ROAD_BEEP_MIN_RATE_HZ).abs() < 0.001,
        &format!("{road_at_400}"),
    );
    check(
        "...while the route scale still has range to give",
        mid > ROUTE_BEACON_MIN_RATE_HZ + 0.5,
        &format!("{mid}"),
    );
    check(
        "the two beacons cannot be confused by rate alone at the same distance",
        (route_beacon_rate_hz(100.0) - road_beacon_rate_hz(100.0)).abs() > 0.5,
        "",
    );

    // Both beacons can sound at once, so the timbres must not collide.
    check(
        "carrier frequencies are far apart",
        (ROUTE_BEACON_FREQ_HZ - ROAD_BEEP_FREQ_HZ).abs() > 150.0,
        "",
    );
    // ...and clear of the slots the rest of the mod already occupies.
    for (name, hz) in [
        ("compass/cam FM", 900.0),
        ("scanner/coupler", 1000.0),
        ("slam tick", 660.0),
    ] {
        check(
            &format!("clear of the {name} slot"),
            (ROUTE_BEACON_FREQ_HZ - hz).abs() > 80.0,
            &format!("{ROUTE_BEACON_FREQ_HZ} vs {hz}"),
        );
    }
}

// ==============================================================================
// 5. Audio gating
// ==============================================================================
fn probe_hrtf() -> HrtfSet {
    let l = vec![vec![1.0f32, 0.5]; 360];
    let r = (0..360)
        .map(|az| {
            let g = az as f64 / 360.0;
            vec![g as f32, (g * 0.5) as f32]
        })
        .collect();
    HrtfSet::from_rows(l, r, 48000.0).expect("probe set")
}

#[test]
fn scenario_5_audio_gate() {
    eprintln!("5. audio gating and teardown");
    let h = harness(None);
    let c = &h.ctrl;

    c.set_route_beacon_mode(true);
    c.update_route_beacon(true, 45.0, 250.0);
    {
        let s = c.lock.lock();
        check(
            "an available route arms the renderer",
            s._route_beacon_available,
            "",
        );
        check(
            "bearing and distance are stored",
            s._route_beacon_bearing == 45.0 && s._route_beacon_distance == 250.0,
            "",
        );
    }

    // A cleared route must drop the CONVOLVED BUFFERS, not merely the position: a pulse
    // aimed at where the destination used to be would otherwise be mixed once more.
    let ones: Wave = Arc::from(vec![1.0f32; 8]);
    {
        let mut v = c.voice.lock();
        v._route_beacon_pulse_L = Some(ones.clone());
        v._route_beacon_pulse_R = Some(ones.clone());
        v._route_beacon_playback_pos = 2.0;
    }
    c.update_route_beacon(false, 0.0, 0.0);
    check(
        "clearing the route silences the beacon",
        !c.lock.lock()._route_beacon_available,
        "",
    );
    {
        let v = c.voice.lock();
        check(
            "...and drops the stale aimed pulse",
            v._route_beacon_pulse_L.is_none() && v._route_beacon_playback_pos < 0.0,
            "",
        );
    }

    c.update_route_beacon(true, 10.0, 100.0);
    {
        let mut v = c.voice.lock();
        v._route_beacon_pulse_L = Some(ones.clone());
        v._route_beacon_playback_pos = 1.0;
    }
    c.set_route_beacon_mode(false);
    check(
        "toggling the mode off silences it too",
        !c.lock.lock()._route_beacon_available && c.voice.lock()._route_beacon_pulse_L.is_none(),
        "",
    );

    // Positive is LEFT, in both the HRTF and the stereo fallback.
    {
        let mut s = c.lock.lock();
        s._hrtf = None;
        s._hrtf_user_enabled = false;
    }
    let (left, right) = c.render_directional_pulse(&[1.0f32; 16], 45.0);
    check(
        "stereo fallback puts +45 on the left",
        max_abs(&left) > max_abs(&right),
        "",
    );
    let (left, right) = c.render_directional_pulse(&[1.0f32; 16], -45.0);
    check(
        "...and -45 on the right",
        max_abs(&right) > max_abs(&left),
        "",
    );

    {
        let mut s = c.lock.lock();
        s._hrtf = Some(Arc::new(probe_hrtf()));
        s._hrtf_user_enabled = true;
    }
    let (left, right) = c.render_directional_pulse(&[1.0f32; 16], 45.0);
    let looked_up = 360.0 * right[0] as f64 / left[0] as f64;
    check(
        "HRTF path is handed the bearing unmodified",
        (looked_up - 45.0).abs() < 1e-3,
        &format!("{looked_up}"),
    );
}
