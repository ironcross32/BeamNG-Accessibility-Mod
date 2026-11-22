//! Golden buffers for the machine-instrument voices (articulation, implement, docking, dock
//! earcons) and their couplings, against the real Python callback
//! (`fixtures/gen/audio_callback_a3.py`). Each case's script is replayed through the
//! Rust controller and every recorded block compared sample for sample.

mod audio_harness;

use audio_harness::{Harness, config_from_json, harness};
use beam_audio::controller::{HazardIn, TelemetryState};
use serde_json::Value;

/// Phases are accumulated in the same order as numpy's sequential `cumsum` and every cast
/// to float32 happens where Python's does, so the panned paths are bit-identical. The HRTF
/// paths differ only by the convolution's summation order (direct sum here, numpy's own
/// order there), measured at <= 1.4e-7 (about two float32 ulps at the output's level);
/// 1e-6 leaves that seven-fold margin and is still far below audibility.
const TOL: f32 = 1e-6;

/// `(dock lock chimes started, centre clicks started)` per case, so the scripts are known
/// to reach the latches they were written for.
fn expected_starts(name: &str) -> (usize, usize) {
    match name {
        "hydro_pan_sweep" | "hydro_hrtf_sweep" => (0, 2),
        "hydro_trailer" => (0, 1),
        "dock_impl_pan" | "dock_impl_hrtf" | "dock_ramp_pan" => (2, 0),
        "coupling" => (1, 0),
        _ => (0, 0),
    }
}

type Blocks = Vec<(Vec<f32>, Vec<f32>)>;

fn hex_f32(s: &str) -> Vec<f32> {
    let b = s.as_bytes();
    let nib = |c: u8| -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("bad hex"),
        }
    };
    let bytes: Vec<u8> = b.chunks(2).map(|p| (nib(p[0]) << 4) | nib(p[1])).collect();
    bytes
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn telemetry(map: &serde_json::Map<String, Value>) -> TelemetryState {
    let mut t = TelemetryState::default();
    for (k, v) in map {
        match k.as_str() {
            "last_actual_steering" => t.last_actual_steering = v.as_f64(),
            "last_steering_input" => t.last_steering_input = v.as_f64(),
            "trailer_artic" => t.trailer_artic = v.as_f64(),
            "trailer_reverse" => t.trailer_reverse = v.as_bool(),
            "implement_flags" => t.implement_flags = v.as_i64(),
            "implement_min_clearance" => t.implement_min_clearance = v.as_f64(),
            "implement_tilt_deg" => t.implement_tilt_deg = v.as_f64(),
            "implement_lift" => t.implement_lift = v.as_f64(),
            "implement_activity" => t.implement_activity = v.as_f64(),
            other => panic!("telemetry key {other} not mapped"),
        }
    }
    t
}

fn call(h: &Harness, name: &str, args: &[Value]) {
    let c = &h.ctrl;
    match name {
        "update_dock_target" => c.update_dock_target(
            args[0].as_f64().unwrap(),
            args[1].as_f64().unwrap(),
            args[2].as_f64().unwrap(),
            args[3].as_str().unwrap(),
            args[4].as_bool().unwrap(),
            args[5].as_f64().unwrap(),
        ),
        "set_dock_mode" => c.set_dock_mode(args[0].as_bool().unwrap()),
        "clear_dock_target" => c.clear_dock_target(),
        "trigger_slam_cue" => c.trigger_slam_cue(args[0].as_str().unwrap()),
        "trigger_entry_cue" => c.trigger_entry_cue(),
        "trigger_dock_approach_cue" => c.trigger_dock_approach_cue(),
        "update_selected_hazard" => match args[0].as_object() {
            None => c.update_selected_hazard(None),
            Some(m) => {
                let mut hz = HazardIn::default();
                for (k, v) in m {
                    match k.as_str() {
                        "state" => hz.state = v.as_f64(),
                        "bearing" => hz.bearing = v.as_f64(),
                        other => panic!("hazard key {other} not mapped"),
                    }
                }
                c.update_selected_hazard(Some(&hz));
            }
        },
        other => panic!("call {other} not mapped"),
    }
}

