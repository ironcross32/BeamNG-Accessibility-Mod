//! The callback frame against the real Python callback
//! (`rust/fixtures/gen/audio_callback_harness.py` -> `audio_callback_frame.json`).

mod audio_harness;

use audio_harness::{config_from_json, fixture_blocks, harness, max_abs_diff};
use beam_audio::controller::TelemetryState;

/// Float32 one-shots copied sample for sample; the shift tone is computed in float64 and
/// rounded once, so both should agree to the last bit or within a rounding step.
const TOL: f32 = 1e-6;

fn case<'a>(fx: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    fx["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("fixture case {name}"))
}

#[test]
fn silence_in_silence_out() {
    let fx = beam_testutil::load("audio_callback_frame");
    let c = case(&fx, "silence");
    let h = harness(config_from_json(&c["config"]).as_ref());
    let got = h.run_blocks(4, 480);
    for (i, (l, r)) in got.iter().enumerate() {
        assert!(
            l.iter().chain(r.iter()).all(|&x| x == 0.0),
            "silence: block {i} not silent"
        );
    }
    let want = fixture_blocks(c);
    assert_eq!(max_abs_diff(&got, &want, "silence"), 0.0);
}

#[test]
fn check_engine_buzzer_matches_python() {
    let fx = beam_testutil::load("audio_callback_frame");
    let c = case(&fx, "check_engine_buzzer");
    let h = harness(config_from_json(&c["config"]).as_ref());
    h.ctrl.trigger_check_engine_buzzer();
    let got = h.run_blocks(3, 480);
    let want = fixture_blocks(c);
    let d = max_abs_diff(&got, &want, "check_engine_buzzer");
    assert!(d <= TOL, "check_engine_buzzer: max diff {d}");
    assert!(
        got[0].0.iter().any(|&x| x != 0.0),
        "check_engine_buzzer: silent"
    );
}

#[test]
fn shift_tone_matches_python() {
    let fx = beam_testutil::load("audio_callback_frame");
    let c = case(&fx, "shift_tone");
    let h = harness(config_from_json(&c["config"]).as_ref());
    h.ctrl.update_telemetry_state(&TelemetryState {
        shift_active: Some(true),
        ..TelemetryState::default()
    });
    let got = h.run_blocks(2, 512);
    let want = fixture_blocks(c);
    let d = max_abs_diff(&got, &want, "shift_tone");
    assert!(d <= TOL, "shift_tone: max diff {d}");
}

#[test]
fn clip_and_never_panic_on_zero_frames() {
    let h = harness(None);
    let (l, r) = h.ctrl.render_block(0);
    assert!(l.is_empty() && r.is_empty());
}
