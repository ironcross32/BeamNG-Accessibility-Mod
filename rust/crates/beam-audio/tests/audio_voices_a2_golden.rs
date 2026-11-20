//! Golden buffers for the A2 callback voices against the real Python callback
//! (`rust/fixtures/gen/audio_voices_a2.py` -> `audio_a2_*.json`): each case replays the
//! same script (targets, hazards, road states, triggers, clock, HRTF on and off, default
//! and non-default configs) and compares every clipped output sample.
//!
//! Tolerance: 1e-6 absolute (the output is at most 0.999). Measured: every stereo-pan case
//! is bit-exact, every HRTF case within 2.1e-7. Each voice computes in f64 and
//! rounds once into the f32 buffer, or does the same f32 arithmetic numpy does, so the
//! residue is libm `sin`/`exp`/`pow` against numpy's versions (a few f64 ulp) and
//! `np.convolve`'s different summation order for the HRTF voices; both sit far below one
//! f32 step of a full-scale sample (6e-8) and flip a rounding at most. The HRTF cases
//! also inherit the FFT-processed HRIRs, which AH's tests hold to about 1e-6 of the peak.

mod a2_script;
mod audio_harness;
mod common;

use a2_script::check_fixture;

const TOL: f32 = 1e-6;

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

/// The scanner calls the diagnostic recorder once per beep block, with the HRTF flag set
/// when it convolved (_scanner_diag is a no-op unless BEAM_SCANNER_DIAG is set, so the
/// test installs an enabled one).
#[test]
fn scanner_feeds_the_hrtf_diagnostic() {
    use std::sync::Arc;
    let dir = std::env::temp_dir().join(format!("beam_a2_scanner_diag_{}", std::process::id()));
    let path = dir.join("scan.csv");
    let diag = Arc::new(beam_audio::scanner_hrtf_diag::ScannerHrtfDiagnostic::with_enabled(
        Some(path.clone()),
        true,
    ));
    let h = audio_harness::harness(None);
    h.ctrl.voice.lock()._scanner_diag = Some(diag.clone());
    let hrtf = beam_testutil::repo_root().join("hrtf_mit_kemar.npz");
    assert!(h.ctrl.load_hrtf(&hrtf, None, None, None));
    h.ctrl.set_scan_mode(true);
    h.ctrl.update_scanner_target(30.0, 5.0);
    // The first beep starts only once a whole beep interval has elapsed: at 5 m that is
    // about 0.15 s, so 0.6 s of blocks guarantees several beep blocks at any test rate.
    let blocks = (0.6 * h.ctrl.samplerate() / 512.0).ceil() as usize;
    h.run_blocks(blocks, 512);
    diag.flush();
    let text = std::fs::read_to_string(&path).expect("diag csv");
    let rows: Vec<&str> = text.lines().skip(1).filter(|l| !l.is_empty()).collect();
    assert!(!rows.is_empty(), "no beep rows recorded");
    diag.stop();
    let _ = std::fs::remove_dir_all(&dir);
}