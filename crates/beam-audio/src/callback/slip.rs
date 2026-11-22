//! Wheel slip (audio.py:5845-5964): a continuous amplitude-modulated wavetable tone whose
//! carrier says which way the wheels diverge from the ground (low for lockup, high for
//! wheelspin) and whose pitch rises with the slip ratio, plus a band-limited noise for a
//! side slide. Both fade in and out, and the mix is placed toward the direction of travel
//! (+LEFT), HRTF-convolved when available. The tables come from `_slip_voice_tables`
//! (3408-3449), cached per integer sample rate.

use std::f64::consts::PI;
use std::sync::Arc;

use beam_base::pyfmt::py_mod;

use crate::callback::block::Block;
use crate::callback::common::{
    add_conv_f32, add_tail, clamp, convolve_f32_into, convolve_full_into, keep_tail,
    linspace_open_at, pan_gains,
};
use crate::consts::{
    SLIDE_AM_DEPTH, SLIDE_AM_HZ, SLIDE_DEG_FULL, SLIDE_DEG_MIN, SLIP_AM_DEPTH, SLIP_AM_HZ,
    SLIP_AZ_TAU_S, SLIP_FADE_ATTACK_S, SLIP_FADE_DECAY_S, SLIP_LOCK_HZ, SLIP_MAG_FOR_FULL,
    SLIP_PITCH_RISE_OCT, SLIP_PITCH_TAU_S, SLIP_SPIN_HZ, SLIP_TABLE_LEN, py_max,
};
use crate::controller::{AudioController, Voice};

/// `(spin_table, lock_table, noise_lo, noise_hi)`.
pub type SlipTables = Arc<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)>;

/// `_slip_voice_tables()`: rebuilt only when the integer sample rate changes (cached in
/// `v._slip_tables` / `v._slip_tables_sr`).
pub fn slip_voice_tables(v: &mut Voice, sr: f64) -> SlipTables {
    let sr_i = sr.trunc() as i64;
    if let (Some(t), Some(cached)) = (&v._slip_tables, v._slip_tables_sr) {
        if cached == sr_i {
            return Arc::clone(t);
        }
    }
    let tables = Arc::new(crate::waveforms::slip_voice_tables(sr));
    v._slip_tables = Some(Arc::clone(&tables));
    v._slip_tables_sr = Some(sr_i);
    tables
}

