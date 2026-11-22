//! Helpers shared by beam-audio's fixture tests: base64 arrays and tolerant comparison.
#![allow(dead_code)]

use serde_json::Value;

pub fn b64(s: &str) -> Vec<u8> {
    let mut map = [255u8; 256];
    for (i, c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        map[*c as usize] = i as u8;
    }
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | map[c as usize] as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// A base64 `<f8` array.
pub fn f8(v: &Value) -> Vec<f64> {
    b64(v.as_str().expect("base64 string"))
        .chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// A base64 `<f4` array.
pub fn f4(v: &Value) -> Vec<f32> {
    b64(v.as_str().expect("base64 string"))
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// Assert `got` matches `want` within `tol` absolute; returns the worst difference.
pub fn assert_close_f64(case: &str, got: &[f64], want: &[f64], tol: f64) -> f64 {
    assert_eq!(got.len(), want.len(), "{case}: length");
    let mut worst = 0.0f64;
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let d = (g - w).abs();
        if !(d <= tol) && !(g.is_nan() && w.is_nan()) {
            panic!("{case}[{i}]: got {g:e}, Python {w:e} (tolerance {tol:e})");
        }
        worst = worst.max(d);
    }
    worst
}

pub fn assert_close_f32(case: &str, got: &[f32], want: &[f32], tol: f64) -> f64 {
    let g: Vec<f64> = got.iter().map(|&v| v as f64).collect();
    let w: Vec<f64> = want.iter().map(|&v| v as f64).collect();
    assert_close_f64(case, &g, &w, tol)
}

/// Python float or null.
pub fn opt_f(v: &Value) -> Option<f64> {
    if v.is_null() {
        None
    } else {
        Some(beam_testutil::f(v))
    }
}

/// Python str or null.
pub fn opt_s(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}
