//! Coupler tracking: a continuous sine with HRTF, beeping when in range
//! (audio.py:5289-5369). Not ducked. A2.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{add_tail, convolve_f32_into, drain_tails, keep_tail, pan_gains};
use crate::consts::*;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let sr = b.sr;
    let mut coupler_produced_audio = false;
    if s.coupler_active && s.coupler_dist != f64::INFINITY {
        let amp = 10.0f64.powf(COUPLER_TONE_AMP_DB / 20.0);
        // np.sin(...).astype(np.float32) * amp: the multiply is float32.
        let amp32 = amp as f32;
        let mut tone = vec![0.0f32; frames];

        if s.coupler_in_range {
            // Rapid beeping: pitch higher, rate rising as the distance falls.
            let pitch_mult = COUPLER_BEEP_PITCH;
            let range_frac = py_max(0.0, 1.0 - (s.coupler_dist / COUPLER_RANGE_M));
            let beep_rate = COUPLER_BEEP_MIN_RATE
                + (COUPLER_BEEP_MAX_RATE - COUPLER_BEEP_MIN_RATE) * range_frac;
            let beep_period = 1.0 / beep_rate;
            let beep_on = COUPLER_BEEP_DUR_MS as f64 / 1000.0;

            let freq = COUPLER_TONE_FREQ_HZ * pitch_mult;
            let inc = freq / sr;
            let mut last_phase = 0.0;
            let mut last_env = 0.0;
            for (k, t) in tone.iter_mut().enumerate() {
                let t_phase = v._coupler_phase + k as f64 * inc;
                let t_env = v._coupler_beep_timer + k as f64 / sr;
                let gate: f32 = if py_mod(t_env, beep_period) < beep_on {
                    1.0
                } else {
                    0.0
                };
                *t = ((2.0 * PI * t_phase).sin() as f32 * amp32) * gate;
                last_phase = t_phase;
                last_env = t_env;
            }
            if frames > 0 {
                v._coupler_phase = py_mod(last_phase + inc, 1.0);
                v._coupler_beep_timer = last_env + (1.0 / sr);
            }
        } else {
            // Continuous tone, pitch scaling with distance.
            let dist_norm = (-s.coupler_dist / COUPLER_HALF_DIST_M).exp();
            let pitch_mult =
                COUPLER_MIN_PITCH + (COUPLER_MAX_PITCH - COUPLER_MIN_PITCH) * dist_norm;
            let freq = COUPLER_TONE_FREQ_HZ * pitch_mult;
            let inc = freq / sr;
            let mut last_phase = 0.0;
            for (k, t) in tone.iter_mut().enumerate() {
                let t_phase = v._coupler_phase + k as f64 * inc;
                *t = (2.0 * PI * t_phase).sin() as f32 * amp32;
                last_phase = t_phase;
            }
            if frames > 0 {
                v._coupler_phase = py_mod(last_phase + inc, 1.0);
            }
            v._coupler_beep_timer = 0.0;
        }

        let hrtf_az = py_mod(s.coupler_bearing, 360.0);
        let ir = s.hrtf().and_then(|h| h.get_hrir(hrtf_az));
        if let Some((ir_l, ir_r)) = ir {
            let mut scratch = Vec::new();
            let mut conv_l = Vec::new();
            let mut conv_r = Vec::new();
            convolve_f32_into(&tone, &ir_l, &mut scratch, &mut conv_l);
            convolve_f32_into(&tone, &ir_r, &mut scratch, &mut conv_r);
            if let Some(tl) = v._coupler_overlap_L.as_ref() {
                // ol comes from the left tail; both tails have equal length.
                let ol = tl.len().min(conv_l.len());
                add_tail(&mut conv_l[..ol], &v._coupler_overlap_L);
                let olr = ol.min(conv_r.len());
                add_tail(&mut conv_r[..olr], &v._coupler_overlap_R);
            }
            for k in 0..frames.min(conv_l.len()) {
                b.buf_l[k] += conv_l[k] as f32;
                b.buf_r[k] += conv_r[k] as f32;
            }
            keep_tail(&conv_l, frames, &mut v._coupler_overlap_L);
            keep_tail(&conv_r, frames, &mut v._coupler_overlap_R);
            coupler_produced_audio = true;
        } else {
            let pan_pos = -s.coupler_bearing / 90.0;
            let (lg, rg) = pan_gains(pan_pos);
            let (lg, rg) = (lg as f32, rg as f32);
            for k in 0..frames {
                b.buf_l[k] += tone[k] * lg;
                b.buf_r[k] += tone[k] * rg;
            }
        }
    }

    // Drain the coupler's HRTF tail when no audio was produced this block.
    if !coupler_produced_audio {
        drain_tails(
            b.buf_l,
            b.buf_r,
            &mut v._coupler_overlap_L,
            &mut v._coupler_overlap_R,
            frames,
        );
    }
}
