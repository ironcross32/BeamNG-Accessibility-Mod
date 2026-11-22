//! Replays a script from `fixtures/gen/audio_voices_a2.py` on the Rust controller:
//! the steps of `audio_callback_harness.run_script` that the A2 fixtures use, plus
//! `{"hrtf": "<file>"}` (load that repository-root HRTF set) and `{"scan": [n, seed]}`
//! (`play_scan` of the generator's deterministic buffer).
#![allow(dead_code)]

use std::sync::Arc;

use beam_audio::controller::{HazardIn, JsonMap, TelemetryState};
use serde_json::Value;

use crate::audio_harness::{Harness, config_from_json, harness};
use crate::common::f4;

fn num(v: &Value) -> f64 {
    v.as_f64()
        .unwrap_or_else(|| panic!("number expected, got {v}"))
}

/// `update_telemetry_state(dict)` for the keys the A2 fixtures send.
pub fn telemetry(map: &JsonMap) -> TelemetryState {
    let mut t = TelemetryState::default();
    for (k, v) in map {
        let b = || v.as_bool().unwrap_or_else(|| panic!("{k}: bool expected"));
        match k.as_str() {
            "last_steering_input" => t.last_steering_input = Some(num(v)),
            "coord_guidance_active" => t.coord_guidance_active = Some(b()),
            "coord_guidance_error_deg" => t.coord_guidance_error_deg = Some(num(v)),
            other => panic!("telemetry key {other} not mapped"),
        }
    }
    t
}

/// `update_selected_hazard(dict or None)`.
pub fn hazard(v: &Value) -> Option<HazardIn> {
    let m = v.as_object()?;
    let f = |k: &str| m.get(k).map(num);
    Some(HazardIn {
        bearing: f("bearing"),
        urgency: f("urgency"),
        gap: f("gap"),
        state: f("state"),
        closing: f("closing"),
        ttc: f("ttc"),
        stopping_margin: f("stopping_margin"),
        legacy: m.get("legacy").and_then(Value::as_bool),
    })
}

/// The generator's `scan_buffer(n, seed)`.
pub fn scan_buffer(n: usize, seed: i64) -> Vec<[f32; 2]> {
    (0..n as i64)
        .map(|k| {
            let l = ((k * 37 + seed).rem_euclid(200) - 100) as f64 / 400.0;
            let r = ((k * 53 + seed * 3).rem_euclid(160) - 80) as f64 / 320.0;
            [l as f32, r as f32]
        })
        .collect()
}

fn call(h: &Harness, name: &str, args: &[Value]) {
    let c = &h.ctrl;
    let f = |i: usize| num(&args[i]);
    let fo = |i: usize, d: f64| args.get(i).map(num).unwrap_or(d);
    let b = |i: usize| args[i].as_bool().expect("bool argument");
    let bo = |i: usize, d: bool| args.get(i).and_then(Value::as_bool).unwrap_or(d);
    let map = |i: usize| args.get(i).and_then(Value::as_object);
    match name {
        "update_telemetry_state" => {
            c.update_telemetry_state(&telemetry(args[0].as_object().expect("state dict")))
        }
        "set_scan_mode" => c.set_scan_mode(b(0)),
        "update_scanner_target" => c.update_scanner_target(f(0), f(1)),
        "update_cannon_aim" => c.update_cannon_aim(f(0), f(1), f(2), b(3), b(4)),
        "set_coupler_tracking" => c.set_coupler_tracking(b(0)),
        "update_coupler_target" => c.update_coupler_target(f(0), f(1), b(2)),
        "set_dock_mode" => c.set_dock_mode(b(0)),
        "update_dock_target" => c.update_dock_target(
            f(0),
            f(1),
            f(2),
            args.get(3).and_then(Value::as_str).unwrap_or("IMPL"),
            bo(4, false),
            fo(5, 0.0),
        ),
        "set_obstacle_mode" => c.set_obstacle_mode(b(0)),
        "update_selected_hazard" => c.update_selected_hazard(hazard(&args[0]).as_ref()),
        "update_obstacle" => c.update_obstacle(args[0].as_i64().expect("type"), f(1), f(2), f(3)),
        "set_road_mode" => c.set_road_mode(b(0)),
        "update_road_guidance" => {
            c.update_road_guidance(
                args[0].as_str().expect("state"),
                map(1),
                map(2),
                bo(3, true),
            );
        }
        "trigger_road_orientation_chime" => {
            c.trigger_road_orientation_chime(f(0), args.get(1).filter(|v| !v.is_null()).map(num))
        }
        "trigger_road_junction_earcon" => c.trigger_road_junction_earcon(),
        "trigger_road_junction_entry_earcon" => c.trigger_road_junction_entry_earcon(),
        "set_route_beacon_mode" => c.set_route_beacon_mode(b(0)),
        "update_route_beacon" => c.update_route_beacon(b(0), fo(1, 0.0), fo(2, 0.0)),
        "update_junction_proximity" => c.update_junction_proximity(map(0), bo(1, true)),
        other => panic!("call {other} not mapped"),
    }
}

/// Build the case's controller and run its script, returning every recorded block.
pub fn run_case(case: &Value) -> (Harness, Vec<(Vec<f32>, Vec<f32>)>) {
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
        } else if let Some(scan) = step.get("scan") {
            let n = scan[0].as_u64().expect("n") as usize;
            let seed = scan[1].as_i64().expect("seed");
            h.ctrl.play_scan(Some(Arc::new(scan_buffer(n, seed))));
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

/// Run every case of a fixture file; assert each matches within `tol` and that the
/// Python recording is not silent (unless the case name ends in `_disabled`, where
/// silence is the point and the Rust must be silent too). Returns the worst differences.
pub fn check_fixture(name: &str, tol: f32) -> Vec<(String, f32)> {
    let fx = beam_testutil::load(name);
    let mut worst = Vec::new();
    for case in fx["cases"].as_array().expect("cases") {
        let cname = case["name"].as_str().expect("name").to_string();
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
        if cname.ends_with("_disabled") {
            assert!(!loud, "{cname}: expected silence");
        } else {
            assert!(loud, "{cname}: the Python recording is silent");
        }
        eprintln!("{name}/{cname}: max |diff| {d:e}");
        worst.push((cname, d));
    }
    worst
}
