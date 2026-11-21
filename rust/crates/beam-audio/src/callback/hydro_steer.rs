//! Articulation (hydraulic steering) FM tone and centre-crossing click
//! (audio.py:6038-6221).
//!
//! Active whenever the frame or wheels are off-centre, whatever the driver's input: on an
//! articulated machine the frame stays where it was left, and reporting that is the point.
//! `hydro_actual` is exactly 0.0 on every self-centring vehicle, so this never fires there.
//!
//! Reads the PREVIOUS block's `v._impl_gate_env` and `v._dock_gate_env`: the implement and
//! dock voices run later in the block and update them afterwards (one buffer of latency on
//! a level change, inaudible).

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{
    add_tail, clamp, clamp01, convolve_f32_into, keep_tail, linspace_open_at, pan_gains,
    play_centred,
};
use crate::consts::{
    DOCK_HYDRO_DUCK_DB, HYDRO_CENTER_REARM, HYDRO_STEER_ATTACK_TAU, HYDRO_STEER_DEADZONE,
    HYDRO_STEER_FM_DIRT_CURVE, HYDRO_STEER_FM_INDEX_HI, HYDRO_STEER_FM_INDEX_LO,
    HYDRO_STEER_FM_RATIO_HI, HYDRO_STEER_FM_RATIO_LO, HYDRO_STEER_FULL, HYDRO_STEER_HI_HZ,
    HYDRO_STEER_LO_HZ, HYDRO_STEER_MAX_AZIMUTH_DEG, HYDRO_STEER_MAX_DB, HYDRO_STEER_MIN_DB,
    HYDRO_STEER_PITCH_TAU, HYDRO_STEER_REL_TAU, IMPL_HYDRO_DUCK_DB, TRAILER_FWD_DUCK_DB,
};
use crate::controller::Voice;

