//! Roll pulse and pitch triangle (audio.py:4692-4855). Each tone is quiet while the
//! attitude holds steady and swells while it changes (the stability fade). With HRTF, the
//! roll pulse sits toward the side the vehicle leans to, the pitch tone in front (nose
//! down) or behind (nose up), both farther away the steeper the angle; overlap-add tails
//! carry across blocks and drain when a tone stops.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{
    Tail, add_conv_f32, add_tail, clamp, clamp01, convolve_f32_into, convolve_full_into,
    drain_tails, keep_tail, pan_gains,
};
use crate::consts::{
    ATT_BASE_PITCH_HZ, ATT_BASE_ROLL_HZ, ATT_FADE_ATTACK_MS, ATT_FADE_DECAY_MS, ATT_FADE_MIN_GAIN,
    ATT_FADE_SENSITIVITY, ATT_HRTF_FAR_DB, ATT_SMOOTH_MS, ATT_START_DEG, ATT_STOP_DEG,
    PITCH_CAP_DEG, ROLL_CAP_DEG, py_max,
};
use crate::controller::{AudioController, Voice};
use crate::hrtf::HrtfSet;

/// Add an `f32` mono tone at `ir` (HRTF) with a distance gain, overlap-adding the tails.
#[allow(clippy::too_many_arguments)]
fn convolve_placed(
    buf_l: &mut [f32],
    buf_r: &mut [f32],
    frames: usize,
    mono: &[f32],
    ir_l: &[f64],
    ir_r: &[f64],
    dist_gain: f64,
    tail_l: &mut Tail,
    tail_r: &mut Tail,
) {
    let mut scratch = Vec::new();
    let mut conv_l = Vec::new();
    let mut conv_r = Vec::new();
    convolve_f32_into(mono, ir_l, &mut scratch, &mut conv_l);
    convolve_full_into(&scratch, ir_r, &mut conv_r);
    for x in conv_l.iter_mut() {
        *x *= dist_gain;
    }
    for x in conv_r.iter_mut() {
        *x *= dist_gain;
    }
    // The two tails always have the same length, so sizing each by its own length is
    // Python's `ol = min(len(tail_L), len(conv_l))` for both.
    add_tail(&mut conv_l, tail_l);
    add_tail(&mut conv_r, tail_r);
    add_conv_f32(buf_l, &conv_l, frames);
    add_conv_f32(buf_r, &conv_r, frames);
    keep_tail(&conv_l, frames, tail_l);
    keep_tail(&conv_r, frames, tail_r);
}

/// `bufL += mono * Lg` with `mono` float32 and `Lg` a Python float: an `f32` multiply.
fn add_panned_f32(buf_l: &mut [f32], buf_r: &mut [f32], mono: &[f32], lg: f64, rg: f64) {
    let (lg, rg) = (lg as f32, rg as f32);
    for (k, &m) in mono.iter().enumerate() {
        buf_l[k] += m * lg;
        buf_r[k] += m * rg;
    }
}

