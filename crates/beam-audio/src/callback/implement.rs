//! Loader implement: ground-proximity FM tone and quarter-tone tilt sine
//! (audio.py:6223-6457).
//!
//! Both are gated by one shared fade envelope and split left/right by a fixed equal-power
//! pan (`IMPL_STREAM_PAN`) so they stay two streams rather than fusing into one. Silent
//! unless the telemetry says an implement was resolved on this vehicle. Writes
//! `v._impl_gate_env`, which hydro_steer reads (one block late) for its duck.

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{clamp, clamp01, linspace_open_at};
use crate::consts::{
    IMPL_CLOSING_STALE_S, IMPL_CLOSING_TAU, IMPL_GATE_ATTACK_TAU, IMPL_GATE_CUT, IMPL_GATE_OFF,
    IMPL_GATE_ON, IMPL_GATE_REL_TAU, IMPL_GROUND_CARRIER_HZ, IMPL_GROUND_CLOSING_MPS,
    IMPL_GROUND_DIRT_CURVE, IMPL_GROUND_FAR_M, IMPL_GROUND_GAIN_L, IMPL_GROUND_GAIN_R,
    IMPL_GROUND_INDEX_HI, IMPL_GROUND_INDEX_LO, IMPL_GROUND_JACK_FULL_M, IMPL_GROUND_JACK_INDEX,
    IMPL_GROUND_JACK_OCTAVES, IMPL_GROUND_RATIO_BASE, IMPL_GROUND_RATIO_DRIFT, IMPL_GROUND_SPAN_DB,
    IMPL_TILT_FALLBACK_SPAN_DEG, IMPL_TILT_GAIN_L, IMPL_TILT_GAIN_R, IMPL_TILT_GLIDE_TAU,
    IMPL_TILT_LEVEL_HZ, IMPL_TILT_MIN_SPAN_DEG, IMPL_TILT_SPAN_DB, IMPL_TILT_SPAN_SWAP_DEG,
    IMPL_TILT_STEP_DIP_DB, IMPL_TILT_STEP_DIP_MS, IMPL_TILT_STEPS_PER_OCT,
    IMPL_TILT_STOP_CONFIRM_DEG, py_max,
};
use crate::controller::Voice;

