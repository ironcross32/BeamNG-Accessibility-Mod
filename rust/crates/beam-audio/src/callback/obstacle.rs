//! Predictive obstacle cue: doublet, cadence pulses, steady emergency spectrum, terrain
//! sweep (audio.py:5371-5514). Not ducked. A2.
//!
//! One trajectory-selected source. Packet updates never reset the cadence; they only move
//! the target and update its state.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{
    PulseEntry, add_tail, clamp01, convolve_f32_into, keep_tail, mix_pulse_queue, pan_gains,
    play_centred, render_directional_pulse,
};
use crate::consts::*;
use crate::controller::Voice;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let Some(buzz) = b.waves.OBSTACLE_BUZZ_WAVEFORM.as_ref() else {
        return;
    };
    if !s.obs_active {
        return;
    }
    let frames = b.frames;
    let sr = b.sr;
    let dt_obs = frames as f64 / sr;
    let hazard = s.obstacle_hazard.as_ref();
    let obstacle_pulses_were_active =
        v._obstacle_doublet_requested || !v._obstacle_pulse_queue.is_empty();

    if let Some(h) = hazard {
        let target_bearing = h.bearing;
        let beta = 1.0 - (-dt_obs / OBSTACLE_BEARING_CROSSFADE_S).exp();
        v._obstacle_render_bearing += beta * (target_bearing - v._obstacle_render_bearing);

        if v._obstacle_doublet_requested {
            let (pulse_l, pulse_r) = render_directional_pulse(
                s._hrtf.as_deref(),
                s._hrtf_user_enabled,
                buzz,
                v._obstacle_render_bearing,
            );
            let second_delay =
                buzz.len() as i64 + (sr * OBSTACLE_DOUBLET_GAP_MS as f64 / 1000.0).trunc() as i64;
            v._obstacle_pulse_queue = vec![
                PulseEntry {
                    delay: 0,
                    L: pulse_l.clone(),
                    R: pulse_r.clone(),
                    pos: 0,
                },
                PulseEntry {
                    delay: second_delay,
                    L: pulse_l,
                    R: pulse_r,
                    pos: 0,
                },
            ];
            v._obstacle_doublet_requested = false;
            v._obstacle_cadence_clock = 0.0;
        } else if h.state < 3 {
            v._obstacle_cadence_clock += dt_obs;
            let rate_hz = obstacle_cue_rate_hz(h.state, h.urgency as f64);
            if v._obstacle_pulse_queue.is_empty() && v._obstacle_cadence_clock >= 1.0 / rate_hz {
                v._obstacle_cadence_clock = py_mod(v._obstacle_cadence_clock, 1.0 / rate_hz);
                let level = if h.state == 1 {
                    10.0f64.powf(OBSTACLE_ADVISORY_OFFSET_DB / 20.0)
                } else {
                    1.0
                };
                // float32 waveform times a Python float: float32.
                let level32 = level as f32;
                let scaled: Vec<f32> = buzz.iter().map(|&x| x * level32).collect();
                let (pulse_l, pulse_r) = render_directional_pulse(
                    s._hrtf.as_deref(),
                    s._hrtf_user_enabled,
                    &scaled,
                    v._obstacle_render_bearing,
                );
                v._obstacle_pulse_queue = vec![PulseEntry {
                    delay: 0,
                    L: pulse_l,
                    R: pulse_r,
                    pos: 0,
                }];
            }
        }
    }

    if !v._obstacle_pulse_queue.is_empty() {
        mix_pulse_queue(&mut v._obstacle_pulse_queue, b.buf_l, b.buf_r, frames);
    }

    // Emergency becomes the same spectrum held steadily only after its entry doublet has
    // completed. It releases even if the clear arrives mid-block.
    let steady_target = hazard.is_some_and(|h| h.state >= 3 && h.closing > 0.0)
        && v._obstacle_pulse_queue.is_empty()
        && !v._obstacle_doublet_requested
        && !obstacle_pulses_were_active;
    let tau = if steady_target {
        OBSTACLE_STEADY_ATTACK_S
    } else {
        OBSTACLE_STEADY_RELEASE_S
    };
    let target_env = if steady_target { 1.0 } else { 0.0 };
    let denom = py_max(1.0, sr * tau);
    let start_env = v._obstacle_steady_env;
    let steady_env: Vec<f64> = (1..=frames)
        .map(|n| target_env + (start_env - target_env) * (-(n as f64) / denom).exp())
        .collect();
    if let Some(&last) = steady_env.last() {
        v._obstacle_steady_env = last;
    }
    if v._obstacle_steady_env > 1e-5 || steady_target {
        let inc = OBSTACLE_CUE_FREQ_HZ / sr;
        let peak = py_max(1e-9, b.waves._obstacle_harmonic_peak.unwrap_or(1.0));
        let level = s._obstacle_amp;
        let offset = 10.0f64.powf(OBSTACLE_STEADY_OFFSET_DB / 20.0);
        let mut last_phase = 0.0;
        let mono: Vec<f32> = (0..frames)
            .map(|k| {
                let phase = v._obstacle_steady_phase + k as f64 * inc;
                last_phase = phase;
                let mut m = 0.0f64;
                for &(harmonic, amplitude) in OBSTACLE_CUE_HARMONICS.iter() {
                    m += amplitude * (2.0 * PI * harmonic as f64 * phase).sin();
                }
                m /= peak;
                (m * steady_env[k] * level * offset) as f32
            })
            .collect();
        if frames > 0 {
            v._obstacle_steady_phase = py_mod(last_phase + OBSTACLE_CUE_FREQ_HZ / sr, 1.0);
        }
        let ir = s
            .hrtf()
            .and_then(|h| h.get_hrir(py_mod(v._obstacle_render_bearing, 360.0)));
        if let Some((ir_l, ir_r)) = ir {
            let mut scratch = Vec::new();
            let mut conv_l = Vec::new();
            let mut conv_r = Vec::new();
            convolve_f32_into(&mono, &ir_l, &mut scratch, &mut conv_l);
            convolve_f32_into(&mono, &ir_r, &mut scratch, &mut conv_r);
            if let Some(tl) = v._obstacle_steady_overlap_L.as_ref() {
                let ol = tl.len().min(conv_l.len());
                add_tail(&mut conv_l[..ol], &v._obstacle_steady_overlap_L);
                let olr = ol.min(conv_r.len());
                add_tail(&mut conv_r[..olr], &v._obstacle_steady_overlap_R);
            }
            for k in 0..frames.min(conv_l.len()) {
                b.buf_l[k] += conv_l[k] as f32;
                b.buf_r[k] += conv_r[k] as f32;
            }
            keep_tail(&conv_l, frames, &mut v._obstacle_steady_overlap_L);
            keep_tail(&conv_r, frames, &mut v._obstacle_steady_overlap_R);
        } else {
            v._obstacle_steady_overlap_L = None;
            v._obstacle_steady_overlap_R = None;
            // `self._clamp` with its default (0, 1) range: a right-hand bearing pans to
            // centre in this fallback. Kept as the Python has it.
            let pan_pos = clamp01(-v._obstacle_render_bearing / 90.0);
            let (gain_l, gain_r) = pan_gains(pan_pos);
            let (gain_l, gain_r) = (gain_l as f32, gain_r as f32);
            for k in 0..frames {
                b.buf_l[k] += mono[k] * gain_l;
                b.buf_r[k] += mono[k] * gain_r;
            }
        }
    }

    // Terrain warning sweep, centred (ahead of the vehicle), intentionally never ducked.
    if v._terrain_playback_pos >= 0.0 {
        let sweep = if s.terrain_type == 2 {
            b.waves.DROPOFF_SWEEP_WAVEFORM.as_ref()
        } else {
            b.waves.HILL_SWEEP_WAVEFORM.as_ref()
        };
        if let Some(wf) = sweep {
            play_centred(b.buf_l, b.buf_r, wf, &mut v._terrain_playback_pos, frames);
        }
    }
}
