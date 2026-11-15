//! Docking instrument: panned pulse, centred beat pair, lock chime (audio.py:6459-6812).
//!
//! Two voices, three dimensions. The panned pulse carries lateral offset on the stereo
//! image and range on its repetition rate; the beat pair carries the null channel, going
//! to unison at alignment. Phased by range: silent past the mode's max range, pulse only in
//! to its beat range, pair fading in inside that. IMPL mode nulls the implement's height
//! error (metres), RAMP mode the squareness to the ramp axis (degrees); the mode selects
//! constants, never a code path.
//!
//! Writes `v._dock_gate_env`, which scanner, cannon_aim and hydro_steer read next block.
//! The frame applies the dock duck bus around this voice.

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{
    add_tail, clamp, clamp01, convolve_f32_into, keep_tail, linspace_open_at, pan_gains,
    play_centred,
};
use crate::consts::{
    DOCK_AM_ABOVE_HZ, DOCK_AM_BELOW_HZ, DOCK_AM_DEPTH, DOCK_ATTACK_TAU, DOCK_BEAT_DB,
    DOCK_BEAT_HZ_PER_M, DOCK_BEAT_MAX_HZ, DOCK_BEAT_RANGE_M, DOCK_LATERAL_FULL_M,
    DOCK_LOCK_BEAT_HZ, DOCK_LOCK_REARM_BEAT_HZ, DOCK_MAX_AZIMUTH_DEG, DOCK_MAX_RANGE_M,
    DOCK_PULSE_DUTY, DOCK_PULSE_EDGE, DOCK_PULSE_FREQ_HZ, DOCK_PULSE_MAX_HZ, DOCK_PULSE_MIN_HZ,
    DOCK_RAMP_BEAT_HZ_PER_DEG, DOCK_RAMP_BEAT_RANGE_M, DOCK_RAMP_LATERAL_DEADZONE_M,
    DOCK_RAMP_LATERAL_FULL_M, DOCK_RAMP_LOCK_LATERAL_M, DOCK_RAMP_LOCK_YAW_DEG,
    DOCK_RAMP_MAX_RANGE_M, DOCK_RAMP_REARM_LATERAL_M, DOCK_RAMP_REARM_YAW_DEG, DOCK_REF_HZ,
    DOCK_REL_TAU, DOCK_STALE_S, py_min,
};
use crate::controller::Voice;

