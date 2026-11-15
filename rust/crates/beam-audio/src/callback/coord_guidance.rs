//! Coordinate guidance FM tone, HRTF (audio.py:6869-6968). The frame applies the
//! coordinate duck bus around it. A2.
//!
//! Bearing error maps to HRTF azimuth: 0 = target ahead, positive = left (az 90),
//! negative = right (az 270), +/-180 = behind.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{clamp, clamp01, convolve_f32_into, keep_tail, pan_gains};
use crate::consts::*;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let sr = b.sr;
    if !s.coord_guide_active {
        v._sm_coord_error = 0.0;
        drain_if_nonempty(b, v);
        v._coord_hrtf_overlap_L = None;
        v._coord_hrtf_overlap_R = None;
        return;
    }

    let dt_cg = frames as f64 / sr;
    let beta_cg = 1.0 - (-dt_cg / 0.15).exp(); // 150 ms smoothing
    v._sm_coord_error += beta_cg * (s.coord_guide_error - v._sm_coord_error);
    let abs_err = v._sm_coord_error.abs();

    if !(abs_err > COORD_GUIDE_DEADZONE_DEG) {
        // In the deadzone: drain the HRTF tail.
        if drain_if_nonempty(b, v) {
            v._coord_hrtf_overlap_L = None;
            v._coord_hrtf_overlap_R = None;
        }
        return;
    }

    // error_norm: 0 just outside the deadzone, 1 at 180 degrees off course.
    let error_norm =
        clamp01((abs_err - COORD_GUIDE_DEADZONE_DEG) / (180.0 - COORD_GUIDE_DEADZONE_DEG));
    let amp = 10.0f64.powf(COORD_GUIDE_AMP_DB / 20.0) * error_norm;
    let fc = COORD_GUIDE_FC_OFFCOURSE_HZ
        + (COORD_GUIDE_FC_ONCOURSE_HZ - COORD_GUIDE_FC_OFFCOURSE_HZ) * (1.0 - error_norm);
    let fm_ratio =
        COORD_GUIDE_FM_RATIO_MIN + (COORD_GUIDE_FM_RATIO_MAX - COORD_GUIDE_FM_RATIO_MIN) * error_norm;
    let fm = fc * fm_ratio;
    let mod_index = COORD_GUIDE_MOD_INDEX_MAX * error_norm;

    let mod_inc = fm / sr;
    let car_inc = fc / sr;
    let mut last_mod = 0.0;
    let mut last_car = 0.0;
    let mono_tone: Vec<f32> = (0..frames)
        .map(|k| {
            let n = k as f64;
            let mod_phase = v._coord_phase_mod + n * mod_inc;
            let mod_sig = mod_index * (2.0 * PI * mod_phase).sin();
            let car_phase = v._coord_phase_carrier + n * car_inc;
            last_mod = mod_phase;
            last_car = car_phase;
            (amp * (2.0 * PI * car_phase + mod_sig).sin()) as f32
        })
        .collect();
    if frames > 0 {
        v._coord_phase_carrier = py_mod(last_car + fc / sr, 1.0);
        v._coord_phase_mod = py_mod(last_mod + fm / sr, 1.0);
    }

    let hrtf_az = py_mod(v._sm_coord_error, 360.0);
    let ir = s.hrtf().and_then(|h| h.get_hrir(hrtf_az));
    if let Some((ir_l, ir_r)) = ir {
        let mut scratch = Vec::new();
        let mut conv_l = Vec::new();
        let mut conv_r = Vec::new();
        convolve_f32_into(&mono_tone, &ir_l, &mut scratch, &mut conv_l);
        convolve_f32_into(&mono_tone, &ir_r, &mut scratch, &mut conv_r);
        if let Some(tl) = v._coord_hrtf_overlap_L.as_ref() {
            let ol = tl.len().min(conv_l.len());
            for k in 0..ol {
                conv_l[k] += tl[k];
            }
            if let Some(tr) = v._coord_hrtf_overlap_R.as_ref() {
                for k in 0..ol.min(tr.len()).min(conv_r.len()) {
                    conv_r[k] += tr[k];
                }
            }
        }
        for k in 0..frames.min(conv_l.len()) {
            b.buf_l[k] += conv_l[k] as f32;
            b.buf_r[k] += conv_r[k] as f32;
        }
        keep_tail(&conv_l, frames, &mut v._coord_hrtf_overlap_L);
        keep_tail(&conv_r, frames, &mut v._coord_hrtf_overlap_R);
    } else {
        let pan_val = clamp(-v._sm_coord_error / 180.0, -1.0, 1.0);
        let (lg, rg) = pan_gains(pan_val);
        let (lg, rg) = (lg as f32, rg as f32);
        for k in 0..frames {
            b.buf_l[k] += mono_tone[k] * lg;
            b.buf_r[k] += mono_tone[k] * rg;
        }
        v._coord_hrtf_overlap_L = None;
        v._coord_hrtf_overlap_R = None;
    }
}

/// `if overlap_L is not None and len(overlap_L) > 0: buf[:ol] += overlap[:ol]`; returns
/// whether it drained (the deadzone branch clears the tails only then).
fn drain_if_nonempty(b: &mut Block<'_>, v: &Voice) -> bool {
    let frames = b.frames;
    match v._coord_hrtf_overlap_L.as_ref() {
        Some(tl) if !tl.is_empty() => {
            let ol = tl.len().min(frames);
            for k in 0..ol {
                b.buf_l[k] += tl[k] as f32;
            }
            if let Some(tr) = v._coord_hrtf_overlap_R.as_ref() {
                for k in 0..ol.min(tr.len()) {
                    b.buf_r[k] += tr[k] as f32;
                }
            }
            true
        }
        _ => false,
    }
}
