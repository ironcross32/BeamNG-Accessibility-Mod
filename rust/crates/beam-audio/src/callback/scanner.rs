//! Vehicle scanner (audio.py:4926-5177): bearing EMA, beep train (bright when aligned),
//! steering-locked steady tone, HRTF, scanner diagnostic capture. A2.
//!
//! `smooth_bearing` (4926-4947) runs BEFORE the frame copies the scanner duck-bus mix;
//! `render` (4949-5177) runs inside it. Gated on the PREVIOUS block's `v._dock_gate_env`
//! and on `coupler_active`. The smoothed bearing replaces `scan_bearing` for this voice
//! (Python rebinds the local); `v._sm_scan_bearing` carries it.

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{add_tail, convolve_f32_into, pan_gains};
use crate::consts::*;
use crate::controller::Voice;

/// The bearing every scanner cue derives from this block: the EMA when it ran, else the
/// raw snapshot value (Python's rebound local `scan_bearing`).
fn scan_bearing(b: &Block<'_>, v: &Voice) -> f64 {
    let s = b.snap;
    if s.scan_active && s.scan_bearing.is_finite() {
        v._sm_scan_bearing.unwrap_or(s.scan_bearing)
    } else {
        s.scan_bearing
    }
}

/// 4926-4947. Smoothed in SHORTEST-ANGULAR-DISTANCE form so +179 then -179 is a two-degree
/// move, not a sweep through the rear. The accumulator is re-wrapped after integrating:
/// turning steadily one way would otherwise walk it off to -400, -760..., and the pitch
/// (which scales on abs(bearing)) would sink below what a target dead astern produces.
pub fn smooth_bearing(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let scan_bearing = s.scan_bearing;
    if s.scan_active && scan_bearing.is_finite() {
        match v._sm_scan_bearing {
            None => v._sm_scan_bearing = Some(scan_bearing),
            Some(sm) => {
                let delta = py_mod(scan_bearing - sm + 180.0, 360.0) - 180.0;
                let beta_sb = 1.0 - (-(b.frames as f64 / b.sr) / SCANNER_BEARING_TAU).exp();
                v._sm_scan_bearing = Some(py_mod(sm + beta_sb * delta + 180.0, 360.0) - 180.0);
            }
        }
    } else {
        v._sm_scan_bearing = None;
    }
}

