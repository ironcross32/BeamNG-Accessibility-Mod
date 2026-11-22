//! Low-speed clicks (audio.py:5720-5843): an FM click whose rate follows longitudinal
//! acceleration (either sign) and whose pitch follows speed, faded in and out, convolved
//! at 0 deg (in front) when HRTF is on. The standstill tone is a one-shot (`oneshots.rs`);
//! once stopped, the fade is cut short and no new click starts.

use std::sync::Arc;

use crate::callback::block::Block;
use crate::callback::common::{Wave, clamp, convolve_f32_into, convolve_full_into, play_resampled};
use crate::consts::{
    LS_ACCEL_FOR_MAX_RATE, LS_DECEL_FOR_MAX_RATE, LS_FADE_ATTACK_S, LS_FADE_DECAY_S,
    LS_HRTF_AZIMUTH, LS_MAX_RATE_HZ, LS_MIN_RATE_HZ, LS_PITCH_AT_0MPH, LS_PITCH_AT_25MPH,
    LS_STOP_FADE_S, py_max,
};
use crate::controller::{AudioController, Voice};

// `!(x > y)` keeps Python's NaN behaviour (NaN takes the reset branch).
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let dt_ls = frames as f64 / b.sr;
    if s.ls_active {
        let alpha_att = 1.0 - (-dt_ls / py_max(1e-6, LS_FADE_ATTACK_S)).exp();
        v._ls_fade_gain += alpha_att * (1.0 - v._ls_fade_gain);
    } else {
        // Decay, cut short once the vehicle has actually stopped so the clicks don't
        // trail on past the standstill tone.
        let decay_s = if s.ls_stopped {
            LS_STOP_FADE_S
        } else {
            LS_FADE_DECAY_S
        };
        let alpha_dec = 1.0 - (-dt_ls / py_max(1e-6, decay_s)).exp();
        v._ls_fade_gain += alpha_dec * (0.0 - v._ls_fade_gain);
    }

    if !(v._ls_fade_gain > 0.001) {
        v._ls_click_timer = 0.0;
        v._ls_playback_pos = -1.0;
        return;
    }

    // Rate from acceleration, normalised per side so braking can reach the top rate too.
    let accel_norm = if s.ls_accel >= 0.0 {
        clamp(s.ls_accel / LS_ACCEL_FOR_MAX_RATE, 0.0, 1.0)
    } else {
        clamp(-s.ls_accel / LS_DECEL_FOR_MAX_RATE, 0.0, 1.0)
    };
    let rate_hz = LS_MIN_RATE_HZ + (LS_MAX_RATE_HZ - LS_MIN_RATE_HZ) * accel_norm;
    let interval_sec = 1.0 / rate_hz;

    let speed_norm = clamp(s.ls_speed_mph / 25.0, 0.0, 1.0);
    let pitch = LS_PITCH_AT_0MPH + (LS_PITCH_AT_25MPH - LS_PITCH_AT_0MPH) * speed_norm;

    // Carry the remainder rather than zeroing, so block quantisation doesn't drag the
    // effective rate down at the top end.
    v._ls_click_timer += dt_ls;
    if v._ls_click_timer >= interval_sec && !s.ls_stopped {
        // Once stopped, the in-flight click rings out but no new one starts.
        v._ls_click_timer = py_max(0.0, v._ls_click_timer - interval_sec);
        v._ls_playback_pos = 0.0;
        v._ls_pitch_mult = pitch;

        // HRTF convolution at 0 deg (front).
        let placed = match (s.hrtf(), &b.waves.LS_CLICK_WAVEFORM) {
            (Some(h), Some(click)) => h.get_hrir(LS_HRTF_AZIMUTH).map(|irs| (irs, click)),
            _ => None,
        };
        if let Some(((ir_l, ir_r), click)) = placed {
            let gain =
                AudioController::hrtf_emphasis_gain_for(s._hrtf_front_emphasis_db, LS_HRTF_AZIMUTH)
                    * s._hrtf_distance_gain;
            let mut scratch = Vec::new();
            let mut conv = Vec::new();
            convolve_f32_into(click, &ir_l, &mut scratch, &mut conv);
            let l: Wave = conv.iter().map(|&x| (x * gain) as f32).collect();
            convolve_full_into(&scratch, &ir_r, &mut conv);
            let r: Wave = conv.iter().map(|&x| (x * gain) as f32).collect();
            v._ls_conv_L = Some(l);
            v._ls_conv_R = Some(r);
            v._ls_use_hrtf = true;
        } else {
            v._ls_use_hrtf = false;
        }
    }

    // Playback with pitch shifting.
    if v._ls_playback_pos >= 0.0 {
        if let Some(click) = &b.waves.LS_CLICK_WAVEFORM {
            let gain = v._ls_fade_gain;
            let (src_l, src_r): (Wave, Wave) = match (&v._ls_conv_L, &v._ls_conv_R) {
                (Some(l), Some(r)) if v._ls_use_hrtf => (l.clone(), r.clone()),
                _ => (Arc::clone(click), Arc::clone(click)),
            };
            play_resampled(
                b.buf_l,
                b.buf_r,
                &src_l,
                &src_r,
                gain,
                gain,
                &mut v._ls_playback_pos,
                v._ls_pitch_mult,
                frames,
            );
        }
    }
}