/// The drain when a tone stops: only when the tail is non-empty, as Python.
fn drain_if_any(buf_l: &mut [f32], buf_r: &mut [f32], frames: usize, tl: &mut Tail, tr: &mut Tail) {
    if tl.as_ref().is_some_and(|t| !t.is_empty()) {
        drain_tails(buf_l, buf_r, tl, tr, frames);
    }
}

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    if s.inv || !s._pitch_roll_enabled {
        v._sm_roll_int *= 0.5;
        v._sm_pitch_int *= 0.5;
        return;
    }
    let frames = b.frames;
    let sr = b.sr;
    let hrtf: Option<&HrtfSet> = s.hrtf();
    let roll_rad = s.roll_rad;
    let pitch_rad = s.pitch_rad;

    let r_norm = AudioController::norm_from_angle_deg(
        roll_rad.to_degrees(),
        ATT_START_DEG,
        ATT_STOP_DEG,
        ROLL_CAP_DEG,
    );
    let p_norm = AudioController::norm_from_angle_deg(
        pitch_rad.to_degrees(),
        ATT_START_DEG,
        ATT_STOP_DEG,
        PITCH_CAP_DEG,
    );
    let (tau, dt) = (ATT_SMOOTH_MS / 1000.0, frames as f64 / sr);
    let beta = 1.0 - (-dt / py_max(1e-6, tau)).exp();
    v._sm_roll_int += beta * (r_norm - v._sm_roll_int);
    v._sm_pitch_int += beta * (p_norm - v._sm_pitch_int);

    // Stability fade: the lag between the instantaneous target and the smoothed value.
    let roll_instability = (r_norm - v._sm_roll_int).abs();
    let pitch_instability = (p_norm - v._sm_pitch_int).abs();

    let target_r_gain = clamp(
        roll_instability * ATT_FADE_SENSITIVITY + ATT_FADE_MIN_GAIN,
        0.0,
        1.0,
    );
    let tau_f = if target_r_gain > v._roll_fade_mult {
        ATT_FADE_ATTACK_MS / 1000.0
    } else {
        ATT_FADE_DECAY_MS / 1000.0
    };
    let beta_f = 1.0 - (-dt / py_max(1e-6, tau_f)).exp();
    v._roll_fade_mult += beta_f * (target_r_gain - v._roll_fade_mult);

    let target_p_gain = clamp(
        pitch_instability * ATT_FADE_SENSITIVITY + ATT_FADE_MIN_GAIN,
        0.0,
        1.0,
    );
    let tau_f = if target_p_gain > v._pitch_fade_mult {
        ATT_FADE_ATTACK_MS / 1000.0
    } else {
        ATT_FADE_DECAY_MS / 1000.0
    };
    let beta_f = 1.0 - (-dt / py_max(1e-6, tau_f)).exp();
    v._pitch_fade_mult += beta_f * (target_p_gain - v._pitch_fade_mult);

    let mut mono: Vec<f32> = Vec::with_capacity(frames);

    if v._sm_roll_int > 1e-3 {
        let f_roll = ATT_BASE_ROLL_HZ * (1.0 + 3.0 * clamp01(v._sm_roll_int));
        let inc_r = f_roll / sr;
        let a_r =
            AudioController::amp_att_from_norm(s._att_min_dbfs, s._att_max_dbfs, v._sm_roll_int)
                * v._roll_fade_mult;
        let mut last = 0.0;
        for k in 0..frames {
            let t = py_mod(k as f64 * inc_r + v._phase_roll, 1.0);
            let pulse = if t < 0.35 { 1.0 } else { -1.0 };
            mono.push((a_r * pulse) as f32);
            last = t;
        }
        // Left roll maps to 90 deg, right roll to 270.
        let pan_pos = clamp(1.0f64.copysign(roll_rad) * v._sm_roll_int, -1.0, 1.0);
        let placed = hrtf.and_then(|h| {
            let hrtf_az = py_mod(-pan_pos * 90.0, 360.0);
            h.get_hrir(hrtf_az)
        });
        if let Some((ir_l, ir_r)) = placed {
            // Distance: a shallow roll is close (0 dB), a steep one far.
            let dist_db = ATT_HRTF_FAR_DB * clamp01(v._sm_roll_int);
            let dist_gain = 10.0f64.powf(dist_db / 20.0);
            convolve_placed(
                b.buf_l,
                b.buf_r,
                frames,
                &mono,
                &ir_l,
                &ir_r,
                dist_gain,
                &mut v._roll_hrtf_overlap_L,
                &mut v._roll_hrtf_overlap_R,
            );
        } else {
            let (lg, rg) = pan_gains(pan_pos);
            add_panned_f32(b.buf_l, b.buf_r, &mono, lg, rg);
            v._roll_hrtf_overlap_L = None;
            v._roll_hrtf_overlap_R = None;
        }
        if frames > 0 {
            v._phase_roll = py_mod(last + inc_r, 1.0);
        }
    } else {
        drain_if_any(
            b.buf_l,
            b.buf_r,
            frames,
            &mut v._roll_hrtf_overlap_L,
            &mut v._roll_hrtf_overlap_R,
        );
    }

    if v._sm_pitch_int > 1e-3 {
        let f_pitch = ATT_BASE_PITCH_HZ * (1.0 + 3.0 * clamp01(v._sm_pitch_int));
        let inc_p = f_pitch / sr;
        let a_p =
            AudioController::amp_att_from_norm(s._att_min_dbfs, s._att_max_dbfs, v._sm_pitch_int)
                * v._pitch_fade_mult;
        let nose_up = pitch_rad >= 0.0;
        let amt = clamp01(v._sm_pitch_int);
        let (h3, h5) = (0.35 * amt, 0.15 * amt);
        let denom = 1.0 + h3.abs() + h5.abs();
        let two_pi = 2.0 * PI;
        mono.clear();
        let mut last = 0.0;
        for k in 0..frames {
            let t = py_mod(k as f64 * inc_p + v._phase_pitch, 1.0);
            let tri = 2.0 * (2.0 * t - 1.0).abs() - 1.0;
            let shape = if nose_up {
                // Nose up adds odd harmonics: brighter as the angle steepens.
                let ang = two_pi * t;
                (tri + h3 * (3.0 * ang).sin() + h5 * (5.0 * ang).sin()) / denom
            } else {
                tri
            };
            mono.push((a_p * shape) as f32);
            last = t;
        }
        // Nose down is in front (0 deg), nose up behind (180 deg).
        let placed = hrtf.and_then(|h| h.get_hrir(if nose_up { 180.0 } else { 0.0 }));
        if let Some((ir_l, ir_r)) = placed {
            let dist_db = ATT_HRTF_FAR_DB * clamp01(v._sm_pitch_int);
            let dist_gain = 10.0f64.powf(dist_db / 20.0);
            convolve_placed(
                b.buf_l,
                b.buf_r,
                frames,
                &mono,
                &ir_l,
                &ir_r,
                dist_gain,
                &mut v._pitch_hrtf_overlap_L,
                &mut v._pitch_hrtf_overlap_R,
            );
        } else {
            let (lg, rg) = pan_gains(0.0);
            add_panned_f32(b.buf_l, b.buf_r, &mono, lg, rg);
            v._pitch_hrtf_overlap_L = None;
            v._pitch_hrtf_overlap_R = None;
        }
        if frames > 0 {
            v._phase_pitch = py_mod(last + inc_p, 1.0);
        }
    } else {
        drain_if_any(
            b.buf_l,
            b.buf_r,
            frames,
            &mut v._pitch_hrtf_overlap_L,
            &mut v._pitch_hrtf_overlap_R,
        );
    }
}
