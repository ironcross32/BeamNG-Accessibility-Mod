//! Replays a script from `rust/fixtures/gen/audio_voices_a1.py` on the Rust controller:
//! the steps of `audio_callback_harness.run_script` that the A1 fixtures use, plus
//! `{"hrtf": "<file>"}` (load that repository-root HRTF set).
#![allow(dead_code)]

use beam_audio::controller::TelemetryState;
use serde_json::Value;

use crate::audio_harness::{Harness, config_from_json, harness};
use crate::common::f4;

fn num(v: &Value) -> f64 {
    v.as_f64()
        .unwrap_or_else(|| panic!("number expected, got {v}"))
}

fn arg(args: &[Value], i: usize) -> Option<&Value> {
    args.get(i)
}

/// `update_telemetry_state(dict)`.
pub fn telemetry(map: &serde_json::Map<String, Value>) -> TelemetryState {
    let mut t = TelemetryState::default();
    for (k, v) in map {
        let b = || v.as_bool().unwrap_or_else(|| panic!("{k}: bool expected"));
        let f = || num(v);
        match k.as_str() {
            "shift_active" => t.shift_active = Some(b()),
            "pedal_tones_active" => t.pedal_tones_active = Some(b()),
            "tc_active" => t.tc_active = Some(b()),
            "last_clutch" => t.last_clutch = Some(f()),
            "last_brake" => t.last_brake = Some(f()),
            "last_throttle" => t.last_throttle = Some(f()),
            "last_steering" => t.last_steering = Some(f()),
            "inverted" => t.inverted = Some(b()),
            "last_roll_rad" => t.last_roll_rad = Some(f()),
            "last_pitch_rad" => t.last_pitch_rad = Some(f()),
            "guidance_active" => t.guidance_active = Some(b()),
            "guidance_error_deg" => t.guidance_error_deg = Some(f()),
            "last_actual_steering" => t.last_actual_steering = Some(f()),
            "last_steering_input" => t.last_steering_input = Some(f()),
            "drift_alert_active" => t.drift_alert_active = Some(b()),
            "drift_pan" => t.drift_pan = Some(f()),
            "drift_rate" => t.drift_rate = Some(f()),
            "ls_clicks_active" => t.ls_clicks_active = Some(b()),
            "ls_speed_mph" => t.ls_speed_mph = Some(f()),
            "ls_accel" => t.ls_accel = Some(f()),
            "ls_stopped" => t.ls_stopped = Some(b()),
            "slip_active" => t.slip_active = Some(b()),
            "slip_kind" => t.slip_kind = Some(v.as_i64().expect("slip_kind int")),
            "slip_mag" => t.slip_mag = Some(f()),
            "slip_ratio" => t.slip_ratio = Some(f()),
            "slide_active" => t.slide_active = Some(b()),
            "slide_deg" => t.slide_deg = Some(f()),
            "slip_azimuth_deg" => t.slip_azimuth_deg = Some(f()),
            other => panic!("telemetry key {other} not mapped"),
        }
    }
    t
}

fn call(h: &Harness, name: &str, args: &[Value]) {
    let c = &h.ctrl;
    let f = |i: usize| num(&args[i]);
    let fo = |i: usize, d: f64| arg(args, i).map(num).unwrap_or(d);
    let bo = |i: usize, d: bool| arg(args, i).and_then(Value::as_bool).unwrap_or(d);
    match name {
        "update_telemetry_state" => {
            c.update_telemetry_state(&telemetry(args[0].as_object().expect("state dict")))
        }
        "trigger_placement_ping" => {
            c.trigger_placement_ping(args[0].as_i64().expect("rung"), f(1), fo(2, 1.0))
        }
        "trigger_compass_click" => c.trigger_compass_click(f(0), f(1)),
        "trigger_compass_highlight" => c.trigger_compass_highlight(f(0), f(1)),
        "trigger_cam_compass_click" => c.trigger_cam_compass_click(f(0), f(1)),
        "trigger_cam_compass_highlight" => c.trigger_cam_compass_highlight(f(0), f(1)),
        "trigger_node_hover_beep" => c.trigger_node_hover_beep(f(0), bo(1, false)),
        "trigger_clickspot_beep" => c.trigger_clickspot_beep(bo(0, false)),
        "trigger_lowspeed_stopped" => c.trigger_lowspeed_stopped(),
        "set_lineup" => c.set_lineup(
            args[0].as_bool().expect("active"),
            fo(1, 0.0),
            fo(2, 1.0),
            fo(3, 1.0),
            fo(4, 0.0),
        ),
        "trigger_gear_tone" => c.trigger_gear_tone(args[0].as_i64().expect("degree"), bo(1, false)),
        "trigger_assistant_cue" => {
            let choices: Vec<&str> = arg(args, 1)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|x| x.as_str().expect("choice")).collect())
                .unwrap_or_default();
            c.trigger_assistant_cue(args[0].as_str().expect("kind"), &choices)
        }
        other => panic!("call {other} not mapped"),
    }
}