const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let frames = b.frames;
    let sr = b.sr;
    let hydro_actual = b.snap.hydro_actual;
    let hydro_from_trailer = b.snap.hydro_from_trailer;
    let trailer_duck_env = b.snap.trailer_duck_env;

    let abs_hydro = hydro_actual.abs();
    let dt_hs = frames as f64 / sr;
    let hs_target = if abs_hydro > HYDRO_STEER_DEADZONE {
        1.0
    } else {
        0.0
    };
    if hs_target > 0.0 || v._hydro_steer_env > 1e-4 {
        let tau_hs = if hs_target > v._hydro_steer_env {
            HYDRO_STEER_ATTACK_TAU
        } else {
            HYDRO_STEER_REL_TAU
        };
        let env_end =
            v._hydro_steer_env + (1.0 - (-dt_hs / tau_hs).exp()) * (hs_target - v._hydro_steer_env);

        let beta_hs = 1.0 - (-dt_hs / HYDRO_STEER_PITCH_TAU).exp();
        v._hydro_steer_sm_mag += beta_hs * (abs_hydro - v._hydro_steer_sm_mag);
        let norm = clamp01(
            (v._hydro_steer_sm_mag - HYDRO_STEER_DEADZONE)
                / (HYDRO_STEER_FULL - HYDRO_STEER_DEADZONE),
        );

        // Glide everything audible across the block; stepping per callback zippers.
        let inc = (HYDRO_STEER_LO_HZ + (HYDRO_STEER_HI_HZ - HYDRO_STEER_LO_HZ) * norm) / sr;
        // Pitch tracks the angle linearly; only the timbre is curved.
        let dirt = norm.powf(HYDRO_STEER_FM_DIRT_CURVE);
        let ratio =
            HYDRO_STEER_FM_RATIO_LO + (HYDRO_STEER_FM_RATIO_HI - HYDRO_STEER_FM_RATIO_LO) * dirt;
        let index =
            HYDRO_STEER_FM_INDEX_LO + (HYDRO_STEER_FM_INDEX_HI - HYDRO_STEER_FM_INDEX_LO) * dirt;

        let inc_prev = if v._hydro_steer_inc > 0.0 {
            v._hydro_steer_inc
        } else {
            inc
        };
        let ratio_prev = if v._hydro_steer_ratio > 0.0 {
            v._hydro_steer_ratio
        } else {
            ratio
        };
        let index_prev = if v._hydro_steer_index > 0.0 {
            v._hydro_steer_index
        } else {
            index
        };

        // dB range rather than a linear scale to zero, so "nearly straight" stays audible.
        // Ducked while the implement is worked and much harder while the docking
        // instrument is open (the goal wins); both gates are last block's values. The
        // trailer term is zero unless the trailer is the source and we are going forward.
        let amp = 10.0f64.powf(
            (HYDRO_STEER_MIN_DB
                + (HYDRO_STEER_MAX_DB - HYDRO_STEER_MIN_DB) * norm
                + IMPL_HYDRO_DUCK_DB * v._impl_gate_env
                + DOCK_HYDRO_DUCK_DB * v._dock_gate_env
                + TRAILER_FWD_DUCK_DB * trailer_duck_env)
                / 20.0,
        );

        // FM: sin(carrier + index * sin(modulator)); float64, then one cast to float32.
        let mut mono_hs: Vec<f32> = Vec::with_capacity(frames);
        let mut cum_inc = 0.0f64;
        let mut cum_mod = 0.0f64;
        let mut last_phase = 0.0f64;
        let mut last_mod_phase = 0.0f64;
        for k in 0..frames {
            let inc_k = linspace_open_at(inc_prev, inc, frames, k);
            let ratio_k = linspace_open_at(ratio_prev, ratio, frames, k);
            let index_k = linspace_open_at(index_prev, index, frames, k);
            cum_inc += inc_k;
            cum_mod += inc_k * ratio_k;
            let phase = py_mod(v._hydro_steer_phase + cum_inc, 1.0);
            let mod_phase = py_mod(v._hydro_steer_mod_phase + cum_mod, 1.0);
            last_phase = phase;
            last_mod_phase = mod_phase;
            let s = amp * (TWO_PI * phase + index_k * (TWO_PI * mod_phase).sin()).sin();
            let env_k = linspace_open_at(v._hydro_steer_env, env_end, frames, k);
            mono_hs.push((s * env_k) as f32);
        }

        v._hydro_steer_inc = inc;
        v._hydro_steer_ratio = ratio;
        v._hydro_steer_index = index;
        if frames > 0 {
            v._hydro_steer_phase = last_phase;
            v._hydro_steer_mod_phase = last_mod_phase;
        }
        v._hydro_steer_env = env_end;

        // HRTF azimuth proportional to the articulation, clamped so an out-of-range value
        // cannot swing the image past the limit. 0 deg = centre.
        let hrtf_az_hs = py_mod(
            HYDRO_STEER_MAX_AZIMUTH_DEG * clamp(hydro_actual, -1.0, 1.0),
            360.0,
        );
        let irs = b.snap.hrtf().and_then(|h| h.get_hrir(hrtf_az_hs));
        if let Some((ir_l, ir_r)) = irs {
            let mut scratch = Vec::new();
            let mut conv_l = Vec::new();
            let mut conv_r = Vec::new();
            convolve_f32_into(&mono_hs, &ir_l, &mut scratch, &mut conv_l);
            convolve_f32_into(&mono_hs, &ir_r, &mut scratch, &mut conv_r);
            if v._hydro_steer_overlap_L.is_some() {
                // ol = min(len(overlap_L), len(conv_l)) for both channels.
                let ol = v
                    ._hydro_steer_overlap_L
                    .as_ref()
                    .map_or(0, |t| t.len())
                    .min(conv_l.len());
                add_tail(&mut conv_l[..ol], &v._hydro_steer_overlap_L);
                add_tail(&mut conv_r[..ol], &v._hydro_steer_overlap_R);
            }
            for k in 0..frames.min(conv_l.len()) {
                b.buf_l[k] += conv_l[k] as f32;
                b.buf_r[k] += conv_r[k] as f32;
            }
            keep_tail(&conv_l, frames, &mut v._hydro_steer_overlap_L);
            keep_tail(&conv_r, frames, &mut v._hydro_steer_overlap_R);
        } else {
            // Equal-power pan. hydro_actual is positive-LEFT, _pan_gains positive-right.
            let (lg, rg) = pan_gains(-hydro_actual);
            let (lg, rg) = (lg as f32, rg as f32);
            for k in 0..frames {
                b.buf_l[k] += mono_hs[k] * lg;
                b.buf_r[k] += mono_hs[k] * rg;
            }
            v._hydro_steer_overlap_L = None;
            v._hydro_steer_overlap_R = None;
        }
    } else {
        // Not active: drain any remaining HRTF tail.
        if let Some(tl) = v._hydro_steer_overlap_L.as_ref() {
            if !tl.is_empty() {
                let ol = tl.len().min(frames);
                for k in 0..ol {
                    b.buf_l[k] += tl[k] as f32;
                }
                if let Some(tr) = v._hydro_steer_overlap_R.as_ref() {
                    for k in 0..ol.min(tr.len()) {
                        b.buf_r[k] += tr[k] as f32;
                    }
                }
            }
        }
        v._hydro_steer_overlap_L = None;
        v._hydro_steer_overlap_R = None;
        v._hydro_steer_env = 0.0;
        v._hydro_steer_inc = 0.0;
        v._hydro_steer_ratio = 0.0;
        v._hydro_steer_index = 0.0;
        v._hydro_steer_sm_mag = abs_hydro;
    }

    // Centre-crossing click: marks the moment of arriving at straight, since silence also
    // means "idle". Arms only well outside the deadzone so hunting cannot machine-gun it.
    //
    // A change of SOURCE disarms rather than fires: uncoupling an angled trailer drops the
    // value straight to 0.0, which would otherwise click "straight" about a trailer that is
    // no longer there.
    if hydro_from_trailer != v._hydro_was_from_trailer {
        v._hydro_center_armed = false;
        v._hydro_was_from_trailer = hydro_from_trailer;
    }

    if abs_hydro > HYDRO_STEER_DEADZONE * HYDRO_CENTER_REARM {
        v._hydro_center_armed = true;
    } else if v._hydro_center_armed && abs_hydro <= HYDRO_STEER_DEADZONE {
        v._hydro_center_armed = false;
        if b.waves.HYDRO_CENTER_CLICK_WAVEFORM.is_some() {
            v._hydro_center_pos = 0.0;
        }
    }

    if v._hydro_center_pos >= 0.0 {
        if let Some(wf) = b.waves.HYDRO_CENTER_CLICK_WAVEFORM.as_ref() {
            // Centred: straight-ahead needs no spatialisation.
            play_centred(b.buf_l, b.buf_r, wf, &mut v._hydro_center_pos, frames);
        }
    }
}
