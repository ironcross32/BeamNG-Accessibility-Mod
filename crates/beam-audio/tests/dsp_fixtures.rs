//! beam_audio::dsp and the rate helpers in beam_audio::consts against numpy/audio.py
//! (`fixtures/gen/audio_dsp.py`).
//!
//! Exact (bitwise): linspace, rfftfreq, sum, mean, rms, cumsum, searchsorted, every
//! default_rng stream, and the rate helpers. Within tolerance: convolve (numpy's dot may
//! be reordered by its BLAS) and the FFTs (pocketfft and rustfft round differently; both
//! are O(eps log n) accurate, so 1e-12 of the signal scale is far above either's error).

use beam_audio::consts::*;
use beam_audio::dsp::*;
use beam_testutil::{f, load, same_f64};
use serde_json::Value;

fn arr(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|b| f64::from_bits(b.as_u64().unwrap()))
        .collect()
}

fn carr(v: &Value) -> Vec<Complex64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            Complex64::new(
                f64::from_bits(p[0].as_u64().unwrap()),
                f64::from_bits(p[1].as_u64().unwrap()),
            )
        })
        .collect()
}

fn assert_exact(case: &str, got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "{case}: length");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(same_f64(*g, *w), "{case}[{i}]: got {g:e}, numpy {w:e}");
    }
}

fn assert_close(case: &str, got: &[f64], want: &[f64], rel: f64) {
    assert_eq!(got.len(), want.len(), "{case}: length");
    let scale = want.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-300);
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() <= rel * scale,
            "{case}[{i}]: got {g:e}, numpy {w:e} (scale {scale:e})"
        );
    }
}

#[test]
fn linspace_matches_numpy() {
    let fx = load("audio_dsp");
    for (k, c) in fx["linspace"].as_array().unwrap().iter().enumerate() {
        let got = linspace(
            f(&c["start"]),
            f(&c["stop"]),
            c["num"].as_u64().unwrap() as usize,
            c["endpoint"].as_bool().unwrap(),
        );
        assert_exact(&format!("linspace case {k}"), &got, &arr(&c["out"]));
    }
}

#[test]
fn rfftfreq_matches_numpy() {
    let fx = load("audio_dsp");
    for c in fx["rfftfreq"].as_array().unwrap() {
        let n = c["n"].as_u64().unwrap() as usize;
        assert_exact(
            &format!("rfftfreq n={n}"),
            &rfftfreq(n, f(&c["d"])),
            &arr(&c["out"]),
        );
    }
}

#[test]
fn reductions_match_numpy_bit_for_bit() {
    let fx = load("audio_dsp");
    for c in fx["sums"].as_array().unwrap() {
        let x = arr(&c["x"]);
        let n = x.len();
        assert!(same_f64(sum(&x), f(&c["sum"])), "sum n={n}");
        if n > 0 {
            assert!(same_f64(mean(&x), f(&c["mean"])), "mean n={n}");
            assert!(same_f64(rms(&x), f(&c["rms"])), "rms n={n}");
        }
        if !c["cumsum"].is_null() {
            assert_exact(&format!("cumsum n={n}"), &cumsum(&x), &arr(&c["cumsum"]));
        }
    }
}

#[test]
fn convolve_full_matches_numpy() {
    let fx = load("audio_dsp");
    for c in fx["convolve"].as_array().unwrap() {
        let a = arr(&c["a"]);
        let v = arr(&c["v"]);
        let case = format!("convolve {}x{}", a.len(), v.len());
        assert_close(&case, &convolve_full(&a, &v), &arr(&c["full"]), 1e-13);
        let a32: Vec<f32> = a.iter().map(|&x| x as f32).collect();
        let v32: Vec<f32> = v.iter().map(|&x| x as f32).collect();
        let got: Vec<f64> = convolve_full_f32(&a32, &v32)
            .iter()
            .map(|&x| x as f64)
            .collect();
        assert_close(&format!("{case} f32"), &got, &arr(&c["full_f32"]), 1e-5);
    }
}

#[test]
fn searchsorted_matches_numpy() {
    let fx = load("audio_dsp");
    let a = arr(&fx["searchsorted"]["a"]);
    for c in fx["searchsorted"]["cases"].as_array().unwrap() {
        let v = f(&c["v"]);
        assert_eq!(
            searchsorted_left(&a, v) as u64,
            c["left"].as_u64().unwrap(),
            "left {v}"
        );
        assert_eq!(
            searchsorted_right(&a, v) as u64,
            c["right"].as_u64().unwrap(),
            "right {v}"
        );
    }
}

