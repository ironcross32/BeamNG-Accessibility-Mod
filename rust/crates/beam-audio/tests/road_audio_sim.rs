//! `diagnostic/road_audio_sim.py`: road cue state, panning, rate and queue checks, in
//! the same order.
//!
//! Python's `FakeHRTF` returns the IR pair `[1.0, 0.5]` / `[0.25, 0.125]` for every
//! bearing and records the bearings it is asked for. Here it is an `HrtfSet` built from
//! those rows at every whole degree; the recorded bearing is recovered from a second
//! "probe" set whose right-ear gain at each degree is `degree / 360`, so the L/R ratio of
//! what comes out names the azimuth that was looked up.

mod audio_harness;

use std::sync::Arc;

use audio_harness::{config_from_json, harness};
use beam_audio::AudioController;
use beam_audio::consts::*;
use beam_audio::controller::{JsonMap, RoadGuidanceResult};
use beam_audio::hrtf::HrtfSet;
use serde_json::{Value, json};

fn fake_hrtf() -> HrtfSet {
    let l = vec![vec![1.0f32, 0.5]; 360];
    let r = vec![vec![0.25f32, 0.125]; 360];
    HrtfSet::from_rows(l, r, 48000.0).expect("fake set")
}

/// One-tap IRs: left 1.0, right `az / 360`.
fn probe_hrtf() -> HrtfSet {
    let l = vec![vec![1.0f32]; 360];
    let r = (0..360)
        .map(|az| vec![(az as f64 / 360.0) as f32])
        .collect();
    HrtfSet::from_rows(l, r, 48000.0).expect("probe set")
}

/// The azimuth the probe set was asked for, from a block it rendered.
fn probed_bearing(left: &[f32], right: &[f32]) -> f64 {
    let peak = |x: &[f32]| x.iter().fold(0.0f64, |m, &v| m.max((v as f64).abs()));
    360.0 * peak(right) / peak(left)
}

fn max_abs(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, &v| m.max(v.abs()))
}

fn obj(v: Value) -> JsonMap {
    v.as_object().expect("object").clone()
}

fn set_hrtf(c: &AudioController, set: Option<HrtfSet>, enabled: bool) {
    let mut s = c.lock.lock();
    s._hrtf = set.map(Arc::new);
    s._hrtf_user_enabled = enabled;
}

