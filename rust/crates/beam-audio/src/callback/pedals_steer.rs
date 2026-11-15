//! Pedal tones and the steering square (audio.py:4637-4690): three slightly detuned
//! sines (clutch left, brake centre, throttle right) whose pitch and level follow the
//! smoothed pedal, and a hard square whose pitch follows the steering input.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{clamp, pan_gains};
use crate::consts::{
    PEDAL_BASE_FREQ, PEDAL_MAX_MULT, PEDAL_SMOOTH_MS, STEER_AMP_DB, STEER_BASE_FREQ,
    STEER_MAX_FREQ, py_max,
};
use crate::controller::{AudioController, Voice};

/// `np.sign`: -1, 0 or 1 (NaN stays NaN).
#[inline]
fn np_sign(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        x
    }
}

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    if !s.pt_on {
        return;
    }
    let frames = b.frames;
    let sr = b.sr;
    let (tau, dt) = (PEDAL_SMOOTH_MS / 1000.0, frames as f64 / sr);
    let beta = 1.0 - (-dt / py_max(1e-6, tau)).exp();
    v._sm_c += beta * (s.v_clutch - v._sm_c);
    v._sm_b += beta * (s.v_brake - v._sm_b);
    v._sm_t += beta * (s.v_throt - v._sm_t);

    let two_pi = 2.0 * PI;
    let voices = [
        (pan_gains(-1.0 / 3.0), v._sm_c, v._detune_c, 0usize),
        (pan_gains(0.0), v._sm_b, v._detune_b, 1),
        (pan_gains(1.0 / 3.0), v._sm_t, v._detune_t, 2),
    ];
    for ((gl, gr), sm, detune, which) in voices {
        let f = (PEDAL_BASE_FREQ + detune) * (1.0 + (PEDAL_MAX_MULT - 1.0) * sm);
        let inc = f / sr;
        let phase = match which {
            0 => v._phase_c,
            1 => v._phase_b,
            _ => v._phase_t,
        };
        let a = AudioController::amp_from_val(sm);
        let mut last = 0.0;
        for k in 0..frames {
            let t = py_mod(k as f64 * inc + phase, 1.0);
            let x = (two_pi * t).sin();
            let base = a * x;
            b.buf_l[k] += (base * gl) as f32;
            b.buf_r[k] += (base * gr) as f32;
            last = t;
        }
        if frames > 0 {
            let p = py_mod(last + inc, 1.0);
            match which {
                0 => v._phase_c = p,
                1 => v._phase_b = p,
                _ => v._phase_t = p,
            }
        }
    }

    // Steering tone (square wave). v_steer is the normalised steering input, -1..1.
    let s_clamped = clamp(s.v_steer, -1.0, 1.0);
    if s_clamped.abs() > 0.0005 {
        let s_norm = s_clamped.abs();
        let freq = STEER_BASE_FREQ + (STEER_MAX_FREQ - STEER_BASE_FREQ) * s_norm;
        let inc = freq / sr;
        let amp = 10.0f64.powf(STEER_AMP_DB / 20.0);
        // Negative input sounds in the left ear only, positive in the right. (The Python
        // comment above these lines describes the opposite; this is what it plays.)
        let pan_left = if s_clamped < 0.0 { 1.0 } else { 0.0 };
        let pan_right = if s_clamped > 0.0 { 1.0 } else { 0.0 };
        let mut last = 0.0;
        for k in 0..frames {
            let t = py_mod(k as f64 * inc + v._phase_steer, 1.0);
            let sq = np_sign((two_pi * t).sin());
            let base = amp * sq;
            b.buf_l[k] += (base * pan_left) as f32;
            b.buf_r[k] += (base * pan_right) as f32;
            last = t;
        }
        if frames > 0 {
            v._phase_steer = py_mod(last + inc, 1.0);
        }
    }
}
