//! Drift tone (audio.py:4857-4924): a sine with 7th and 11th harmonics. Everything the
//! listener hears change -- pitch, pan, level -- is ramped across the block rather than
//! stepped at block boundaries, so a fast-updating rate signal still renders smoothly.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{clamp, linspace_open_at};
use crate::consts::{
    DRIFT_AMP_DB, DRIFT_ATTACK_TAU, DRIFT_FREQ_HZ, DRIFT_PAN_TAU, DRIFT_RATE_TAU,
    DRIFT_RELEASE_TAU, py_max,
};
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let sr = b.sr;
    let dt_dr = frames as f64 / sr;
    let alert = s._drift_alert_active;
    let gain_target = if alert { 1.0 } else { 0.0 };
    if !(alert || v._drift_gain > 1e-4) {
        v._drift_gain = 0.0;
        v._sm_drift_rate = s._drift_rate;
        v._sm_drift_pan = s._drift_pan;
        v._drift_inc = 0.0;
        return;
    }
    let tau_g = if gain_target > v._drift_gain {
        DRIFT_ATTACK_TAU
    } else {
        DRIFT_RELEASE_TAU
    };
    let gain_end = v._drift_gain + (1.0 - (-dt_dr / tau_g).exp()) * (gain_target - v._drift_gain);

    // Pitch from the rate: 0.5 (threshold) is 1x, 10 (max severity) 4x.
    let beta_r = 1.0 - (-dt_dr / DRIFT_RATE_TAU).exp();
    v._sm_drift_rate += beta_r * (s._drift_rate - v._sm_drift_rate);
    let rate = py_max(0.5, v._sm_drift_rate);
    let norm = clamp((rate - 0.5) / (10.0 - 0.5), 0.0, 1.0);
    let pitch_mult = 1.0 + 3.0 * norm;

    let inc = (DRIFT_FREQ_HZ * pitch_mult) / sr;
    let inc_prev = if v._drift_inc > 0.0 {
        v._drift_inc
    } else {
        inc
    };

    let amp = 10.0f64.powf(DRIFT_AMP_DB / 20.0);
    let gain0 = v._drift_gain;
    v._drift_gain = gain_end;

    // Pan: -1 left, 1 right, equal power, smoothed so a direction flip crossfades.
    let beta_p = 1.0 - (-dt_dr / DRIFT_PAN_TAU).exp();
    let pan_prev = v._sm_drift_pan;
    v._sm_drift_pan += beta_p * (s._drift_pan - v._sm_drift_pan);
    let pan_end = v._sm_drift_pan;

    let two_pi = 2.0 * PI;
    let (w7, w11) = (2.0 * PI * 7.0, 2.0 * PI * 11.0);
    let quarter_pi = 0.25 * PI;
    // t = (phase + cumsum(linspace(inc_prev, inc, frames, endpoint=False))) % 1.0
    let mut cum = 0.0;
    let mut last_t = 0.0;
    for k in 0..frames {
        cum += linspace_open_at(inc_prev, inc, frames, k);
        let t = py_mod(v._phase_drift + cum, 1.0);
        let mut tone = 1.0 * (two_pi * t).sin() + 0.5 * (w7 * t).sin() + 0.3 * (w11 * t).sin();
        tone *= 0.55;
        tone *= amp;
        tone *= linspace_open_at(gain0, gain_end, frames, k);
        let ang = (linspace_open_at(pan_prev, pan_end, frames, k) + 1.0) * quarter_pi;
        b.buf_l[k] += (tone * ang.cos()) as f32;
        b.buf_r[k] += (tone * ang.sin()) as f32;
        last_t = t;
    }
    if frames > 0 {
        v._phase_drift = last_t;
        v._drift_inc = inc;
    }
}
