//! Golden buffers for the A2 callback voices against the real Python callback
//! (`rust/fixtures/gen/audio_voices_a2.py` -> `audio_a2_*.json`): each case replays the
//! same script (targets, hazards, road states, triggers, clock, HRTF on and off, default
//! and non-default configs) and compares every clipped output sample.
//!
//! Tolerance: 2e-6 absolute (the output is at most 0.999). Each voice computes in f64 and
//! rounds once into the f32 buffer, or does the same f32 arithmetic numpy does, so the
//! residue is libm `sin`/`exp`/`pow` against numpy's versions (a few f64 ulp) and
//! `np.convolve`'s different summation order for the HRTF voices; both sit far below one
//! f32 step of a full-scale sample (6e-8) and flip a rounding at most. The HRTF cases
//! also inherit the FFT-processed HRIRs, which AH's tests hold to about 1e-6 of the peak.

mod a2_script;
mod audio_harness;
mod common;

use a2_script::check_fixture;

const TOL: f32 = 2e-6;

#[test]
fn scanner_matches_python() {
    check_fixture("audio_a2_scanner", TOL);
}

#[test]
fn scanner_dock_gate_and_coupler_suppression_match_python() {
    check_fixture("audio_a2_scanner_coupling", TOL);
}

#[test]
fn cannon_aim_matches_python() {
    check_fixture("audio_a2_cannon", TOL);
}

#[test]
fn coupler_matches_python() {
    check_fixture("audio_a2_coupler", TOL);
}

#[test]
fn obstacle_pan_matches_python() {
    check_fixture("audio_a2_obstacle_pan", TOL);
}

#[test]
fn obstacle_hrtf_matches_python() {
    check_fixture("audio_a2_obstacle_hrtf", TOL);
}

#[test]
fn obstacle_duck_buses_match_python() {
    check_fixture("audio_a2_obstacle_ducks", TOL);
}

#[test]
fn road_pan_matches_python() {
    check_fixture("audio_a2_road_pan", TOL);
}

#[test]
fn road_levels_match_python() {
    check_fixture("audio_a2_road_levels", TOL);
}

#[test]
fn road_hrtf_matches_python() {
    check_fixture("audio_a2_road_hrtf", TOL);
}

#[test]
fn coordinate_guidance_matches_python() {
    check_fixture("audio_a2_coord", TOL);
}

#[test]
fn scan_playback_crossfade_matches_python() {
    check_fixture("audio_a2_scan_playback", TOL);
}

#[test]
fn junction_stop_bar_matches_python() {
    check_fixture("audio_a2_junction", TOL);
}
