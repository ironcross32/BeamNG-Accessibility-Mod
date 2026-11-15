//! Road correction pips / unwind doublet and the settled sweep (audio.py:5624-5654), with
//! `_render_road_correction_block` (2515-2620). Inside the road duck bus. A2.
//!
//! Directional pips say to apply steering; a repeating doublet says to ease off while
//! keeping direction. The frontal arc keeps a correction from sounding like a target
//! beside or behind the car.
//!
//! `_stop_road_correction_voice` is `AudioController::stop_road_correction_voice_locked`.

use std::f64::consts::PI;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{clamp, convolve_f32_into, pan_gains};
use crate::consts::*;
use crate::controller::{AudioController, Voice};
use crate::hrtf::HrtfSet;

/// 5624-5654: the correction block when `road_active and road_on_road` (else stop the
/// voice), then the settled sweep.
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    if s.road_active && s.road_on_road {
        let block = render_road_correction_block(
            v,
            s.hrtf(),
            b.sr,
            s._road_correction_amp,
            frames,
            s.road_correction_active,
            s.road_correction_bearing,
            s.road_correction_severity,
            &s.road_correction_phase,
        );
        if let Some((cl, cr)) = block {
            for k in 0..frames {
                b.buf_l[k] += cl[k];
                b.buf_r[k] += cr[k];
            }
        }
    } else {
        AudioController::stop_road_correction_voice_locked(v);
    }

    if s.road_active && v._road_correction_settled_pos >= 0 {
        if let Some(waveform) = b.waves.ROAD_CORRECTION_SETTLED_WAVEFORM.as_ref() {
            let start = v._road_correction_settled_pos;
            let len = waveform.len() as i64;
            let count = (frames as i64).min(len - start);
            if count > 0 {
                let st = start as usize;
                for k in 0..count as usize {
                    b.buf_l[k] += waveform[st + k];
                    b.buf_r[k] += waveform[st + k];
                }
            }
            v._road_correction_settled_pos = start + frames as i64;
            if v._road_correction_settled_pos >= len {
                v._road_correction_settled_pos = -1;
            }
        } else {
            v._road_correction_settled_pos = -1;
        }
    }
}