/// Replay one fixture case's script; return the recorded blocks.
fn replay(case: &Value) -> (Blocks, (usize, usize)) {
    let h = harness(config_from_json(&case["config"]).as_ref());
    let mut out = Vec::new();
    // (dock lock chimes started, centre clicks started)
    let mut starts = (0usize, 0usize);
    for step in case["script"].as_array().expect("script") {
        if let Some(m) = step.get("telemetry") {
            h.ctrl
                .update_telemetry_state(&telemetry(m.as_object().expect("telemetry")));
        } else if let Some(name) = step.get("call") {
            let args = step
                .get("args")
                .and_then(|a| a.as_array().cloned())
                .unwrap_or_default();
            call(&h, name.as_str().unwrap(), &args);
        } else if let Some(file) = step.get("hrtf") {
            let path = beam_testutil::repo_root().join(file.as_str().unwrap());
            assert!(
                h.ctrl.load_hrtf(&path, None, None, None),
                "HRTF set failed to load"
            );
        } else if let Some(dt) = step.get("advance") {
            h.clock.advance(dt.as_f64().unwrap());
        } else if let Some(n) = step.get("blocks") {
            let frames = step["frames"].as_u64().unwrap() as usize;
            let tick = step.get("tick").and_then(|t| t.as_bool()).unwrap_or(false);
            let record = step.get("record").and_then(|t| t.as_bool()).unwrap_or(true);
            for _ in 0..n.as_u64().unwrap() {
                let blk = h.ctrl.render_block(frames);
                {
                    let v = h.ctrl.voice.lock();
                    if v._dock_lock_pos == frames as f64 {
                        starts.0 += 1;
                    }
                    if v._hydro_center_pos == frames as f64 {
                        starts.1 += 1;
                    }
                }
                if tick {
                    h.clock.advance(frames as f64 / h.ctrl.samplerate());
                }
                if record {
                    out.push(blk);
                }
            }
        } else {
            panic!("unknown step {step}");
        }
    }
    (out, starts)
}

fn want(case: &Value) -> Blocks {
    case["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .map(|b| {
            (
                hex_f32(b["left"].as_str().unwrap()),
                hex_f32(b["right"].as_str().unwrap()),
            )
        })
        .collect()
}

/// Compare, naming the case, block and sample of the worst difference. Returns the max
/// difference and the peak level (so a silent voice cannot pass by matching silence).
fn compare(name: &str, got: &Blocks, want: &Blocks) -> (f32, f32) {
    assert_eq!(got.len(), want.len(), "{name}: recorded block count");
    let mut worst = (0.0f32, 0usize, 0usize);
    let mut peak = 0.0f32;
    for (i, ((gl, gr), (wl, wr))) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(gl.len(), wl.len(), "{name}: block {i} length");
        for (k, (x, y)) in gl
            .iter()
            .zip(wl.iter())
            .chain(gr.iter().zip(wr.iter()))
            .enumerate()
        {
            let d = (x - y).abs();
            if !(d <= worst.0) {
                worst = (d, i, k);
            }
            peak = peak.max(y.abs());
        }
    }
    eprintln!(
        "{name}: max diff {:e} (block {}, sample {}), peak {peak}",
        worst.0, worst.1, worst.2
    );
    assert!(
        worst.0 <= TOL,
        "{name}: max diff {:e} at block {} sample {} (tolerance {TOL:e})",
        worst.0,
        worst.1,
        worst.2
    );
    (worst.0, peak)
}

fn run_fixture(fixture: &str) {
    let fx = beam_testutil::load(fixture);
    for case in fx["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let (got, starts) = replay(case);
        eprintln!(
            "{name}: lock chimes {}, centre clicks {}",
            starts.0, starts.1
        );
        assert_eq!(
            starts,
            expected_starts(name),
            "{name}: (lock chimes, centre clicks)"
        );
        let (_, peak) = compare(name, &got, &want(case));
        assert!(peak > 1e-3, "{name}: the reference itself is silent");
    }
}

#[test]
fn hydro_steer_matches_python() {
    run_fixture("audio_a3_hydro");
}

#[test]
fn implement_matches_python() {
    run_fixture("audio_a3_implement");
}

#[test]
fn dock_implement_mode_matches_python() {
    run_fixture("audio_a3_dock");
}

#[test]
fn dock_ramp_mode_and_earcons_match_python() {
    run_fixture("audio_a3_dock_ramp");
}

#[test]
fn couplings_and_dock_duck_bus_match_python() {
    run_fixture("audio_a3_coupling");
}