const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let frames = b.frames;
    let sr = b.sr;
    let s = b.snap;
    let impl_flags = s.impl_flags;
    let impl_clearance = s.impl_clearance;
    let impl_tilt_deg = s.impl_tilt_deg;
    let impl_lift = s.impl_lift;
    let impl_activity = s.impl_activity;

    let dt_impl = frames as f64 / sr;
    let impl_present = (impl_flags & 1) != 0 && s.impl_enabled;
    let impl_ground_ok = (impl_flags & 4) != 0 && impl_clearance >= 0.0;

    // Closing rate from the clearance itself; negative = converging, which counts as
    // activity with the joystick released. Measured against the time since the value last
    // CHANGED: it arrives at 20 Hz while this runs at ~100 Hz.
    if !impl_ground_ok {
        v._impl_prev_clearance = None;
        v._impl_clear_elapsed = 0.0;
        v._impl_closing = 0.0;
    } else {
        v._impl_clear_elapsed += dt_impl;
        if Some(impl_clearance) != v._impl_prev_clearance {
            if let Some(prev) = v._impl_prev_clearance {
                if v._impl_clear_elapsed > 1e-4 {
                    let raw = (impl_clearance - prev) / v._impl_clear_elapsed;
                    // Blend weighted by the time this sample actually covers.
                    let alpha = 1.0 - (-v._impl_clear_elapsed / IMPL_CLOSING_TAU).exp();
                    v._impl_closing += alpha * (raw - v._impl_closing);
                }
            }
            v._impl_prev_clearance = Some(impl_clearance);
            v._impl_clear_elapsed = 0.0;
        } else if v._impl_clear_elapsed > IMPL_CLOSING_STALE_S {
            // Telemetry stopped: let the estimate go rather than latching the gate.
            v._impl_closing = 0.0;
        }
    }
    let closing = v._impl_closing;

    // Shared fade gate: Schmitt trigger, asymmetric one-pole with a hard cut.
    let mut gate_target = if v._impl_gate_open { 1.0 } else { 0.0 };
    if impl_present {
        if impl_activity > IMPL_GATE_ON || closing < -IMPL_GROUND_CLOSING_MPS {
            gate_target = 1.0;
        } else if impl_activity < IMPL_GATE_OFF {
            gate_target = 0.0;
        }
    } else {
        gate_target = 0.0;
    }
    v._impl_gate_open = gate_target > 0.0;
    let tau_impl = if gate_target > v._impl_gate_env {
        IMPL_GATE_ATTACK_TAU
    } else {
        IMPL_GATE_REL_TAU
    };
    let mut gate_end =
        v._impl_gate_env + (1.0 - (-dt_impl / tau_impl).exp()) * (gate_target - v._impl_gate_env);
    if gate_end < IMPL_GATE_CUT && gate_target == 0.0 {
        gate_end = 0.0;
    }
    let gate_start = v._impl_gate_env;
    let gate_at = |k: usize| linspace_open_at(gate_start, gate_end, frames, k);
    let gate_was = v._impl_gate_env;
    v._impl_gate_env = gate_end;

    if impl_present && (gate_end > 0.0 || gate_was > 0.0) {
        // ---- Ground proximity ----------------------------------------------------------
        if impl_ground_ok {
            let prox = 1.0 - clamp(impl_clearance / IMPL_GROUND_FAR_M, 0.0, 1.0);
            let dirt = prox.powf(IMPL_GROUND_DIRT_CURVE);
            let jack = clamp(impl_lift / IMPL_GROUND_JACK_FULL_M, 0.0, 1.0);
            // Smoothstep, so the ratio settles onto the integer rather than sliding through.
            let jb = jack * jack * (3.0 - 2.0 * jack);

            let ratio_dirty = IMPL_GROUND_RATIO_BASE + IMPL_GROUND_RATIO_DRIFT * dirt;
            let ratio = ratio_dirty + (IMPL_GROUND_RATIO_BASE + 1.0 - ratio_dirty) * jb;
            let index_dirty =
                IMPL_GROUND_INDEX_LO + (IMPL_GROUND_INDEX_HI - IMPL_GROUND_INDEX_LO) * dirt;
            let index = index_dirty + (IMPL_GROUND_JACK_INDEX - index_dirty) * jb;
            let carrier = IMPL_GROUND_CARRIER_HZ * 2.0f64.powf(jack * IMPL_GROUND_JACK_OCTAVES);
            let inc_g = carrier / sr;

            let inc_prev = if v._impl_ground_inc > 0.0 {
                v._impl_ground_inc
            } else {
                inc_g
            };
            let ratio_prev = if v._impl_ground_ratio > 0.0 {
                v._impl_ground_ratio
            } else {
                ratio
            };
            let index_prev = if v._impl_ground_index > 0.0 {
                v._impl_ground_index
            } else {
                index
            };

            // The configured level is the LOUD end and the span hangs below it.
            let g_amp = 10.0f64
                .powf((s._impl_ground_db - IMPL_GROUND_SPAN_DB * (1.0 - py_max(prox, jb))) / 20.0);

            let (gl, gr) = (IMPL_GROUND_GAIN_L as f32, IMPL_GROUND_GAIN_R as f32);
            let mut cum_inc = 0.0f64;
            let mut cum_mod = 0.0f64;
            let mut last_ph = 0.0f64;
            let mut last_mph = 0.0f64;
            for k in 0..frames {
                let inc_k = linspace_open_at(inc_prev, inc_g, frames, k);
                let ratio_k = linspace_open_at(ratio_prev, ratio, frames, k);
                let index_k = linspace_open_at(index_prev, index, frames, k);
                cum_inc += inc_k;
                cum_mod += inc_k * ratio_k;
                let ph = py_mod(v._impl_ground_phase + cum_inc, 1.0);
                let mph = py_mod(v._impl_ground_mod_phase + cum_mod, 1.0);
                last_ph = ph;
                last_mph = mph;
                let mono = g_amp * (TWO_PI * ph + index_k * (TWO_PI * mph).sin()).sin();
                let mono_g = (mono * gate_at(k)) as f32;
                b.buf_l[k] += mono_g * gl;
                b.buf_r[k] += mono_g * gr;
            }
            v._impl_ground_inc = inc_g;
            v._impl_ground_ratio = ratio;
            v._impl_ground_index = index;
            if frames > 0 {
                v._impl_ground_phase = last_ph;
                v._impl_ground_mod_phase = last_mph;
            }
        } else {
            v._impl_ground_inc = 0.0;
            v._impl_ground_ratio = 0.0;
            v._impl_ground_index = 0.0;
        }

        // ---- Tilt ----------------------------------------------------------------------
        // Learn the real mechanical stops. A running extreme is only a stop once the
        // implement has come back OFF it; while still pushed outward the extreme IS the
        // current angle and the step would pin to the top of the scale.
        if impl_tilt_deg > v._impl_tilt_seen_up {
            v._impl_tilt_seen_up = impl_tilt_deg;
        } else if impl_tilt_deg < v._impl_tilt_seen_up - IMPL_TILT_STOP_CONFIRM_DEG {
            v._impl_tilt_stop_up = v._impl_tilt_seen_up;
        }
        if -impl_tilt_deg > v._impl_tilt_seen_dn {
            v._impl_tilt_seen_dn = -impl_tilt_deg;
        } else if -impl_tilt_deg < v._impl_tilt_seen_dn - IMPL_TILT_STOP_CONFIRM_DEG {
            v._impl_tilt_stop_dn = v._impl_tilt_seen_dn;
        }
        // Swap a newly confirmed stop in only while passing level, where every span maps
        // to step 0 and the change is inaudible.
        if impl_tilt_deg.abs() < IMPL_TILT_SPAN_SWAP_DEG {
            v._impl_tilt_span_up = v._impl_tilt_stop_up;
            v._impl_tilt_span_dn = v._impl_tilt_stop_dn;
        }
        let mut span = if impl_tilt_deg >= 0.0 {
            v._impl_tilt_span_up
        } else {
            v._impl_tilt_span_dn
        };
        if span < IMPL_TILT_MIN_SPAN_DEG {
            span = IMPL_TILT_FALLBACK_SPAN_DEG;
        }

        let steps_per_oct = IMPL_TILT_STEPS_PER_OCT as f64;
        let step_f = (steps_per_oct * clamp(impl_tilt_deg / span, -1.0, 1.0)).round_ties_even();
        if step_f.is_nan() {
            // int(round(nan)) raises in Python, which aborts the stream.
            panic!("cannot convert float NaN to integer");
        }
        let mut step = step_f as i64;
        // Freeze the step while the gate fades, or a bucket settling under gravity
        // re-steps mid-fade and reads as still being commanded.
        if gate_target > 0.0 {
            v._impl_tilt_held_step = Some(step);
        } else if let Some(held) = v._impl_tilt_held_step {
            step = held;
        }

        if let Some(prev) = v._impl_tilt_step {
            if step != prev {
                v._impl_tilt_dip_left = IMPL_TILT_STEP_DIP_MS / 1000.0;
            }
        }
        v._impl_tilt_step = Some(step);

        // Linear degrees -> logarithmic frequency, so the steps are evenly perceived.
        let freq_t = IMPL_TILT_LEVEL_HZ * 2.0f64.powf(step as f64 / steps_per_oct);
        let inc_t_target = freq_t / sr;
        let inc_t_prev = if v._impl_tilt_inc > 0.0 {
            v._impl_tilt_inc
        } else {
            inc_t_target
        };
        // One-pole on the increment, NOT a linspace straight to the new frequency: the step
        // has to arrive as a step for the scale to be countable.
        let beta_t = 1.0 - (-dt_impl / IMPL_TILT_GLIDE_TAU).exp();
        let inc_t = inc_t_prev + beta_t * (inc_t_target - inc_t_prev);

        let t_amp = 10.0f64.powf(
            (s._impl_tilt_db
                - IMPL_TILT_SPAN_DB * (1.0 - clamp01(step.abs() as f64 / steps_per_oct)))
                / 20.0,
        );

        // Short notch on each step change, so a fast sweep is countable.
        let mut n_dip = 0usize;
        let dip_gain = 10.0f64.powf(IMPL_TILT_STEP_DIP_DB * -1.0 / 20.0);
        let dipping = v._impl_tilt_dip_left > 0.0;
        if dipping {
            let n = (v._impl_tilt_dip_left * sr).trunc() as i64;
            n_dip = (frames as i64).min(n).max(0) as usize;
            v._impl_tilt_dip_left = py_max(0.0, v._impl_tilt_dip_left - dt_impl);
        }

        let (tl, tr) = (IMPL_TILT_GAIN_L as f32, IMPL_TILT_GAIN_R as f32);
        let mut cum = 0.0f64;
        let mut last = 0.0f64;
        for k in 0..frames {
            cum += linspace_open_at(inc_t_prev, inc_t, frames, k);
            let ph_t = py_mod(v._impl_tilt_phase + cum, 1.0);
            last = ph_t;
            let mut mono = t_amp * (TWO_PI * ph_t).sin();
            if k < n_dip {
                mono *= dip_gain;
            }
            let mono_t = (mono * gate_at(k)) as f32;
            b.buf_l[k] += mono_t * tl;
            b.buf_r[k] += mono_t * tr;
        }
        v._impl_tilt_inc = inc_t;
        if frames > 0 {
            v._impl_tilt_phase = last;
        }
    } else {
        // Idle: reset the glide anchors so the next onset starts from the real value.
        v._impl_ground_inc = 0.0;
        v._impl_ground_ratio = 0.0;
        v._impl_ground_index = 0.0;
        v._impl_tilt_inc = 0.0;
        v._impl_tilt_step = None;
        v._impl_tilt_dip_left = 0.0;
    }
}
