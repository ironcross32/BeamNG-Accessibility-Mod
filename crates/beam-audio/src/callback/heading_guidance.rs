//! Heading guidance tone (audio.py:5968-6032): fundamental plus 3rd and 5th harmonics,
//! silent inside the deadzone, cubic swell to full scale, panned toward the error
//! (positive right). The frame applies the heading duck bus around it.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{clamp, pan_gains};
use crate::consts::{
    GUIDANCE_DEADZONE_DEG, GUIDANCE_FREQ_HZ, GUIDANCE_FULL_SCALE_DEG, GUIDANCE_MAX_DBFS,
    GUIDANCE_MIN_DBFS, py_max, py_min,
};
use crate::controller::Voice;

// `!(x > y)` keeps Python's NaN behaviour (NaN takes the quiet branch).
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    if !s.guide_active {
        v._sm_guidance_error = 0.0; // reset when off
        return;
    }
    let frames = b.frames;
    // Smooth the error (100 ms) to avoid zippers.
    let (tau, dt) = (0.1, frames as f64 / b.sr);
    let beta = 1.0 - (-dt / py_max(1e-6, tau)).exp();
    v._sm_guidance_error += beta * (s.guide_error - v._sm_guidance_error);

    let abs_err = v._sm_guidance_error.abs();
    if !(abs_err > GUIDANCE_DEADZONE_DEG) {
        return;
    }
    // Error from [DEADZONE, FULL_SCALE] to [MIN_DB, MAX_DB] on a cubic, clamped above.
    let excess_err = py_min(abs_err, GUIDANCE_FULL_SCALE_DEG) - GUIDANCE_DEADZONE_DEG;
    let range_deg = GUIDANCE_FULL_SCALE_DEG - GUIDANCE_DEADZONE_DEG;
    let norm_linear = py_max(0.0, excess_err / range_deg);
    let norm_amp = norm_linear.powf(3.0);
    let db = GUIDANCE_MIN_DBFS + (GUIDANCE_MAX_DBFS - GUIDANCE_MIN_DBFS) * norm_amp;
    let amp = 10.0f64.powf(db / 20.0);

    let inc = GUIDANCE_FREQ_HZ / b.sr;
    let pan_val = clamp(v._sm_guidance_error / GUIDANCE_FULL_SCALE_DEG, -1.0, 1.0);
    let (lg, rg) = pan_gains(pan_val);
    let two_pi = 2.0 * PI;
    let (w3, w5) = (2.0 * PI * 3.0, 2.0 * PI * 5.0);
    let mut last = 0.0;
    for k in 0..frames {
        let t = py_mod(k as f64 * inc + v._phase_guidance, 1.0);
        let mut tone = 1.0 * (two_pi * t).sin() + 0.33 * (w3 * t).sin() + 0.2 * (w5 * t).sin();
        tone *= 0.7;
        tone *= amp;
        b.buf_l[k] += (tone * lg) as f32;
        b.buf_r[k] += (tone * rg) as f32;
        last = t;
    }
    if frames > 0 {
        v._phase_guidance = py_mod(last + inc, 1.0);
    }
}