/// `_render_road_correction_block(frames, active, bearing_deg, severity, phase)`:
/// `None` where Python returns None, else the `(L, R)` float32 block.
/// `hrtf` is `self._hrtf` only when `_hrtf_user_enabled`; `amp` is `_road_correction_amp`.
#[allow(clippy::too_many_arguments)]
pub fn render_road_correction_block(
    v: &mut Voice,
    hrtf: Option<&HrtfSet>,
    sr: f64,
    amp: f64,
    frames: usize,
    active: bool,
    bearing_deg: f64,
    severity: f64,
    phase: &str,
) -> Option<(Vec<f32>, Vec<f32>)> {
    if frames == 0 {
        return None;
    }

    if active {
        let bearing = bearing_deg;
        // Small pursuit angles still need a clearly audible side. Cadence and the doublet
        // convey demand; azimuth preserves its direction.
        v._road_correction_render_bearing = if bearing != 0.0 {
            clamp(
                bearing.abs(),
                ROAD_CORRECTION_MIN_AZIMUTH_DEG,
                ROAD_CORRECTION_MAX_AZIMUTH_DEG,
            )
            .copysign(bearing)
        } else {
            0.0
        };
    }

    let target_env = if active { 1.0 } else { 0.0 };
    let tau = if active {
        ROAD_CORRECTION_ATTACK_S
    } else {
        ROAD_CORRECTION_RELEASE_S
    };
    let denom = py_max(1.0, sr * tau);
    let start_env = v._road_correction_env;
    let envelope: Vec<f64> = (1..=frames)
        .map(|n| target_env + (start_env - target_env) * (-(n as f64) / denom).exp())
        .collect();
    v._road_correction_env = envelope[frames - 1];

    if !active && v._road_correction_env < 1e-4 {
        AudioController::stop_road_correction_voice_locked(v);
        return None;
    }

    let amount = py_min(1.0, py_max(0.0, severity));
    let pulse_phase = v._road_correction_pulse_phase;
    let pulse_time = |k: usize| pulse_phase + k as f64 / sr;
    let mut pulse_envelope = vec![0.0f64; frames];
    let cycle_s;
    let frequency;
    if phase == "unwind" {
        cycle_s = ROAD_CORRECTION_UNWIND_REPEAT_S;
        let pip_s = ROAD_CORRECTION_UNWIND_PIP_MS as f64 / 1000.0;
        let second_start = pip_s + ROAD_CORRECTION_UNWIND_GAP_MS as f64 / 1000.0;
        for (k, pe) in pulse_envelope.iter_mut().enumerate() {
            let local_time = py_mod(pulse_time(k), cycle_s);
            for start in [0.0, second_start] {
                let progress = (local_time - start) / pip_s;
                if progress >= 0.0 && progress < 1.0 {
                    *pe = (PI * progress).sin();
                }
            }
        }
        frequency = ROAD_CORRECTION_UNWIND_HZ;
    } else {
        let rate_hz = road_correction_rate_hz(amount);
        cycle_s = 1.0 / rate_hz;
        let pip_s = py_min(ROAD_CORRECTION_PIP_MS as f64 / 1000.0, cycle_s * 0.65);
        for (k, pe) in pulse_envelope.iter_mut().enumerate() {
            let local_time = py_mod(pulse_time(k), cycle_s);
            let progress = local_time / pip_s;
            *pe = if progress < 1.0 {
                (PI * progress.min(1.0)).sin()
            } else {
                0.0
            };
        }
        frequency = if amount >= 0.67 {
            ROAD_CORRECTION_HIGH_HZ
        } else if amount >= 0.34 {
            ROAD_CORRECTION_MID_HZ
        } else {
            ROAD_CORRECTION_CARRIER_HZ
        };
    }

    let carrier_inc = frequency / sr;
    let carrier0 = v._road_correction_carrier_phase;
    let mono: Vec<f32> = (0..frames)
        .map(|k| {
            let carrier_phase = carrier0 + carrier_inc * k as f64;
            ((2.0 * PI * carrier_phase).sin() * pulse_envelope[k] * envelope[k] * amp) as f32
        })
        .collect();
    v._road_correction_carrier_phase = py_mod(carrier0 + carrier_inc * frames as f64, 1.0);
    v._road_correction_pulse_phase = py_mod(pulse_phase + frames as f64 / sr, cycle_s);

    let bearing = v._road_correction_render_bearing;
    if let Some((ir_l, ir_r)) = hrtf.and_then(|h| h.get_hrir(py_mod(bearing, 360.0))) {
        let mut scratch = Vec::new();
        let mut conv_l = Vec::new();
        let mut conv_r = Vec::new();
        convolve_f32_into(&mono, &ir_l, &mut scratch, &mut conv_l);
        convolve_f32_into(&mono, &ir_r, &mut scratch, &mut conv_r);
        if let Some(tl) = v._road_correction_overlap_L.as_ref() {
            let overlap_l = tl.len().min(conv_l.len());
            for k in 0..overlap_l {
                conv_l[k] += tl[k];
            }
            if let Some(tr) = v._road_correction_overlap_R.as_ref() {
                let overlap_r = tr.len().min(conv_r.len());
                for k in 0..overlap_r {
                    conv_r[k] += tr[k];
                }
            }
        }
        let tail = |c: &[f64]| -> Vec<f64> {
            if frames < c.len() {
                c[frames..].to_vec()
            } else {
                Vec::new()
            }
        };
        v._road_correction_overlap_L = Some(tail(&conv_l));
        v._road_correction_overlap_R = Some(tail(&conv_r));
        let head = |c: &[f64]| -> Vec<f32> { c[..frames.min(c.len())].iter().map(|&x| x as f32).collect() };
        return Some((head(&conv_l), head(&conv_r)));
    }

    v._road_correction_overlap_L = None;
    v._road_correction_overlap_R = None;
    let pan_pos = -bearing / ROAD_CORRECTION_MAX_AZIMUTH_DEG;
    let (gain_l, gain_r) = pan_gains(pan_pos);
    let (gain_l, gain_r) = (gain_l as f32, gain_r as f32);
    Some((
        mono.iter().map(|&x| x * gain_l).collect(),
        mono.iter().map(|&x| x * gain_r).collect(),
    ))
}