#[test]
fn rfft_and_irfft_match_numpy() {
    let fx = load("audio_dsp");
    for c in fx["rfft"].as_array().unwrap() {
        let x = arr(&c["x"]);
        let n = c["n"].as_u64().unwrap() as usize;
        let got = rfft(&x, n);
        let want = carr(&c["rfft"]);
        let case = format!("rfft len {} n {n}", x.len());
        let g: Vec<f64> = got.iter().flat_map(|z| [z.re, z.im]).collect();
        let w: Vec<f64> = want.iter().flat_map(|z| [z.re, z.im]).collect();
        assert_close(&case, &g, &w, 1e-12);
    }
    for c in fx["irfft"].as_array().unwrap() {
        let spec = carr(&c["spec"]);
        let n = c["n"].as_u64().unwrap() as usize;
        let case = format!("irfft bins {} n {n}", spec.len());
        assert_close(&case, &irfft(&spec, n), &arr(&c["irfft"]), 1e-12);
    }
}

#[test]
fn default_rng_streams_match_numpy_exactly() {
    let fx = load("audio_dsp");
    let r = &fx["rng"];

    let mut g = default_rng(7);
    for (i, w) in r["raw_seed7"].as_array().unwrap().iter().enumerate() {
        assert_eq!(g.next_u64(), w.as_u64().unwrap(), "raw seed 7 [{i}]");
    }
    let mut g = default_rng((1u128 << 70) + 99);
    for (i, w) in r["raw_seed_2_70_plus_99"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(g.next_u64(), w.as_u64().unwrap(), "raw big seed [{i}]");
    }

    let mut g = default_rng(SCAN_RNG_SEED as u128);
    for (i, w) in r["scan_jitter_1200"].as_array().unwrap().iter().enumerate() {
        assert_eq!(g.integers(-1200, 1201), w.as_i64().unwrap(), "jitter [{i}]");
    }
    let mut g = default_rng(SCAN_RNG_SEED as u128);
    for (i, w) in r["scan_jitter_1"].as_array().unwrap().iter().enumerate() {
        assert_eq!(g.integers(-1, 2), w.as_i64().unwrap(), "jitter 1 [{i}]");
    }

    let mut g = default_rng(0xC0FFEE);
    let want = arr(&r["uniform_c0ffee"]);
    assert_exact(
        "uniform c0ffee",
        &g.uniform_n(0.0, 2.0 * std::f64::consts::PI, want.len()),
        &want,
    );

    let mut g = default_rng(0x511DE);
    let want = arr(&r["normal_511de_head"]);
    assert_exact("normal head", &g.standard_normal_n(want.len()), &want);

    let mut g = default_rng(0x511DE);
    let big = g.standard_normal_n(r["normal_511de_big_len"].as_u64().unwrap() as usize);
    let sampled: Vec<f64> = big.iter().step_by(4999).copied().collect();
    assert_exact(
        "normal sampled",
        &sampled,
        &arr(&r["normal_511de_big_every_997"]),
    );
    assert!(
        same_f64(sum(&big), f(&r["normal_511de_big_sum"])),
        "normal big sum"
    );

    let m = r["mixed_seed0"].as_array().unwrap();
    let mut g = default_rng(0);
    assert_eq!(g.integers(0, 10), m[0].as_i64().unwrap());
    assert!(same_f64(g.uniform(-3.0, 3.0), f(&m[1])));
    assert_eq!(g.integers(-5, 6), m[2].as_i64().unwrap());
    assert_eq!(g.integers(0, 1 << 40), m[3].as_i64().unwrap());
    assert!(same_f64(g.standard_normal(), f(&m[4])));
}

#[test]
fn rate_helpers_match_audio_py() {
    let fx = load("audio_dsp");
    for c in fx["rates"].as_array().unwrap() {
        let x = f(&c["x"]);
        assert!(same_f64(road_beacon_rate_hz(x), f(&c["road"])), "road {x}");
        assert!(
            same_f64(route_beacon_rate_hz(x), f(&c["route"])),
            "route {x}"
        );
        assert!(
            same_f64(road_correction_rate_hz(x), f(&c["correction"])),
            "correction {x}"
        );
    }
    for c in fx["obstacle"].as_array().unwrap() {
        let s = c["state"].as_i64().unwrap();
        let u = f(&c["urgency"]);
        assert!(
            same_f64(obstacle_cue_rate_hz(s, u), f(&c["rate"])),
            "obstacle rate {s} {u}"
        );
        assert!(same_f64(obstacle_duck_gain(s), f(&c["duck"])), "duck {s}");
    }
    for c in fx["stop_bar"].as_array().unwrap() {
        let st = stop_bar_state(
            f(&c["d"]),
            f(&c["speed"]),
            c["stopped"].as_bool(),
            c["on_bar"].as_bool().unwrap(),
        );
        let case = format!("stop_bar {c}");
        assert_eq!(st.name(), c["name"].as_str().unwrap(), "{case}");
        match (st.period(), c["period"].is_null()) {
            (None, true) => {}
            (Some(p), false) => assert!(same_f64(p, f(&c["period"])), "{case}"),
            _ => panic!("{case}: period presence differs"),
        }
    }
}
