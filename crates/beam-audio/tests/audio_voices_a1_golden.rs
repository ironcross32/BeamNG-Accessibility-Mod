//! Golden buffers for the A1 callback voices against the real Python callback
//! (`fixtures/gen/audio_voices_a1.py` -> `audio_a1_*.json`): each case replays the
//! same script (telemetry, triggers, clock, HRTF on or off, default and a non-default
//! config) and compares every clipped output sample.
//!
//! Tolerance: cases without HRTF must match bit for bit (every voice does numpy's f32 /
//! f64 arithmetic in the same order, and they do). Cases with HRTF allow 1e-6 absolute
//! (the output is at most 0.999; the worst seen is 2.7e-7): the HRIRs come out of an
//! FFT-based processing chain that AH's tests hold to about 1e-7 relative, and
//! `np.convolve` sums in a different order from the direct convolution here.

mod a1_script;
mod audio_harness;
mod common;

use a1_script::check_fixture;

const TOL: f32 = 1e-6;

#[test]
fn placement_ping_matches_python() {
    check_fixture("audio_a1_ping", TOL);
}

#[test]
fn compass_clicks_match_python() {
    check_fixture("audio_a1_clicks", TOL);
}

#[test]
fn pedals_and_steering_match_python() {
    check_fixture("audio_a1_pedals", TOL);
}

#[test]
fn attitude_matches_python() {
    check_fixture("audio_a1_attitude", TOL);
}

#[test]
fn drift_matches_python() {
    check_fixture("audio_a1_drift", TOL);
}

#[test]
fn node_and_clickspot_beeps_match_python() {
    check_fixture("audio_a1_node", TOL);
}

#[test]
fn lowspeed_clicks_match_python() {
    check_fixture("audio_a1_lowspeed", TOL);
}

#[test]
fn wheel_slip_matches_python() {
    check_fixture("audio_a1_slip", TOL);
}

#[test]
fn heading_guidance_matches_python() {
    check_fixture("audio_a1_heading", TOL);
}

#[test]
fn lineup_matches_python() {
    check_fixture("audio_a1_lineup", TOL);
}

#[test]
fn gear_tones_and_assistant_cues_match_python() {
    check_fixture("audio_a1_gear_assistant", TOL);
}
