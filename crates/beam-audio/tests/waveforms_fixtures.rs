//! beam_audio::waveforms against a real AudioController's `_generate_*` output
//! (`fixtures/gen/audio_waveforms.py`): default config at 48 kHz and a non-default
//! config at 44.1 kHz, plus `_slip_voice_tables`.
//!
//! Sample for sample within 2e-6 absolute (every waveform is at most 1.0 in magnitude,
//! so that is a few float32 ulps). The float64 maths is in numpy's order; what remains
//! is numpy's AVX-512 sin/exp/pow against the C library's, which differ by a float64
//! ulp or so before the float32 cast. The two noise sources (centre click, slide noise)
//! use numpy's own PCG64 stream, so they are held to the same tolerance.
//!
//! One exception, stated: the square-wave sweeps are `sign(sin(phase))`, and a phase an
//! ulp either side of a zero crossing flips a sample's sign. Those two are allowed up to
//! 2 such samples (out of 9600) on top of the tolerance.

mod common;

use beam_audio::waveforms as wf;
use beam_testutil::{f, load};
use common::*;
use serde_json::Value;

fn param(e: &Value, key: &str) -> f64 {
    f(&e["params"][key])
}

fn generate(name: &str, sr: f64, e: &Value) -> Option<Vec<f32>> {
    if let Some(i) = name.strip_prefix("placement_ping_") {
        let i: usize = i.parse().unwrap();
        return Some(wf::generate_placement_pings(sr, param(e, "amp"))[i].clone());
    }
    Some(match name {
        "click" => wf::generate_click(sr, param(e, "amp")),
        "highlight_click" => wf::generate_highlight_click(sr, param(e, "amp")),
        "cam_click" => wf::generate_cam_click(sr, param(e, "amp")),
        "cam_highlight_click" => wf::generate_cam_highlight_click(sr, param(e, "amp")),
        "center_click" => wf::generate_center_click(sr),
        "dock_lock" => wf::generate_dock_lock(sr),
        "slam_clear" | "slam_over" | "slam_committed" => {
            wf::generate_slam_cue(sr, e["params"]["kind"].as_str().unwrap())
        }
        "entry_cue" => wf::generate_entry_cue(sr),
        "dock_approach_cue" => wf::generate_dock_approach_cue(sr),
        "tc_tone" => wf::generate_tc_tone(sr),
        "check_engine_buzzer" => wf::generate_check_engine_buzzer(sr, param(e, "amp")),
        "describe_error_buzz" => wf::generate_describe_error_buzz(sr),
        "oil_chime" => wf::generate_oil_chime(sr, param(e, "amp")),
        "scanner_beep" => wf::generate_scanner_beep(sr),
        "scanner_aligned_beep" => wf::generate_scanner_aligned_beep(sr),
        "obstacle_buzz" => {
            let (w, peak) = wf::generate_obstacle_buzz(sr, param(e, "amp"));
            let want = param(e, "harmonic_peak");
            assert!(
                (peak - want).abs() <= 1e-12 * want,
                "harmonic peak {peak} vs {want}"
            );
            w
        }
        "dropoff_sweep" => wf::generate_dropoff_sweep(sr),
        "hill_sweep" => wf::generate_hill_sweep(sr),
        "road_beep" => return wf::generate_road_beep(sr, param(e, "amp")),
        "route_beacon_pulse" => return wf::generate_route_beacon_pulse(sr, param(e, "amp")),
        "road_chime_fwd" | "road_chime_back" => {
            return wf::generate_road_chime_tone(sr, param(e, "freq"));
        }
        "road_junction_pip" => return wf::generate_road_junction_pip(sr, param(e, "amp")),
        "road_junction_entry" => return wf::generate_road_junction_entry(sr, param(e, "amp")),
        "road_correction_settled" => {
            return wf::generate_road_correction_settled(sr, param(e, "amp"));
        }
        "guidance_tone" => return wf::generate_guidance_tone(sr),
        "lowspeed_click" => wf::generate_lowspeed_click(sr, param(e, "db")),
        "lowspeed_stop_tone" => wf::generate_lowspeed_stop_tone(sr, param(e, "db")),
        "clickspot_beep" => wf::generate_clickspot_beep(sr),
        "clickspot_beep_reverse" => wf::generate_clickspot_beep_reverse(sr),
        "node_beep" => wf::generate_node_beep(sr),
        "node_beep_reverse" => wf::generate_node_beep_reverse(sr),
        other => panic!("no generator for {other}"),
    })
}

fn run(fixture: &str) {
    let fx = load(fixture);
    let sr = f(&fx["sr"]);
    let mut count = 0;
    for e in fx["waves"].as_array().unwrap() {
        let name = e["name"].as_str().unwrap();
        let case = format!("{fixture}/{name}");
        let got = generate(name, sr, e);
        if e.get("none").is_some() {
            assert!(got.is_none(), "{case}: Python returns None");
            continue;
        }
        assert_eq!(e["dtype"].as_str().unwrap(), "<f4", "{case}: dtype");
        let got = got.unwrap_or_else(|| panic!("{case}: Rust returned None"));
        let want = f4(&e["out"]);
        assert_eq!(got.len(), want.len(), "{case}: length");
        let sweep = name.ends_with("_sweep");
        let mut flips = 0;
        let mut worst = 0.0f64;
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            let d = (*g as f64 - *w as f64).abs();
            if d > 2e-6 {
                if sweep && *g == -*w {
                    flips += 1;
                    continue;
                }
                panic!("{case}[{i}]: Rust {g:e}, Python {w:e}");
            }
            worst = worst.max(d);
        }
        assert!(flips <= 2, "{case}: {flips} zero-crossing sign flips");
        eprintln!(
            "{case}: {} samples, worst {worst:.2e}, flips {flips}",
            got.len()
        );
        count += 1;
    }
    assert!(count >= 38, "{fixture}: only {count} waveforms checked");
}

#[test]
fn default_config_at_48k() {
    run("audio_waveforms_default");
}

#[test]
fn custom_config_at_44k1() {
    run("audio_waveforms_custom");
}

#[test]
fn slip_voice_tables_match() {
    let fx = load("audio_slip_tables");
    let sr = f(&fx["sr"]);
    let (spin, lock, lo, hi) = wf::slip_voice_tables(sr);
    assert_close_f32("spin table", &spin, &f4(&fx["spin"]), 2e-6);
    assert_close_f32("lock table", &lock, &f4(&fx["lock"]), 2e-6);
    let n = fx["noise_len"].as_u64().unwrap() as usize;
    assert_eq!(lo.len(), n);
    assert_eq!(hi.len(), n);
    let lo_s: Vec<f32> = lo.iter().step_by(11).copied().collect();
    let hi_s: Vec<f32> = hi.iter().step_by(11).copied().collect();
    assert_close_f32("noise lo", &lo_s, &f4(&fx["noise_lo_sampled"]), 2e-6);
    assert_close_f32("noise hi", &hi_s, &f4(&fx["noise_hi_sampled"]), 2e-6);
    for (name, t, key) in [("lo", &lo, "noise_lo_rms"), ("hi", &hi, "noise_hi_rms")] {
        let w: Vec<f64> = t.iter().map(|&v| v as f64).collect();
        let r = beam_audio::dsp::rms(&w);
        assert!((r - f(&fx[key])).abs() < 1e-7, "noise {name} rms {r}");
    }
}