const PI: f64 = std::f64::consts::PI;
const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let frames = b.frames;
    let sr = b.sr;
    let s = b.snap;
    let dock_on = s.dock_on;
    let dock_have = s.dock_have;
    let dock_range = s.dock_range;
    let dock_lateral = s.dock_lateral;
    let dock_null = s.dock_null;
    let dock_acquire = s.dock_acquire;
    let dock_bearing = s.dock_bearing;
    let dock_stamp = s.dock_stamp;
    let dock_pulse_db = s.dock_pulse_db;

    let dt_dock = frames as f64 / sr;
    let mut dock_produced_audio = false;

    // Everything that differs between the implement and ramp answers, resolved once.
    let dock_ramp = s.dock_mode == "RAMP";
    let dock_max_range = if dock_ramp {
        DOCK_RAMP_MAX_RANGE_M
    } else {
        DOCK_MAX_RANGE_M
    };
    let dock_beat_range = if dock_ramp {
        DOCK_RAMP_BEAT_RANGE_M
    } else {
        DOCK_BEAT_RANGE_M
    };
    let dock_lat_full = if dock_ramp {
        DOCK_RAMP_LATERAL_FULL_M
    } else {
        DOCK_LATERAL_FULL_M
    };
    // The null is vertical metres for an implement and yaw degrees for a ramp.
    let dock_beat_slope = if dock_ramp {
        DOCK_RAMP_BEAT_HZ_PER_DEG
    } else {
        DOCK_BEAT_HZ_PER_M
    };

    // Stale-guard on wall time: the mod stops sending entirely when it goes CLEAR.
    let dock_live = dock_on
        && dock_have
        && dock_range < dock_max_range
        && (b.clock.perf_counter() - dock_stamp) < DOCK_STALE_S;

    let gate_tgt_dock = if dock_live { 1.0 } else { 0.0 };
    let tau_dock = if gate_tgt_dock > v._dock_gate_env {
        DOCK_ATTACK_TAU
    } else {
        DOCK_REL_TAU
    };
    let mut dock_gate_end =
        v._dock_gate_env + (1.0 - (-dt_dock / tau_dock).exp()) * (gate_tgt_dock - v._dock_gate_env);
    if dock_gate_end < 0.01 && gate_tgt_dock == 0.0 {
        dock_gate_end = 0.0; // hard cut; a one-pole never actually reaches zero
    }
    let dock_gate_arr: Vec<f32> = (0..frames)
        .map(|k| linspace_open_at(v._dock_gate_env, dock_gate_end, frames, k) as f32)
        .collect();
    let dock_gate_was = v._dock_gate_env;
    v._dock_gate_env = dock_gate_end;

    // frames > 0 guarded because every phase advance below reads arr[-1].
    if frames > 0 && (dock_gate_end > 0.0 || dock_gate_was > 0.0) {
        // ---- Voice 1: panned pulse (lateral + range) ------------------------------------
        let rng_norm = clamp01(1.0 - (dock_range / dock_max_range));
        let pulse_hz =
            DOCK_PULSE_MIN_HZ + (DOCK_PULSE_MAX_HZ - DOCK_PULSE_MIN_HZ) * rng_norm.powf(2.0);
        let inc_p = DOCK_PULSE_FREQ_HZ / sr;

        // Pulse position is an ACCUMULATED PHASE, never elapsed-time-modulo-period: both of
        // those move, so the remainder teleports around the cycle on every update. Changing
        // the rate changes only how fast the phase advances, glided across the block.
        let rate_prev = if v._dock_pulse_rate > 0.0 {
            v._dock_pulse_rate
        } else {
            pulse_hz
        };
        let amp_p = 10.0f64.powf(dock_pulse_db / 20.0) as f32;
        let edge = DOCK_PULSE_EDGE;
        let mut mono_dock: Vec<f32> = Vec::with_capacity(frames);
        let mut cum_rate = 0.0f64;
        let mut last_ph_p = 0.0f64;
        let mut last_cyc = 0.0f64;
        for k in 0..frames {
            let ph_p = v._dock_pulse_phase + k as f64 * inc_p;
            cum_rate += linspace_open_at(rate_prev, pulse_hz, frames, k);
            let cyc = v._dock_pulse_cycle + cum_rate / sr;
            last_ph_p = ph_p;
            last_cyc = cyc;
            // 0 at the start of a pulse, 1 at its end, >1 through the silent remainder.
            let frac = py_mod(cyc, 1.0) / DOCK_PULSE_DUTY;
            // Raised-cosine skirt rather than a rectangular gate, which would click.
            let mut pulse_gate = clamp(1.0 - frac, 0.0, 1.0);
            if !(frac <= 1.0) {
                pulse_gate = 0.0;
            }
            let rise = clamp(frac / edge, 0.0, 1.0);
            let pulse_gate = (pulse_gate * 0.5 * (1.0 - (PI * rise).cos())) as f32;
            let sine = (TWO_PI * ph_p).sin() as f32;
            mono_dock.push(sine * amp_p * pulse_gate);
        }
        v._dock_pulse_phase = py_mod(last_ph_p + inc_p, 1.0);
        v._dock_pulse_cycle = py_mod(last_cyc, 1.0);
        v._dock_pulse_rate = pulse_hz;

        // ---- Voice 2: the null channel --------------------------------------------------
        // Its own envelope rather than the shared gate, and held out entirely while
        // hunting for a ramp mouth: from beside or behind the machine there is no line to
        // become square with, and the pair arriving is the "now you are lined up" cue.
        let beat_tgt = if dock_live && !dock_acquire && dock_range < dock_beat_range {
            1.0
        } else {
            0.0
        };
        let tau_b = if beat_tgt > v._dock_beat_env {
            DOCK_ATTACK_TAU
        } else {
            DOCK_REL_TAU
        };
        let mut beat_end =
            v._dock_beat_env + (1.0 - (-dt_dock / tau_b).exp()) * (beat_tgt - v._dock_beat_env);
        if beat_end < 0.01 && beat_tgt == 0.0 {
            beat_end = 0.0;
        }
        let beat_arr: Vec<f32> = (0..frames)
            .map(|k| linspace_open_at(v._dock_beat_env, beat_end, frames, k) as f32)
            .collect();
        v._dock_beat_env = beat_end;

        // Beat rate straight from the null error, scaled by the mode's slope. The pair sits
        // symmetrically about DOCK_REF_HZ so the centre pitch never moves.
        let beat_hz = py_min(dock_null.abs() * dock_beat_slope, DOCK_BEAT_MAX_HZ);
        let beat_max = beat_arr
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, |a, x| if x > a || x.is_nan() { x } else { a });
        let mut pair: Option<Vec<f32>> = None;
        if beat_end > 0.0 || beat_max > 0.0 {
            let inc_ref_tgt = (DOCK_REF_HZ - beat_hz * 0.5) / sr;
            let inc_mov_tgt = (DOCK_REF_HZ + beat_hz * 0.5) / sr;
            if v._dock_mov_inc <= 0.0 {
                v._dock_ref_inc = inc_ref_tgt;
                v._dock_mov_inc = inc_mov_tgt;
            }

            // Sign carrier: beating is signless, so the direction rides on the tremolo
            // RATE, suppressed near the null. In ramp mode yaw is positive-LEFT and FAST is
            // selected on a NEGATIVE value, so fast means turn right.
            let am_hz = if dock_null < 0.0 {
                DOCK_AM_ABOVE_HZ
            } else {
                DOCK_AM_BELOW_HZ
            };
            let tremolo = beat_hz > DOCK_LOCK_BEAT_HZ;
            let inc_am = am_hz / sr;
            if !tremolo {
                v._dock_am_phase = 0.0;
            }

            let amp_b = 10.0f64.powf(DOCK_BEAT_DB / 20.0);
            let amp_b_half = (amp_b * 0.5) as f32;
            let mut out: Vec<f32> = Vec::with_capacity(frames);
            let mut cum_ref = 0.0f64;
            let mut cum_mov = 0.0f64;
            let mut last_ref = 0.0f64;
            let mut last_mov = 0.0f64;
            for k in 0..frames {
                cum_ref += linspace_open_at(v._dock_ref_inc, inc_ref_tgt, frames, k);
                cum_mov += linspace_open_at(v._dock_mov_inc, inc_mov_tgt, frames, k);
                let ph_ref = v._dock_ref_phase + cum_ref;
                let ph_mov = v._dock_mov_phase + cum_mov;
                last_ref = ph_ref;
                last_mov = ph_mov;
                let am: f32 = if tremolo {
                    let ph_am = v._dock_am_phase + k as f64 * inc_am;
                    (1.0 - DOCK_AM_DEPTH + DOCK_AM_DEPTH * (0.5 + 0.5 * (TWO_PI * ph_am).cos()))
                        as f32
                } else {
                    1.0
                };
                // The tremolo scales the SUM, leaving the beat itself untouched.
                let sum = ((TWO_PI * ph_ref).sin() + (TWO_PI * ph_mov).sin()) * am as f64;
                out.push((sum as f32) * amp_b_half * beat_arr[k] * dock_gate_arr[k]);
            }
            if tremolo {
                let ph_am_last = v._dock_am_phase + (frames - 1) as f64 * inc_am;
                v._dock_am_phase = py_mod(ph_am_last + inc_am, 1.0);
            }
            pair = Some(out);

            v._dock_ref_phase = py_mod(last_ref, 1.0);
            v._dock_mov_phase = py_mod(last_mov, 1.0);
            v._dock_ref_inc = inc_ref_tgt;
            v._dock_mov_inc = inc_mov_tgt;
        } else {
            v._dock_ref_inc = 0.0;
            v._dock_mov_inc = 0.0;
        }

        for (m, g) in mono_dock.iter_mut().zip(dock_gate_arr.iter()) {
            *m *= *g;
        }

        // ---- Lateral -> HRTF azimuth ----------------------------------------------------
        let lat_norm;
        let hrtf_az_dock;
        if dock_acquire {
            // Hunting: the pulse sits at the mouth's TRUE bearing over the whole circle, not
            // squeezed into DOCK_MAX_AZIMUTH_DEG; the pan fallback takes the sine.
            lat_norm = clamp(dock_bearing.to_radians().sin(), -1.0, 1.0);
            hrtf_az_dock = py_mod(dock_bearing, 360.0);
        } else {
            let lat_abs = dock_lateral.abs();
            lat_norm = if dock_ramp && lat_abs <= DOCK_RAMP_LATERAL_DEADZONE_M {
                0.0
            } else if dock_ramp {
                let usable = dock_lat_full - DOCK_RAMP_LATERAL_DEADZONE_M;
                clamp((lat_abs - DOCK_RAMP_LATERAL_DEADZONE_M) / usable, 0.0, 1.0)
                    .copysign(dock_lateral)
            } else {
                clamp(dock_lateral / dock_lat_full, -1.0, 1.0)
            };
            // Game bearing is positive-LEFT; the in-callback HRTF convention takes the raw
            // bearing mod 360.
            hrtf_az_dock = py_mod(DOCK_MAX_AZIMUTH_DEG * lat_norm, 360.0);
        }
        let irs = b.snap.hrtf().and_then(|h| h.get_hrir(hrtf_az_dock));
        if let Some((ir_l, ir_r)) = irs {
            let mut scratch = Vec::new();
            let mut conv_l = Vec::new();
            let mut conv_r = Vec::new();
            convolve_f32_into(&mono_dock, &ir_l, &mut scratch, &mut conv_l);
            convolve_f32_into(&mono_dock, &ir_r, &mut scratch, &mut conv_r);
            if v._dock_overlap_L.is_some() {
                let ol = v
                    ._dock_overlap_L
                    .as_ref()
                    .map_or(0, |t| t.len())
                    .min(conv_l.len());
                add_tail(&mut conv_l[..ol], &v._dock_overlap_L);
                add_tail(&mut conv_r[..ol], &v._dock_overlap_R);
            }
            for k in 0..frames.min(conv_l.len()) {
                b.buf_l[k] += conv_l[k] as f32;
                b.buf_r[k] += conv_r[k] as f32;
            }
            keep_tail(&conv_l, frames, &mut v._dock_overlap_L);
            keep_tail(&conv_r, frames, &mut v._dock_overlap_R);
            dock_produced_audio = true;
        } else {
            let (lg, rg) = pan_gains(-lat_norm);
            let (lg, rg) = (lg as f32, rg as f32);
            for k in 0..frames {
                b.buf_l[k] += mono_dock[k] * lg;
                b.buf_r[k] += mono_dock[k] * rg;
            }
        }

        // ---- The pair is rendered CENTRE, deliberately not with the pulse --------------
        // Two sounds from one point fuse into one object; the null is not directional.
        if let Some(pair) = pair.as_ref() {
            for k in 0..frames {
                b.buf_l[k] += pair[k];
                b.buf_r[k] += pair[k];
            }
        }

        // ---- Alignment lock earcon -----------------------------------------------------
        // Armed latch inside the callback, like the articulation centre click: hysteresis
        // on the mode's alignment axes, so it cannot double-fire while hunting.
        let lock_wave = b.waves.DOCK_LOCK_WAVEFORM.is_some();
        if dock_ramp {
            // Acquisition has no alignment state; clearing here also stops a centred
            // corridor entry producing a second cue after the handoff pip.
            if dock_acquire {
                v._dock_lock_armed = false;
            } else if dock_lateral.abs() >= DOCK_RAMP_REARM_LATERAL_M
                || dock_null.abs() >= DOCK_RAMP_REARM_YAW_DEG
            {
                v._dock_lock_armed = true;
            } else if v._dock_lock_armed
                && dock_lateral.abs() <= DOCK_RAMP_LOCK_LATERAL_M
                && dock_null.abs() <= DOCK_RAMP_LOCK_YAW_DEG
                && beat_end > 0.5
            {
                v._dock_lock_armed = false;
                if lock_wave {
                    v._dock_lock_pos = 0.0;
                }
            }
        } else if beat_hz > DOCK_LOCK_REARM_BEAT_HZ {
            v._dock_lock_armed = true;
        } else if v._dock_lock_armed && beat_hz <= DOCK_LOCK_BEAT_HZ && beat_end > 0.5 {
            v._dock_lock_armed = false;
            if lock_wave {
                v._dock_lock_pos = 0.0;
            }
        }
    } else {
        // Idle: reset the glide anchors so re-entry attacks cleanly.
        v._dock_beat_env = 0.0;
        v._dock_mov_inc = 0.0;
        v._dock_pulse_cycle = 0.0;
        v._dock_pulse_rate = 0.0;
        v._dock_lock_armed = false;
    }

    if !dock_produced_audio {
        // Drain the HRTF tail, or the last convolution hangs on the output forever.
        if let Some(tl) = v._dock_overlap_L.as_ref() {
            if !tl.is_empty() {
                let ol = tl.len().min(frames);
                for k in 0..ol {
                    b.buf_l[k] += tl[k] as f32;
                }
                if let Some(tr) = v._dock_overlap_R.as_ref() {
                    for k in 0..ol.min(tr.len()) {
                        b.buf_r[k] += tr[k] as f32;
                    }
                }
            }
        }
        v._dock_overlap_L = None;
        v._dock_overlap_R = None;
    }

    if v._dock_lock_pos >= 0.0 {
        if let Some(wf) = b.waves.DOCK_LOCK_WAVEFORM.as_ref() {
            play_centred(b.buf_l, b.buf_r, wf, &mut v._dock_lock_pos, frames);
        }
    }
}