/// Build the case's controller and run its script, returning every recorded block.
pub type Blocks = Vec<(Vec<f32>, Vec<f32>)>;

pub fn run_case(case: &Value) -> (Harness, Blocks) {
    let h = harness(config_from_json(&case["config"]).as_ref());
    let mut out = Vec::new();
    for step in case["script"].as_array().expect("script") {
        if let Some(file) = step.get("hrtf") {
            let path = beam_testutil::repo_root().join(file.as_str().expect("hrtf file"));
            assert!(
                h.ctrl.load_hrtf(&path, None, None, None),
                "HRTF {} failed to load",
                path.display()
            );
        } else if let Some(name) = step.get("call") {
            let empty = Vec::new();
            let args = step.get("args").and_then(Value::as_array).unwrap_or(&empty);
            call(&h, name.as_str().expect("call name"), args);
        } else if let Some(t) = step.get("clock") {
            h.clock.set(num(t));
        } else if let Some(dt) = step.get("advance") {
            h.clock.advance(num(dt));
        } else if let Some(n) = step.get("blocks") {
            let frames = step["frames"].as_u64().expect("frames") as usize;
            out.extend(h.run_blocks(n.as_u64().expect("n") as usize, frames));
        } else {
            panic!("unknown step {step}");
        }
    }
    (h, out)
}

/// The fixture's recorded blocks (base64 `<f4`).
pub fn recorded(case: &Value) -> Vec<(Vec<f32>, Vec<f32>)> {
    case["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .map(|b| (f4(&b["left"]), f4(&b["right"])))
        .collect()
}

/// Run every case of a fixture file; assert each matches (exactly, or within `hrtf_tol`
/// for a case that loads an HRTF set) and is not silent. Returns `(case, worst
/// difference)` for the report.
pub fn check_fixture(name: &str, hrtf_tol: f32) -> Vec<(String, f32)> {
    let fx = beam_testutil::load(name);
    let mut worst = Vec::new();
    for case in fx["cases"].as_array().expect("cases") {
        let cname = case["name"].as_str().expect("name").to_string();
        let uses_hrtf = case["script"]
            .as_array()
            .is_some_and(|s| s.iter().any(|st| st.get("hrtf").is_some()));
        let tol = if uses_hrtf { hrtf_tol } else { 0.0 };
        let (_h, got) = run_case(case);
        let want = recorded(case);
        assert_eq!(got.len(), want.len(), "{cname}: block count");
        let mut d = 0.0f32;
        for (i, ((gl, gr), (wl, wr))) in got.iter().zip(&want).enumerate() {
            assert_eq!(gl.len(), wl.len(), "{cname}: block {i} length");
            for (k, (g, w)) in gl.iter().zip(wl).chain(gr.iter().zip(wr)).enumerate() {
                let e = (g - w).abs();
                assert!(
                    e <= tol,
                    "{cname}: block {i} sample {k}: Rust {g:e}, Python {w:e} (tolerance {tol:e})"
                );
                d = d.max(e);
            }
        }
        let loud = want
            .iter()
            .any(|(l, r)| l.iter().chain(r).any(|&x| x != 0.0));
        let disabled = cname.ends_with("_disabled");
        assert!(loud || disabled, "{cname}: the Python recording is silent");
        eprintln!("{name}/{cname}: max |diff| {d:e}");
        worst.push((cname, d));
    }
    worst
}
