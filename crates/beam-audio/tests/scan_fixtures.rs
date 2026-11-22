//! beam_audio::scan against audio.py's terrain scan block
//! (`fixtures/gen/audio_scan.py`).
//!
//! Exact: pan gains, scale steps, pitches, the jitter stream, buffer lengths and the
//! raised errors. Within tolerance: envelopes (1e-12; numpy's SIMD cos/pow differ from
//! the C library by an ulp or so) and every float32 grain and rendered buffer (2e-6 of
//! full scale: grains are peak-normalised to 1.0, so that is a few float32 ulps; the
//! inputs to the cast differ only by float64 ulps from SIMD sin/exp).

mod common;

use beam_audio::consts::*;
use beam_audio::scan::*;
use beam_testutil::{f, load, same_f64};
use common::*;
use serde_json::Value;

#[test]
fn pan_step_and_pitch_are_exact() {
    let fx = load("audio_scan");
    for c in fx["pan"].as_array().unwrap() {
        let (l, r) = equal_power_pan(f(&c["p"]));
        assert!(
            same_f64(l, f(&c["out"][0])) && same_f64(r, f(&c["out"][1])),
            "pan {c}"
        );
    }
    for c in fx["steps"].as_array().unwrap() {
        let dz = f(&c["dz"]);
        assert_eq!(
            scan_step_from_dz(dz),
            c["step"].as_i64().unwrap(),
            "step {dz}"
        );
        assert!(
            same_f64(scan_pitch_hz(dz, 0.0), f(&c["pitch"])),
            "pitch {dz}"
        );
        assert!(
            same_f64(scan_pitch_hz(dz, SCAN_OBJECT_OCTAVE), f(&c["pitch_obj"])),
            "object pitch {dz}"
        );
    }
}

#[test]
fn envelopes_match() {
    let fx = load("audio_scan");
    for c in fx["envelopes"].as_array().unwrap() {
        let n = c["n"].as_u64().unwrap() as usize;
        let sr = f(&c["sr"]);
        let (case, got) = if c["kind"] == "ref" {
            (
                format!("ref env n={n}"),
                scan_envelope(n, sr, SCAN_REF_RELEASE_MS),
            )
        } else {
            let m = f(&c["mult"]);
            (
                format!("grain env n={n} mult={m}"),
                scan_grain_envelope(n, sr, m),
            )
        };
        assert_close_f64(&case, &got, &f8(&c["out"]), 1e-12);
    }
}

#[test]
fn grains_match() {
    let fx = load("audio_scan");
    for c in fx["grains"].as_array().unwrap() {
        let sr = f(&c["sr"]);
        let kind = c["fn"].as_str().unwrap();
        let hz = c.get("hz").map(f).unwrap_or(0.0);
        let got = match kind {
            "terrain" => scan_terrain_grain(hz, sr, c["family"].as_str().unwrap()),
            "water" => scan_water_grain(hz, sr),
            "object" => scan_object_ping(hz, sr, c["kind"].as_str().unwrap()),
            "poi" => scan_poi_doublet(hz, sr),
            "ref" => scan_reference_ping(sr),
            other => panic!("unknown grain {other}"),
        };
        let case = format!(
            "{kind} {:?} {hz} Hz at {sr}",
            c.get("family").or(c.get("kind"))
        );
        let w = assert_close_f32(&case, &got, &f4(&c["out"]), 2e-6);
        eprintln!("{case}: worst {w:.2e}");
    }
}

fn samples_of(v: &Value) -> Vec<ScanSample> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| ScanSample {
            bearing_deg: f(&s[0]),
            range_m: f(&s[1]),
            dz_m: opt_f(&s[2]),
            depth_m: opt_f(&s[3]),
            family: opt_s(s.get(4)),
        })
        .collect()
}

fn objects_of(v: &Value) -> Vec<ScanObject> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|o| ScanObject {
            bearing_deg: f(&o[0]),
            range_m: f(&o[1]),
            dz_m: f(&o[2]),
            kind: opt_s(o.get(3)),
        })
        .collect()
}

fn pois_of(v: &Value) -> Vec<ScanPoi> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| ScanPoi {
            bearing_deg: f(&p[0]),
            range_m: f(&p[1]),
            dz_m: f(&p[2]),
        })
        .collect()
}

#[test]
fn rendered_scans_match() {
    let fx = load("audio_scan");
    for c in fx["renders"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let buf = render_scan(
            &samples_of(&c["samples"]),
            &objects_of(&c["objects"]),
            f(&c["reach"]),
            f(&c["sr"]),
            f(&c["level_db"]),
            &pois_of(&c["pois"]),
        )
        .unwrap();
        let out = &c["out"];
        assert_eq!(
            buf.len() as u64,
            out["frames"].as_u64().unwrap(),
            "{name}: frames"
        );
        let sampled: Vec<f32> = buf
            .iter()
            .step_by(67)
            .flat_map(|fr| [fr[0], fr[1]])
            .collect();
        let w = assert_close_f32(
            &format!("{name} sampled"),
            &sampled,
            &f4(&out["sampled"]),
            2e-6,
        );
        let peak = buf
            .iter()
            .flat_map(|fr| [fr[0], fr[1]])
            .fold(0.0f64, |m, v| m.max((v as f64).abs()));
        assert!(
            (peak - f(&out["peak"])).abs() <= 2e-6,
            "{name}: peak {peak}"
        );
        for (ch, key, abs) in [
            (0, "sum_l", false),
            (1, "sum_r", false),
            (0, "abs_l", true),
            (1, "abs_r", true),
        ] {
            let v: Vec<f64> = buf
                .iter()
                .map(|fr| {
                    let x = fr[ch] as f64;
                    if abs { x.abs() } else { x }
                })
                .collect();
            let s = beam_audio::dsp::sum(&v);
            let want = f(&out[key]);
            assert!(
                (s - want).abs() <= 2e-6 * buf.len() as f64 * 1e-2 + 1e-9,
                "{name}: {key} {s} vs {want}"
            );
        }
        eprintln!("{name}: worst sampled difference {w:.2e}");
    }
}

#[test]
fn render_scan_raises_where_python_raises() {
    let fx = load("audio_scan");
    for c in fx["errors"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let nan = f64::NAN;
        let s = |range: f64, dz: f64| ScanSample {
            bearing_deg: 0.0,
            range_m: range,
            dz_m: Some(dz),
            depth_m: None,
            family: None,
        };
        let res = match name {
            "nan_range" => render_scan(&[s(nan, 0.0)], &[], 200.0, 48000.0, SCAN_TERRAIN_DB, &[]),
            "neg_inf_range" => render_scan(
                &[s(f64::NEG_INFINITY, 0.0)],
                &[],
                200.0,
                48000.0,
                SCAN_TERRAIN_DB,
                &[],
            ),
            "nan_dz" => render_scan(&[s(10.0, nan)], &[], 200.0, 48000.0, SCAN_TERRAIN_DB, &[]),
            "nan_object_dz" => render_scan(
                &[],
                &[ScanObject {
                    bearing_deg: 0.0,
                    range_m: 10.0,
                    dz_m: nan,
                    kind: None,
                }],
                200.0,
                48000.0,
                SCAN_TERRAIN_DB,
                &[],
            ),
            other => panic!("unknown case {other}"),
        };
        match (res, c["msg"].as_str()) {
            (Err(e), Some(msg)) => assert_eq!(e.0, msg, "{name}"),
            (Ok(_), None) => {}
            (r, m) => panic!("{name}: Rust {:?}, Python {m:?}", r.err()),
        }
    }
}