#[test]
fn road_audio_simulation() {
    let cfg = config_from_json(&json!({
        "road_beep_volume_db": -14.0,
        "road_correction_volume_db": -24.0,
        "road_junction_volume_db": -14.0,
        "hrtf_enabled": false,
    }));
    let h = harness(cfg.as_ref());
    let controller = &h.ctrl;
    assert!(
        controller.is_enabled(),
        "audio dependencies are unavailable"
    );
    controller.set_road_mode(true);
    {
        let s = controller.lock.lock();
        assert!(s._road_correction_amp < s._road_amp);
    }

    // Aligned driving is the dead zone: no correction and no pending acquisition pulse.
    let idle = obj(json!({"active": false, "bearing": 0, "severity": 0}));
    controller.update_road_guidance("onRoad", None, Some(&idle), true);
    assert!(!controller.lock.lock()._road_correction_active);
    assert!(controller.voice.lock()._road_playback_pos < 0.0);

    controller.update_road_guidance("offRoad", None, None, true);
    assert!(!controller.lock.lock()._road_beacon_available);
    controller.update_road_guidance("onRoad", None, Some(&idle), true);

    assert!(road_beacon_rate_hz(2.0) > road_beacon_rate_hz(100.0));

    assert_eq!(road_correction_rate_hz(0.0), ROAD_CORRECTION_RATE_MIN_HZ);
    assert_eq!(road_correction_rate_hz(1.0), ROAD_CORRECTION_RATE_MAX_HZ);

    // Positive bearings are left. Stereo fallback and HRTF both preserve that contract.
    set_hrtf(controller, None, false);
    let (left, right) = controller.render_directional_pulse(&[1.0f32; 16], 45.0);
    assert!(max_abs(&left) > max_abs(&right));

    set_hrtf(controller, Some(fake_hrtf()), true);
    let (left, right) = controller.render_directional_pulse(&[1.0f32; 16], 45.0);
    assert!(left.len() == 17 && right.len() == 17);
    assert!(max_abs(&left) > max_abs(&right));

    // Correction is a bounded directional pip train. Demand raises its cadence.
    set_hrtf(controller, None, false);
    let corr = obj(json!({"active": true, "bearing": 90, "severity": 0.25, "phase": "correct"}));
    controller.update_road_guidance("onRoad", None, Some(&corr), true);
    let correction = controller
        .render_road_correction_block(4096, true, 90.0, 0.25, "correct")
        .expect("correction block");
    assert!(max_abs(&correction.0) > max_abs(&correction.1));
    assert!(correction.0.iter().filter(|&&x| x.abs() < 1e-8).count() > 0);
    assert_eq!(
        controller.voice.lock()._road_correction_render_bearing,
        35.0
    );
    assert!(controller.voice.lock()._road_playback_pos < 0.0);

    // The unwind instruction is a directional doublet and begins immediately when the
    // protocol phase changes.
    let unwind_pkt =
        obj(json!({"active": true, "bearing": -20, "severity": 0.4, "phase": "unwind"}));
    controller.update_road_guidance("onRoad", None, Some(&unwind_pkt), true);
    assert_eq!(controller.lock.lock()._road_correction_phase, "unwind");
    let sr = controller.samplerate();
    let unwind = controller
        .render_road_correction_block((sr * 0.25).trunc() as usize, true, -20.0, 0.4, "unwind")
        .expect("unwind block");
    assert!(max_abs(&unwind.1) > max_abs(&unwind.0));
    assert_eq!(
        controller.voice.lock()._road_correction_render_bearing,
        -25.0
    );

    for phase in ["correct", "unwind"] {
        for bearing in [-0.1, 0.0, 0.1] {
            controller.stop_road_correction_voice();
            let (left, right) = controller
                .render_road_correction_block(4096, true, bearing, 0.25, phase)
                .expect("block");
            if bearing == 0.0 {
                // np.allclose(left, right): rtol 1e-5, atol 1e-8.
                assert!(
                    left.iter()
                        .zip(&right)
                        .all(|(a, b)| (a - b).abs() <= 1e-8 + 1e-5 * b.abs())
                );
            } else {
                assert_eq!(max_abs(&left) > max_abs(&right), bearing > 0.0);
            }
        }
    }

    // HRTF lookup never leaves the requested 35-degree frontal arc. The sim's own sequence
    // runs on the fake set, checking the azimuth the voice looks up (`bearing % 360`)...
    set_hrtf(controller, Some(fake_hrtf()), true);
    let lookups = [
        (-90.0, 1.0, "correct", 325.0),
        (-0.1, 0.25, "unwind", 335.0),
        (0.1, 0.25, "unwind", 25.0),
        (0.0, 0.25, "unwind", 0.0),
    ];
    for (bearing, severity, phase, expected) in lookups {
        controller.render_road_correction_block(512, true, bearing, severity, phase);
        let az = beam_base::pyfmt::py_mod(
            controller.voice.lock()._road_correction_render_bearing,
            360.0,
        );
        assert_eq!(az, expected, "{bearing}: lookup azimuth");
    }
    // ...and, on a second controller, the probe set confirms that azimuth is what reaches
    // the HRTF. The voice is stopped first so each block starts on an audible pip (the sim
    // records the lookup whether or not the block is audible; the probe needs sound).
    {
        let p = harness(cfg.as_ref());
        set_hrtf(&p.ctrl, Some(probe_hrtf()), true);
        for (bearing, severity, phase, expected) in lookups {
            p.ctrl.stop_road_correction_voice();
            let (l, r) = p
                .ctrl
                .render_road_correction_block(512, true, bearing, severity, phase)
                .expect("block");
            // One-tap IRs leave no overlap tail, so the block is exactly this lookup.
            let got = probed_bearing(&l, &r);
            assert!(
                (got - expected).abs() < 1e-3,
                "{bearing}: looked up {got}, want {expected}"
            );
        }
    }

    // Alignment releases to silence; leaving the road cuts the voice and tail now.
    let mut released = controller.render_road_correction_block(512, false, 0.0, 0.0, "correct");
    assert!(released.is_some());
    for _ in 0..100 {
        released = controller.render_road_correction_block(512, false, 0.0, 0.0, "correct");
        if released.is_none() {
            break;
        }
    }
    assert!(released.is_none() && controller.voice.lock()._road_correction_env == 0.0);
    controller.render_road_correction_block(512, true, 10.0, 0.5, "correct");
    let off = obj(json!({"bearing": 20, "distance": 8}));
    controller.update_road_guidance("offRoad", Some(&off), None, true);
    assert_eq!(controller.voice.lock()._road_correction_env, 0.0);
    assert!(controller.voice.lock()._road_correction_overlap_L.is_none());

    // The fake clock (both readings) stands for the sim's patched `time.monotonic`.
    h.clock.set(100.0);
    controller.lock.lock()._road_chime_next_allowed_time = 0.0;
    controller.trigger_road_orientation_chime(12.0, None);
    assert_eq!(controller.lock.lock()._road_chime_queue.len(), 1); // one-way legal direction only
    h.clock.advance(2.0);
    controller.trigger_road_orientation_chime(12.0, Some(-168.0));
    assert_eq!(controller.lock.lock()._road_chime_queue.len(), 2);

    controller.trigger_road_junction_earcon();
    let first_delays: Vec<i64> = controller
        .lock
        .lock()
        ._road_junction_queue
        .iter()
        .map(|e| e.delay)
        .collect();
    assert_eq!(first_delays.len(), 2);
    controller.trigger_road_junction_earcon();
    let delays: Vec<i64> = controller
        .lock
        .lock()
        ._road_junction_queue
        .iter()
        .map(|e| e.delay)
        .collect();
    assert_eq!(delays, first_delays);
    controller.trigger_road_junction_entry_earcon();
    let waves = controller.waveforms();
    let entry = waves
        .ROAD_JUNCTION_ENTRY_WAVEFORM
        .clone()
        .expect("entry waveform");
    let pip = waves
        .ROAD_JUNCTION_WAVEFORM
        .clone()
        .expect("junction waveform");
    {
        let s = controller.lock.lock();
        assert_eq!(s._road_junction_queue.len(), 1);
        assert!(Arc::ptr_eq(&s._road_junction_queue[0].L, &entry));
    }
    assert!(entry.len() != pip.len() || entry.iter().zip(pip.iter()).any(|(a, b)| a != b));

    h.clock.set(100.0);
    let corr = obj(json!({"active": true, "bearing": -35, "severity": 0.7, "phase": "correct"}));
    controller.update_road_guidance("onRoad", None, Some(&corr), true);
    assert!(controller.lock.lock()._road_correction_active);
    assert_eq!(controller.lock.lock()._road_correction_bearing, -35.0);
    let settled_packet = obj(json!({
        "active": false, "bearing": 0, "severity": 0, "phase": "idle", "settled": true,
    }));
    let settled_event =
        controller.update_road_guidance("onRoad", None, Some(&settled_packet), true);
    assert_eq!(controller.voice.lock()._road_correction_settled_pos, 0);
    assert_eq!(
        settled_event,
        RoadGuidanceResult {
            settled_triggered: true
        }
    );
    let repeated_event =
        controller.update_road_guidance("onRoad", None, Some(&settled_packet), true);
    assert_eq!(
        repeated_event,
        RoadGuidanceResult {
            settled_triggered: false
        }
    );
    h.clock.advance(ROAD_CORRECTION_SETTLED_REFRACTORY_S + 0.01);
    assert_eq!(
        controller.update_road_guidance("onRoad", None, Some(&settled_packet), true),
        RoadGuidanceResult {
            settled_triggered: true
        }
    );
    controller.clear_road_audio();
    {
        let s = controller.lock.lock();
        assert!(!s._road_correction_active);
        assert!(s._road_chime_queue.is_empty());
        assert!(s._road_junction_queue.is_empty());
    }
    eprintln!("road_audio_sim: all diagnostics passed");
}