/// 4957-5177: beeps, steady tone, else-branch reset. Suppressed while coupler tracking or
/// the docking instrument (previous block's gate) owns the same target.
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let sr = b.sr;
    let scan_bearing = scan_bearing(b, v);
    let scan_dist = s.scan_dist;

    let mut scanner_produced_audio = false;
    if !(s.scan_active
        && scan_dist != f64::INFINITY
        && !s.coupler_active
        && v._dock_gate_env <= 0.0)
    {
        // Scanner inactive (or coupler tracking): clear all tails and relax the morph level
        // so the next activation starts cleanly.
        v._scanner_overlap_L = None;
        v._scanner_overlap_R = None;
        v._scan_tone_overlap_L = None;
        v._scan_tone_overlap_R = None;
        v._scan_tone_level = 0.0;
        return;
    }

    // Beep rate from distance; the half-distance scales with CLOSING speed, floored so the
    // exp() denominator stays positive when receding fast.
    let half_dist = py_max(
        SCANNER_HALF_DIST_MIN_M,
        SCANNER_HALF_DIST_BASE_M + s.scan_closing_ms * SCANNER_HALF_DIST_PER_MS,
    );
    let rate_norm = (-scan_dist / half_dist).exp();
    let rate_hz = SCANNER_MIN_RATE_HZ + (SCANNER_MAX_RATE_HZ - SCANNER_MIN_RATE_HZ) * rate_norm;
    let interval_sec = 1.0 / rate_hz;

    // Alignment is horizontal target alignment in every vehicle; the cannon has its own
    // centred pulse.
    let aligned = scan_bearing.abs() <= SCANNER_ALIGN_THRESHOLD_DEG;
    let pitch_norm = 1.0 - (scan_bearing.abs() / 180.0);
    let align_norm = (-scan_bearing.abs() / SCANNER_ALIGN_PITCH_DECAY_DEG).exp();
    let proximity =
        SCANNER_ALIGN_WEIGHT * align_norm + (1.0 - SCANNER_ALIGN_WEIGHT) * pitch_norm;
    let scan_freq = s.scan_base_freq * 2.0f64.powf(s.scan_offset_oct * proximity);
    let pitch_mult = scan_freq / SCANNER_BEEP_FREQ_HZ;
    let hrtf_az_scan = py_mod(scan_bearing, 360.0);

    // Steering-locked morph: 0 = pure beeps, 1 = steady tone, smoothed per block.
    let steer_mag = if s.scan_steer_tone_enabled {
        s.v_steer.abs()
    } else {
        0.0
    };
    let tone_target = (steer_mag - SCANNER_STEER_TONE_DEADZONE)
        / py_max(1e-6, SCANNER_STEER_TONE_FULL - SCANNER_STEER_TONE_DEADZONE);
    let tone_target = py_min(1.0, py_max(0.0, tone_target)).powf(SCANNER_STEER_TONE_CURVE);
    v._scan_tone_level += SCANNER_TONE_LEVEL_SMOOTH * (tone_target - v._scan_tone_level);
    let tone_level = v._scan_tone_level;
    let beep_gain = 1.0 - tone_level;

    v._scanner_beep_timer += frames as f64 / sr;
    if v._scanner_beep_timer >= interval_sec {
        v._scanner_beep_timer = 0.0;
        v._scanner_playback_pos = 0.0;
    }

    let hrtf = s.hrtf();
    if v._scanner_playback_pos >= 0.0 {
        if let Some(beep) = b.waves.SCANNER_BEEP_WAVEFORM.as_ref() {
            let active_wf: &[f32] = match (aligned, b.waves.SCANNER_ALIGNED_WAVEFORM.as_ref()) {
                (true, Some(w)) => w,
                _ => beep,
            };
            let beep_len = active_wf.len();
            let limit = beep_len as f64 - 1.0;
            let pos = v._scanner_playback_pos;

            // indices = pos + arange(frames) * pitch_mult; valid = indices < len - 1.
            // beep_segment is float64; mono_scan holds it rounded to float32.
            let mut valid = vec![false; frames];
            let mut segment = vec![0.0f64; frames];
            let mut mono_scan = vec![0.0f32; frames];
            let mut any_valid = false;
            for k in 0..frames {
                let idx = pos + k as f64 * pitch_mult;
                if idx < limit {
                    let i = idx.trunc() as i64;
                    if i < 0 {
                        continue;
                    }
                    let i = i as usize;
                    let fract = idx - i as f64;
                    let s1 = active_wf[i];
                    let s2 = active_wf[i + 1];
                    let seg = s1 as f64 + (s2 - s1) as f64 * fract;
                    valid[k] = true;
                    segment[k] = seg;
                    mono_scan[k] = seg as f32;
                    any_valid = true;
                }
            }

            if any_valid {
                let diag = v._scanner_diag.clone();
                let diag_on = diag.as_ref().is_some_and(|d| d.enabled());
                let mut d_hrtf_used = false;
                let mut d_ir: Option<(Vec<f64>, Vec<f64>)> = None;
                let mut d_out: Option<(Vec<f32>, Vec<f32>)> = None;

                let ir = hrtf.and_then(|h| h.get_hrir(hrtf_az_scan));
                if let Some((ir_l, ir_r)) = ir {
                    let mut scratch = Vec::new();
                    let mut conv_l = Vec::new();
                    let mut conv_r = Vec::new();
                    convolve_f32_into(&mono_scan, &ir_l, &mut scratch, &mut conv_l);
                    convolve_f32_into(&mono_scan, &ir_r, &mut scratch, &mut conv_r);
                    if v._scanner_overlap_L.is_some() {
                        add_tail(&mut conv_l, &v._scanner_overlap_L);
                        add_tail_len_of(&mut conv_r, &v._scanner_overlap_R, &v._scanner_overlap_L);
                    }
                    for k in 0..frames.min(conv_l.len()) {
                        b.buf_l[k] += (beep_gain * conv_l[k]) as f32;
                        b.buf_r[k] += (beep_gain * conv_r[k]) as f32;
                    }
                    v._scanner_overlap_L = Some(tail_of(&conv_l, frames));
                    v._scanner_overlap_R = Some(tail_of(&conv_r, frames));
                    scanner_produced_audio = true;
                    if diag_on {
                        d_hrtf_used = true;
                        // Python records the float64 post-overlap samples; the recorder
                        // takes float32 (its RMS is a diagnostic, not audio).
                        d_out = Some((
                            conv_l[..frames.min(conv_l.len())]
                                .iter()
                                .map(|&x| x as f32)
                                .collect(),
                            conv_r[..frames.min(conv_r.len())]
                                .iter()
                                .map(|&x| x as f32)
                                .collect(),
                        ));
                        d_ir = Some((ir_l, ir_r));
                    }
                } else {
                    // No HRTF (or no IR): proportional stereo pan, negated for direction.
                    let pan_pos = -scan_bearing / 90.0;
                    let (lg, rg) = pan_gains(pan_pos);
                    for k in 0..frames {
                        if valid[k] {
                            b.buf_l[k] = (b.buf_l[k] as f64 + beep_gain * segment[k] * lg) as f32;
                            b.buf_r[k] = (b.buf_r[k] as f64 + beep_gain * segment[k] * rg) as f32;
                        }
                    }
                    if diag_on {
                        let mut ol = Vec::new();
                        let mut or = Vec::new();
                        for k in 0..frames {
                            if valid[k] {
                                ol.push((segment[k] * lg) as f32);
                                or.push((segment[k] * rg) as f32);
                            }
                        }
                        d_out = Some((ol, or));
                    }
                }

                if let (Some(d), true) = (diag.as_ref(), diag_on) {
                    let (out_l, out_r) = d_out.unwrap_or_default();
                    d.record_beep(
                        scan_bearing,
                        scan_dist,
                        aligned,
                        pitch_norm,
                        pitch_mult,
                        d_hrtf_used,
                        hrtf_az_scan,
                        d_ir.as_ref().map(|(l, r)| (l.as_slice(), r.as_slice())),
                        &out_l,
                        &out_r,
                    );
                }
            }

            let next_pos = v._scanner_playback_pos + frames as f64 * pitch_mult;
            v._scanner_playback_pos = if next_pos < limit { next_pos } else { -1.0 };
        }
    }

    // Drain the beep's HRTF tail (crossfaded) when no new beep audio played this block.
    if !scanner_produced_audio {
        if let Some(tl) = v._scanner_overlap_L.take() {
            let ol = tl.len().min(frames);
            if ol > 0 {
                for k in 0..ol {
                    b.buf_l[k] += (beep_gain * tl[k]) as f32;
                }
                if let Some(tr) = v._scanner_overlap_R.as_ref() {
                    for k in 0..ol.min(tr.len()) {
                        b.buf_r[k] += (beep_gain * tr[k]) as f32;
                    }
                }
            }
            v._scanner_overlap_R = None;
        }
    }

    // Steering-locked steady tone: a continuous, HRTF-positioned triangle on the beeps'
    // bearing->pitch map, filling the gaps between slow beeps while a turn is held.
    if tone_level > 1e-3 {
        let inc = scan_freq / sr;
        let gain = SCANNER_TONE_AMP * tone_level;
        let mono_tone: Vec<f32> = (0..frames)
            .map(|k| {
                let ph = v._scan_tone_phase + inc * k as f64;
                let tri = (2.0 / std::f64::consts::PI)
                    * (2.0 * std::f64::consts::PI * ph).sin().asin();
                (tri * gain) as f32
            })
            .collect();
        v._scan_tone_phase = py_mod(v._scan_tone_phase + inc * frames as f64, 1.0);

        let mut used_hrtf_tone = false;
        if let Some((ir_l, ir_r)) = hrtf.and_then(|h| h.get_hrir(hrtf_az_scan)) {
            let mut scratch = Vec::new();
            let mut tl = Vec::new();
            let mut tr = Vec::new();
            convolve_f32_into(&mono_tone, &ir_l, &mut scratch, &mut tl);
            convolve_f32_into(&mono_tone, &ir_r, &mut scratch, &mut tr);
            if v._scan_tone_overlap_L.is_some() {
                add_tail(&mut tl, &v._scan_tone_overlap_L);
                add_tail_len_of(&mut tr, &v._scan_tone_overlap_R, &v._scan_tone_overlap_L);
            }
            for k in 0..frames.min(tl.len()) {
                b.buf_l[k] += tl[k] as f32;
                b.buf_r[k] += tr[k] as f32;
            }
            v._scan_tone_overlap_L = Some(tail_of(&tl, frames));
            v._scan_tone_overlap_R = Some(tail_of(&tr, frames));
            used_hrtf_tone = true;
        }
        if !used_hrtf_tone {
            let pan_pos = -scan_bearing / 90.0;
            let (lg, rg) = pan_gains(pan_pos);
            // float32 array times a Python float: float32 arithmetic.
            let (lg, rg) = (lg as f32, rg as f32);
            for k in 0..frames {
                b.buf_l[k] += mono_tone[k] * lg;
                b.buf_r[k] += mono_tone[k] * rg;
            }
            v._scan_tone_overlap_L = None;
            v._scan_tone_overlap_R = None;
        }
    } else if let Some(tl) = v._scan_tone_overlap_L.take() {
        // Drain/clear the steady tone's HRTF tail as it fades out.
        let ol = tl.len().min(frames);
        if ol > 0 {
            for k in 0..ol {
                b.buf_l[k] += tl[k] as f32;
            }
            if let Some(tr) = v._scan_tone_overlap_R.as_ref() {
                for k in 0..ol.min(tr.len()) {
                    b.buf_r[k] += tr[k] as f32;
                }
            }
        }
        v._scan_tone_overlap_R = None;
    }
}

/// `conv_r[:ol] += tail_r[:ol]` where Python took `ol` from the LEFT tail's length
/// (`ol = min(len(overlap_L), len(conv_l))`). Both tails always have equal length.
fn add_tail_len_of(conv: &mut [f64], tail: &crate::callback::common::Tail, len_from: &crate::callback::common::Tail) {
    if let (Some(t), Some(lf)) = (tail, len_from) {
        let ol = lf.len().min(conv.len()).min(t.len());
        for k in 0..ol {
            conv[k] += t[k];
        }
    }
}

/// `conv[frames:].copy()`.
fn tail_of(conv: &[f64], frames: usize) -> Vec<f64> {
    if frames < conv.len() {
        conv[frames..].to_vec()
    } else {
        Vec::new()
    }
}
