//! Free-camera line-up guide FM tone (audio.py:6974-6977) via `_render_lineup`
//! (3311-3399), HRTF-placed. Not ducked.
//!
//! Two things keep it free of zipper noise, both learned from the driver's seat: amplitude
//! and index are RAMPED across the block from last block's value rather than stepped, and
//! when the azimuth moves the block is convolved with BOTH the previous and the new HRIR
//! and crossfaded, instead of swapping the filter at the block edge.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;
use beam_reports::camera_lineup::CARRIER_HZ;

use crate::callback::block::Block;
use crate::callback::common::{
    add_conv_f32, add_tail, clamp, convolve_f32_into, convolve_full_into, drain_tails, keep_tail,
    pan_gains,
};
use crate::consts::{LINEUP_AMP_DB, LINEUP_AZ_SMOOTH_S, LINEUP_SMOOTH_S};
use crate::controller::Voice;
use crate::hrtf::HrtfSet;

/// 6977: `self._render_lineup(bufL, bufR, frames, lineup_live, lineup_target)`.
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let hrtf = b.snap.hrtf();
    render_lineup(
        v,
        hrtf,
        b.sr,
        b.buf_l,
        b.buf_r,
        b.frames,
        b.snap.lineup_live,
        b.snap.lineup_target,
    );
}

/// `_render_lineup(bufL, bufR, frames, live, target)`. `hrtf` is `self._hrtf` only when
/// `_hrtf_user_enabled`.
#[allow(clippy::too_many_arguments)]
pub fn render_lineup(
    v: &mut Voice,
    hrtf: Option<&HrtfSet>,
    sr: f64,
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    frames: usize,
    live: bool,
    target: (f64, f64, f64, f64),
) {
    let (t_amp, mut t_index, mut t_ratio, t_az) = if live { target } else { (0.0, 1.0, 1.0, 0.0) };
    let mut t_az = Some(t_az);
    if !live {
        t_index = v._sm_lineup_index;
        t_ratio = v._sm_lineup_ratio;
        t_az = None;
    }
    let dt = frames as f64 / sr;
    let beta = 1.0 - (-dt / LINEUP_SMOOTH_S).exp();
    let beta_az = 1.0 - (-dt / LINEUP_AZ_SMOOTH_S).exp();

    let (amp0, idx0) = (v._sm_lineup_amp, v._sm_lineup_index);
    v._sm_lineup_amp += beta * (t_amp - v._sm_lineup_amp);
    v._sm_lineup_index += beta * (t_index - v._sm_lineup_index);
    v._sm_lineup_ratio += beta * (t_ratio - v._sm_lineup_ratio);
    if let Some(t_az) = t_az {
        // Smoothed as a unit vector so a swing through behind takes the short way.
        let (tx, ty) = (t_az.to_radians().cos(), t_az.to_radians().sin());
        let (sx, sy) = v._sm_lineup_az_xy;
        v._sm_lineup_az_xy = (sx + beta_az * (tx - sx), sy + beta_az * (ty - sy));
    }

    if amp0 < 1e-4 && v._sm_lineup_amp < 1e-4 {
        v._sm_lineup_amp = 0.0;
        if v._lineup_overlap_L.as_ref().is_some_and(|t| !t.is_empty()) {
            drain_tails(
                buf_l,
                buf_r,
                &mut v._lineup_overlap_L,
                &mut v._lineup_overlap_R,
                frames,
            );
        }
        v._lineup_overlap_L = None;
        v._lineup_overlap_R = None;
        v._lineup_ir = None;
        return;
    }

    let peak = 10.0f64.powf(LINEUP_AMP_DB / 20.0);
    let denom = frames.max(1) as f64;
    let ramp: Vec<f64> = (0..frames).map(|k| (k as f64 + 1.0) / denom).collect();
    let amp_end = v._sm_lineup_amp;
    let idx_end = v._sm_lineup_index;

    let fc = CARRIER_HZ;
    let fm = fc * v._sm_lineup_ratio;
    let (fm_inc, fc_inc) = (fm / sr, fc / sr);
    let two_pi = 2.0 * PI;
    let mut mono: Vec<f32> = Vec::with_capacity(frames);
    let (mut car_last, mut mod_last) = (0.0, 0.0);
    for (k, &r) in ramp.iter().enumerate() {
        let amp = peak * (amp0 + (amp_end - amp0) * r);
        let index = idx0 + (idx_end - idx0) * r;
        let n = k as f64;
        let mod_phase = v._lineup_phase_mod + n * fm_inc;
        let car_phase = v._lineup_phase_carrier + n * fc_inc;
        mono.push((amp * (two_pi * car_phase + index * (two_pi * mod_phase).sin()).sin()) as f32);
        car_last = car_phase;
        mod_last = mod_phase;
    }
    if frames > 0 {
        v._lineup_phase_carrier = py_mod(car_last + fc / sr, 1.0);
        v._lineup_phase_mod = py_mod(mod_last + fm / sr, 1.0);
    }

    let (sx, sy) = v._sm_lineup_az_xy;
    let az = py_mod(sy.atan2(sx).to_degrees(), 360.0);
    let irs = hrtf.and_then(|h| h.get_hrir(az));
    let Some((ir_l, ir_r)) = irs else {
        // Stereo fallback: sin(az) is +1 hard left, matching the HRTF's 90 = left.
        let (lg, rg) = pan_gains(clamp(-az.to_radians().sin(), -1.0, 1.0));
        let (lg, rg) = (lg as f32, rg as f32);
        for (k, &m) in mono.iter().enumerate() {
            buf_l[k] += m * lg;
            buf_r[k] += m * rg;
        }
        v._lineup_overlap_L = None;
        v._lineup_overlap_R = None;
        v._lineup_ir = None;
        return;
    };

    let mut scratch = Vec::new();
    let mut conv_l = Vec::new();
    let mut conv_r = Vec::new();
    convolve_f32_into(&mono, &ir_l, &mut scratch, &mut conv_l);
    convolve_full_into(&scratch, &ir_r, &mut conv_r);
    if let Some((prev_l, prev_r)) = &v._lineup_ir {
        if prev_l.len() == ir_l.len() {
            // Crossfade the block from the old filter to the new one. The tail (beyond
            // `frames`) belongs to the new filter alone; the ramp has reached 1 by then.
            let mut old = Vec::new();
            convolve_full_into(&scratch, prev_l, &mut old);
            for k in 0..frames.min(old.len()) {
                let w = ramp[k];
                conv_l[k] = old[k] * (1.0 - w) + conv_l[k] * w;
            }
            convolve_full_into(&scratch, prev_r, &mut old);
            for k in 0..frames.min(old.len()) {
                let w = ramp[k];
                conv_r[k] = old[k] * (1.0 - w) + conv_r[k] * w;
            }
        }
    }
    v._lineup_ir = Some((ir_l, ir_r));
    add_tail(&mut conv_l, &v._lineup_overlap_L);
    add_tail(&mut conv_r, &v._lineup_overlap_R);
    add_conv_f32(buf_l, &conv_l, frames);
    add_conv_f32(buf_r, &conv_r, frames);
    keep_tail(&conv_l, frames, &mut v._lineup_overlap_L);
    keep_tail(&conv_r, frames, &mut v._lineup_overlap_R);
}