#[allow(clippy::needless_range_loop)]
pub fn render(b: &mut Block<'_>, v: &mut Voice) {
    let s = b.snap;
    let frames = b.frames;
    let sr = b.sr;
    let dt_ls = frames as f64 / sr;

    if s.slip_active {
        let alpha_att = 1.0 - (-dt_ls / py_max(1e-6, SLIP_FADE_ATTACK_S)).exp();
        v._slip_fade_gain += alpha_att * (1.0 - v._slip_fade_gain);
    } else {
        let alpha_dec = 1.0 - (-dt_ls / py_max(1e-6, SLIP_FADE_DECAY_S)).exp();
        v._slip_fade_gain += alpha_dec * (0.0 - v._slip_fade_gain);
    }

    // Side slide: its own fade, so a power drift layers it with the spin voice.
    if s.slide_active {
        let alpha_att = 1.0 - (-dt_ls / py_max(1e-6, SLIP_FADE_ATTACK_S)).exp();
        v._slide_fade_gain += alpha_att * (1.0 - v._slide_fade_gain);
    } else {
        let alpha_dec = 1.0 - (-dt_ls / py_max(1e-6, SLIP_FADE_DECAY_S)).exp();
        v._slide_fade_gain += alpha_dec * (0.0 - v._slide_fade_gain);
    }

    // Placement: toward the direction of travel, +LEFT. Smoothed as a unit vector, so a
    // slide swinging through dead astern stays behind.
    let az_beta = 1.0 - (-dt_ls / py_max(1e-6, SLIP_AZ_TAU_S)).exp();
    let az_rad = s.slip_azimuth_deg.to_radians();
    v._slip_az_x += az_beta * (az_rad.cos() - v._slip_az_x);
    v._slip_az_y += az_beta * (az_rad.sin() - v._slip_az_y);

    if !(v._slip_fade_gain > 0.001 || v._slide_fade_gain > 0.001) {
        v._slip_overlap_L = None;
        v._slip_overlap_R = None;
        return;
    }

    let tables = slip_voice_tables(v, sr);
    let (spin_tab, lock_tab, noise_lo, noise_hi) = (&tables.0, &tables.1, &tables.2, &tables.3);
    let amp = 10.0f64.powf(s._slip_amp_db / 20.0);
    let amp32 = amp as f32;
    let mut mono_slip = vec![0.0f32; frames];
    let two_pi = 2.0 * PI;
    let table_len = SLIP_TABLE_LEN as usize;

    if v._slip_fade_gain > 0.001 {
        let spin = s.slip_kind > 0;
        let base_hz = if spin { SLIP_SPIN_HZ } else { SLIP_LOCK_HZ };
        let table = if spin { spin_tab } else { lock_tab };
        // Pitch from the slip ratio. Held while fading out (the tail should not sweep
        // down), and snapped rather than glided when the voice changes kind.
        let target = clamp(s.slip_ratio, 0.0, 1.0);
        let mut prev = v._slip_ratio_sm;
        if s.slip_kind != v._slip_pitch_kind {
            v._slip_pitch_kind = s.slip_kind;
            prev = target;
        }
        if s.slip_active {
            let beta = 1.0 - (-dt_ls / py_max(1e-6, SLIP_PITCH_TAU_S)).exp();
            v._slip_ratio_sm = prev + beta * (target - prev);
        } else {
            v._slip_ratio_sm = prev;
        }
        let inc0 = base_hz * 2.0f64.powf(SLIP_PITCH_RISE_OCT * prev) / sr;
        let inc1 = base_hz * 2.0f64.powf(SLIP_PITCH_RISE_OCT * v._slip_ratio_sm) / sr;
        // Ramp the phase increment across the block: continuous in phase AND frequency.
        let incs: Vec<f64> = (0..frames)
            .map(|k| linspace_open_at(inc0, inc1, frames, k))
            .collect();
        let phase0 = v._phase_slip;
        v._phase_slip = py_mod(phase0 + crate::dsp::sum(&incs), 1.0);

        let am_inc = SLIP_AM_HZ / sr;
        let mag_norm = clamp(s.slip_mag / SLIP_MAG_FOR_FULL, 0.0, 1.0);
        let level = (0.35 + 0.65 * mag_norm) as f32;
        let fade = v._slip_fade_gain as f32;
        let mut cum = 0.0;
        let mut am_last = 0.0;
        for k in 0..frames {
            cum += incs[k];
            let t = py_mod(phase0 + cum - incs[k], 1.0);
            let idx = t * SLIP_TABLE_LEN as f64;
            let i0 = (idx.trunc() as i64).rem_euclid(SLIP_TABLE_LEN) as usize;
            let i1 = (i0 + 1) % table_len;
            let fr = (idx - idx.floor()) as f32;
            let mut tone = table[i0] + (table[i1] - table[i0]) * fr;
            let am_t = py_mod(k as f64 * am_inc + v._phase_slip_am, 1.0);
            tone =
                (tone as f64 * (1.0 - SLIP_AM_DEPTH * (0.5 - 0.5 * (two_pi * am_t).cos()))) as f32;
            mono_slip[k] += tone * amp32 * level * fade;
            am_last = am_t;
        }
        // Unguarded in Python: `am_t[-1]` raises on a zero-frame block, aborting the stream.
        assert!(
            frames > 0,
            "index -1 is out of bounds for axis 0 with size 0"
        );
        v._phase_slip_am = py_mod(am_last + am_inc, 1.0);
    }

    if v._slide_fade_gain > 0.001 {
        let n = noise_lo.len();
        let pos = v._slide_noise_pos;
        v._slide_noise_pos = (pos + frames) % n;
        let deg_norm = clamp(
            (s.slide_deg - SLIDE_DEG_MIN) / (SLIDE_DEG_FULL - SLIDE_DEG_MIN),
            0.0,
            1.0,
        );
        let w_lo = (1.0 - deg_norm) as f32;
        let w_hi = deg_norm as f32;
        let am_inc = SLIDE_AM_HZ / sr;
        let level = (0.35 + 0.65 * deg_norm) as f32;
        let fade = v._slide_fade_gain as f32;
        let mut am_last = 0.0;
        for k in 0..frames {
            let id = (pos + k) % n;
            let mut noise = noise_lo[id] * w_lo + noise_hi[id] * w_hi;
            let am_t = py_mod(k as f64 * am_inc + v._phase_slide_am, 1.0);
            noise = (noise as f64 * (1.0 - SLIDE_AM_DEPTH * (0.5 - 0.5 * (two_pi * am_t).cos())))
                as f32;
            mono_slip[k] += noise * amp32 * level * fade;
            am_last = am_t;
        }
        assert!(
            frames > 0,
            "index -1 is out of bounds for axis 0 with size 0"
        );
        v._phase_slide_am = py_mod(am_last + am_inc, 1.0);
    }

    let az_deg = v._slip_az_y.atan2(v._slip_az_x).to_degrees();
    let mut placed = false;
    if let Some(h) = s.hrtf() {
        // In-callback convention: the raw +LEFT bearing mod 360, as the dock tone.
        let hrtf_az_slip = py_mod(az_deg, 360.0);
        if let Some((ir_l, ir_r)) = h.get_hrir(hrtf_az_slip) {
            let gain =
                AudioController::hrtf_emphasis_gain_for(s._hrtf_front_emphasis_db, hrtf_az_slip)
                    * s._hrtf_distance_gain;
            let mut scratch = Vec::new();
            let mut conv_l = Vec::new();
            let mut conv_r = Vec::new();
            convolve_f32_into(&mono_slip, &ir_l, &mut scratch, &mut conv_l);
            convolve_full_into(&scratch, &ir_r, &mut conv_r);
            for x in conv_l.iter_mut() {
                *x *= gain;
            }
            for x in conv_r.iter_mut() {
                *x *= gain;
            }
            add_tail(&mut conv_l, &v._slip_overlap_L);
            add_tail(&mut conv_r, &v._slip_overlap_R);
            add_conv_f32(b.buf_l, &conv_l, frames);
            add_conv_f32(b.buf_r, &conv_r, frames);
            keep_tail(&conv_l, frames, &mut v._slip_overlap_L);
            keep_tail(&conv_r, frames, &mut v._slip_overlap_R);
            placed = true;
        }
    }
    if !placed {
        // The stereo fallback has no back hemisphere; the sine keeps the side right.
        let (lg, rg) = pan_gains(-az_deg.to_radians().sin());
        let (lg, rg) = (lg as f32, rg as f32);
        for k in 0..frames {
            b.buf_l[k] += mono_slip[k] * lg;
            b.buf_r[k] += mono_slip[k] * rg;
        }
    }
}
