//! Every `_generate_*` waveform of AudioController (audio.py), plus the maths of
//! `_slip_voice_tables`, as free functions of exactly what each reads: the sample rate
//! and, where Python reads one, a level attribute or frequency.
//!
//! Every buffer is built in float64 and returned as float32 (`.astype(np.float32)`),
//! keeping numpy's evaluation order so the products round as Python's do. The two noise
//! sources (the articulation centre click and the slide noise loops) draw from numpy's
//! own `default_rng` stream, reproduced in [`crate::dsp`], so they match Python too.

use std::f64::consts::PI;

use crate::consts::*;
use crate::dsp::{self, Complex64};

const TWO_PI: f64 = 2.0 * PI;

/// `np.linspace(0, dur_s, n, endpoint=False)`.
fn lin_t(dur_s: f64, n: usize) -> Vec<f64> {
    dsp::linspace(0.0, dur_s, n, false)
}

/// `np.arange(n) / sr`.
fn arange_t(sr: f64, n: usize) -> Vec<f64> {
    dsp::arange_div(n, sr)
}

/// `int(sr * ms / 1000.0)`, for a non-negative duration.
fn ms_samples(sr: f64, ms: f64) -> usize {
    (sr * ms / 1000.0) as usize
}

/// `wave[-n:] *= np.linspace(a, b, n)` (callers guard `n > 0`).
fn mul_tail(w: &mut [f64], n: usize, a: f64, b: f64) {
    let len = w.len();
    for (v, g) in w[len - n..].iter_mut().zip(dsp::linspace(a, b, n, true)) {
        *v *= g;
    }
}

/// `wave[:n] *= np.linspace(a, b, n)`.
fn mul_head(w: &mut [f64], n: usize, a: f64, b: f64) {
    for (v, g) in w[..n].iter_mut().zip(dsp::linspace(a, b, n, true)) {
        *v *= g;
    }
}

/// `wave /= np.max(np.abs(wave))`, unguarded as in the callers that do not check.
fn div_peak(w: &mut [f64]) {
    let p = dsp::max_abs(w);
    for v in w.iter_mut() {
        *v /= p;
    }
}

/// `peak = float(np.max(np.abs(wave))); if peak > 0: wave /= peak`.
fn div_peak_guarded(w: &mut [f64]) {
    let p = dsp::max_abs(w);
    if p > 0.0 {
        for v in w.iter_mut() {
            *v /= p;
        }
    }
}

fn scaled_f32(w: &[f64], amp: f64) -> Vec<f32> {
    w.iter().map(|&v| (v * amp) as f32).collect()
}

/// The shared exponential decay of the compass-family clicks.
fn click_decay(t: &[f64]) -> Vec<f64> {
    let k = 1.0 / (CLICK_DUR_MS as f64 / 2000.0);
    t.iter().map(|&x| (-x * k).exp()).collect()
}

fn click_len_and_t(sr: f64) -> (usize, Vec<f64>) {
    let n = ms_samples(sr, CLICK_DUR_MS as f64);
    (n, lin_t(CLICK_DUR_MS as f64 / 1000.0, n))
}

/// `_generate_click`: the compass click, a decaying 1200 Hz sine.
pub fn generate_click(sr: f64, compass_click_amp: f64) -> Vec<f32> {
    let (n, t) = click_len_and_t(sr);
    let decay = click_decay(&t);
    let k = TWO_PI * 1200.0;
    let mut wave: Vec<f64> = t
        .iter()
        .zip(&decay)
        .map(|(&x, d)| (k * x).sin() * d)
        .collect();
    let fade_len = (n as f64 * 0.1) as usize;
    if fade_len > 0 {
        mul_tail(&mut wave, fade_len, 1.0, 0.0);
    }
    scaled_f32(&wave, compass_click_amp)
}

/// `_generate_highlight_click`: fundamental, 3rd and 5th harmonics, normalised.
pub fn generate_highlight_click(sr: f64, compass_click_amp: f64) -> Vec<f32> {
    let (n, t) = click_len_and_t(sr);
    let decay = click_decay(&t);
    let f0 = 1200.0;
    let (k1, k3, k5) = (TWO_PI * f0, TWO_PI * f0 * 3.0, TWO_PI * f0 * 5.0);
    let mut wave: Vec<f64> = t
        .iter()
        .map(|&x| 0.6 * (k1 * x).sin() + 0.3 * (k3 * x).sin() + 0.1 * (k5 * x).sin())
        .collect();
    div_peak(&mut wave);
    for (v, d) in wave.iter_mut().zip(&decay) {
        *v *= d;
    }
    let fade_len = (n as f64 * 0.1) as usize;
    if fade_len > 0 {
        mul_tail(&mut wave, fade_len, 1.0, 0.0);
    }
    scaled_f32(&wave, compass_click_amp)
}

/// `_generate_placement_pings`: the five FM placement pings, index 0 = "ones" through
/// 4 = "tens of thousands", each with its own carrier, ratio and modulation index.
pub fn generate_placement_pings(sr: f64, ping_amp: f64) -> Vec<Vec<f32>> {
    let mut waves = Vec::new();
    for &(fc, fm_ratio, beta_start, beta_end, dur_ms) in PLACEMENT_PING_RUNGS.iter() {
        let dur_s = dur_ms / 1000.0;
        let n = (sr * dur_s) as usize;
        if n <= 1 {
            waves.push(vec![0.0f32; 1]);
            continue;
        }
        let t = lin_t(dur_s, n);
        // Fast attack so the ping reads as a discrete event even at 8 per second, then
        // an exponential decay over the note.
        let ka = 1.0 / (dur_s * 0.06);
        let kd = 3.0 / dur_s;
        let beta = dsp::linspace(beta_start, beta_end, n, true);
        let fm = fc * fm_ratio;
        let (kc, km) = (TWO_PI * fc, TWO_PI * fm);
        let mut wave: Vec<f64> = t
            .iter()
            .zip(&beta)
            .map(|(&x, &b)| {
                let env = (1.0 - (-x * ka).exp()) * (-x * kd).exp();
                (kc * x + b * (km * x).sin()).sin() * env
            })
            .collect();
        div_peak_guarded(&mut wave);
        let fade_len = (n as f64 * 0.1) as usize;
        if fade_len > 0 {
            mul_tail(&mut wave, fade_len, 1.0, 0.0);
        }
        waves.push(scaled_f32(&wave, ping_amp));
    }
    waves
}

/// `_generate_cam_click`: FM, carrier 900 Hz, modulator 450 Hz, index 1.5.
pub fn generate_cam_click(sr: f64, compass_click_amp: f64) -> Vec<f32> {
    let (n, t) = click_len_and_t(sr);
    let decay = click_decay(&t);
    let (fc, fm, beta) = (900.0, 450.0, 1.5);
    let (kc, km) = (TWO_PI * fc, TWO_PI * fm);
    let mut wave: Vec<f64> = t
        .iter()
        .zip(&decay)
        .map(|(&x, d)| (kc * x + beta * (km * x).sin()).sin() * d)
        .collect();
    let fade_len = (n as f64 * 0.1) as usize;
    if fade_len > 0 {
        mul_tail(&mut wave, fade_len, 1.0, 0.0);
    }
    scaled_f32(&wave, compass_click_amp)
}

/// `_generate_center_click`: the short atonal tick marking the frame passing through
/// straight. Band-limited noise built in the frequency domain (a raised-cosine passband,
/// 2.5-7 kHz, with seeded random phase), so it cannot be mistaken for the FM clicks near
/// 900 Hz and stays where the ear is most sensitive.
pub fn generate_center_click(sr: f64) -> Vec<f32> {
    let n = ms_samples(sr, HYDRO_CENTER_CLICK_DUR_MS as f64);
    let t = arange_t(sr, n);
    let (lo, hi) = (HYDRO_CENTER_CLICK_LO_HZ, HYDRO_CENTER_CLICK_HI_HZ);
    let freqs = dsp::rfftfreq(n, 1.0 / sr);
    let mag: Vec<f64> = freqs
        .iter()
        .map(|&f| {
            let mut m = if f >= lo && f <= hi { 1.0 } else { 0.0 };
            // Raised-cosine skirts, so the burst has no ringing before or after.
            if f > lo * 0.7 && f < lo {
                m = 0.5 * (1.0 - (PI * (f - lo * 0.7) / (lo * 0.3)).cos());
            }
            if f > hi && f < hi * 1.3 {
                m = 0.5 * (1.0 + (PI * (f - hi) / (hi * 0.3)).cos());
            }
            m
        })
        .collect();
    let mut rng = dsp::default_rng(0xC0FFEE);
    let phases = rng.uniform_n(0.0, TWO_PI, freqs.len());
    let spec: Vec<Complex64> = mag
        .iter()
        .zip(&phases)
        .map(|(&m, &p)| Complex64::new(0.0, p).exp() * m)
        .collect();
    let mut wave = dsp::irfft(&spec, n);
    // Very fast decay: the point is a transient, not a beep.
    let k = 6.0 / (HYDRO_CENTER_CLICK_DUR_MS as f64 / 1000.0);
    for (v, &x) in wave.iter_mut().zip(&t) {
        *v *= (-x * k).exp();
    }
    let fade_len = (n as f64 * 0.15) as usize;
    if fade_len > 0 {
        mul_tail(&mut wave, fade_len, 1.0, 0.0);
    }
    div_peak_guarded(&mut wave); // normalise so the dB constant means what it says
    scaled_f32(&wave, 10.0f64.powf(HYDRO_CENTER_CLICK_DB / 20.0))
}

/// Two notes by integrated frequency (no phase jump at the change), shaped by an
/// exponential decay, 4 ms end fades and a dip across the seam: the docking lock chime
/// and the slam cues share it.
fn two_note(
    sr: f64,
    n: usize,
    half: Option<usize>,
    freq: &[f64],
    dur_s: f64,
    decay: f64,
) -> Vec<f64> {
    let t = arange_t(sr, n);
    let phase: Vec<f64> = dsp::cumsum(freq).iter().map(|p| p / sr).collect();
    let k = decay / dur_s;
    let mut wave: Vec<f64> = phase
        .iter()
        .zip(&t)
        .map(|(&p, &x)| (TWO_PI * p).sin() * (-x * k).exp())
        .collect();
    let edge = 1usize.max((sr * 0.004) as usize);
    mul_head(&mut wave, edge.min(n), 0.0, 1.0);
    if edge <= n {
        mul_tail(&mut wave, edge, 1.0, 0.0);
    }
    if let Some(half) = half {
        if half > edge {
            let h = edge / 2;
            let mut seam = dsp::linspace(1.0, 0.3, h, true);
            seam.extend(dsp::linspace(0.3, 1.0, h, true));
            for (v, g) in wave[half - h..half + h].iter_mut().zip(&seam) {
                *v *= g;
            }
        }
    }
    div_peak_guarded(&mut wave);
    wave
}

/// `_generate_dock_lock`: the descending two-note chime marking docking alignment.
pub fn generate_dock_lock(sr: f64) -> Vec<f32> {
    let dur_s = DOCK_LOCK_DUR_MS / 1000.0;
    let n = (sr * dur_s) as usize;
    let half = n / 2;
    let freq: Vec<f64> = (0..n)
        .map(|i| {
            if i < half {
                DOCK_LOCK_HI_HZ
            } else {
                DOCK_LOCK_LO_HZ
            }
        })
        .collect();
    let wave = two_note(sr, n, Some(half), &freq, dur_s, 3.0);
    scaled_f32(&wave, 10.0f64.powf(DOCK_LOCK_DB / 20.0))
}

/// `_generate_slam_cue(kind)`: `"clear"` and `"over"` are single ticks a fifth apart
/// (any other kind is `"over"`, as in Python); `"committed"` is the RISING pair.
pub fn generate_slam_cue(sr: f64, kind: &str) -> Vec<f32> {
    let (dur_s, n, half, freq) = if kind == "committed" {
        let dur_s = SLAM_COMMIT_DUR_MS / 1000.0;
        let n = (sr * dur_s) as usize;
        let half = n / 2;
        let freq: Vec<f64> = (0..n)
            .map(|i| {
                if i < half {
                    SLAM_COMMIT_LO_HZ
                } else {
                    SLAM_COMMIT_HI_HZ
                }
            })
            .collect();
        (dur_s, n, Some(half), freq)
    } else {
        let dur_s = SLAM_TICK_DUR_MS / 1000.0;
        let n = (sr * dur_s) as usize;
        let f = if kind == "clear" {
            SLAM_CLEAR_HZ
        } else {
            SLAM_OVER_HZ
        };
        (dur_s, n, None, vec![f; n])
    };
    let wave = two_note(sr, n, half, &freq, dur_s, 3.5);
    scaled_f32(&wave, 10.0f64.powf(SLAM_CUE_DB / 20.0))
}

/// `_generate_entry_cue`: a single high sine with a slower decay than the ticks.
pub fn generate_entry_cue(sr: f64) -> Vec<f32> {
    let dur_s = ENTRY_CUE_DUR_MS / 1000.0;
    let n = (sr * dur_s) as usize;
    let t = arange_t(sr, n);
    let (k, kd) = (TWO_PI * ENTRY_CUE_HZ, 2.5 / dur_s);
    let mut wave: Vec<f64> = t.iter().map(|&x| (k * x).sin() * (-x * kd).exp()).collect();
    let edge = 1usize.max((sr * 0.004) as usize);
    mul_head(&mut wave, edge.min(n), 0.0, 1.0);
    if edge <= n {
        mul_tail(&mut wave, edge, 1.0, 0.0);
    }
    div_peak_guarded(&mut wave);
    scaled_f32(&wave, 10.0f64.powf(ENTRY_CUE_DB / 20.0))
}

/// `_generate_dock_approach_cue`: two centred pips with a gap between them.
pub fn generate_dock_approach_cue(sr: f64) -> Vec<f32> {
    let pip_n = ms_samples(sr, DOCK_APPROACH_CUE_PIP_MS);
    let gap_n = ms_samples(sr, DOCK_APPROACH_CUE_GAP_MS);
    let k = TWO_PI * DOCK_APPROACH_CUE_HZ;
    let mut pip: Vec<f64> = arange_t(sr, pip_n).iter().map(|&x| (k * x).sin()).collect();
    let edge = (pip_n / 2).min(1usize.max((sr * 0.004) as usize));
    if edge > 0 {
        mul_head(&mut pip, edge, 0.0, 1.0);
        mul_tail(&mut pip, edge, 1.0, 0.0);
    }
    let mut wave = pip.clone();
    wave.extend(std::iter::repeat_n(0.0, gap_n));
    wave.extend_from_slice(&pip);
    div_peak_guarded(&mut wave);
    scaled_f32(&wave, 10.0f64.powf(DOCK_APPROACH_CUE_DB / 20.0))
}

/// `_generate_cam_highlight_click`: the cam click with a 3rd-harmonic carrier added.
pub fn generate_cam_highlight_click(sr: f64, compass_click_amp: f64) -> Vec<f32> {
    let (n, t) = click_len_and_t(sr);
    let decay = click_decay(&t);
    let (fc, fm, beta) = (900.0, 450.0, 1.5);
    let (kc, kc3, km) = (TWO_PI * fc, TWO_PI * fc * 3.0, TWO_PI * fm);
    let mut wave: Vec<f64> = t
        .iter()
        .map(|&x| {
            0.7 * (kc * x + beta * (km * x).sin()).sin()
                + 0.3 * (kc3 * x + beta * (km * x).sin()).sin()
        })
        .collect();
    div_peak(&mut wave);
    for (v, d) in wave.iter_mut().zip(&decay) {
        *v *= d;
    }
    let fade_len = (n as f64 * 0.1) as usize;
    if fade_len > 0 {
        mul_tail(&mut wave, fade_len, 1.0, 0.0);
    }
    scaled_f32(&wave, compass_click_amp)
}

/// FM with a decaying index: `sin(2 pi fc t + idx0 * exp(-t / tau) * sin(2 pi fm t))`.
fn decaying_fm(t: &[f64], fc: f64, fm: f64, idx0: f64, tau: f64) -> Vec<f64> {
    let (kc, km) = (2.0 * PI * fc, 2.0 * PI * fm);
    t.iter()
        .map(|&x| (kc * x + idx0 * (-x / tau).exp() * (km * x).sin()).sin())
        .collect()
}

/// `_generate_tc_tone`: the traction-control tone.
pub fn generate_tc_tone(sr: f64) -> Vec<f32> {
    let (dur, fc, cm_ratio) = (0.032, 500.0, 3.075);
    let fm = fc / cm_ratio;
    let n = (sr * dur) as usize;
    let t = lin_t(dur, n);
    let carrier = decaying_fm(&t, fc, fm, 4.0, dur / 3.0);
    t.iter()
        .zip(&carrier)
        .map(|(&x, c)| ((0.4 * (PI * x / dur).sin()) * c) as f32)
        .collect()
}

/// Squared linear fade from `fade_start` to the end, as the buzzers do.
fn squared_fade(env: &mut [f64], fade_start: usize) {
    let n = env.len();
    if n > fade_start {
        for (e, g) in
            env[fade_start..]
                .iter_mut()
                .zip(dsp::linspace(1.0, 0.0, n - fade_start, true))
        {
            *e = g * g;
        }
    }
}

/// `_generate_check_engine_buzzer(amplitude)`.
pub fn generate_check_engine_buzzer(sr: f64, amplitude: f64) -> Vec<f32> {
    let (dur, fc, fm) = (0.5, 400.0, 150.0);
    let n = (sr * dur) as usize;
    let t = lin_t(dur, n);
    let carrier = decaying_fm(&t, fc, fm, 8.0, dur / 3.0);
    let mut amp_env = vec![1.0f64; n];
    squared_fade(&mut amp_env, (sr * 0.4) as usize);
    amp_env
        .iter()
        .zip(&carrier)
        .map(|(e, c)| (amplitude * e * c) as f32)
        .collect()
}

/// `_generate_describe_error_buzz`: low, dark FM buzz for the AI describer
/// double-press rejection.
pub fn generate_describe_error_buzz(sr: f64) -> Vec<f32> {
    let (dur, fc, fm) = (0.25, 220.0, 90.0);
    let amplitude = 10.0f64.powf(-18.0 / 20.0);
    let n = (sr * dur) as usize;
    let t = lin_t(dur, n);
    let carrier = decaying_fm(&t, fc, fm, 7.0, dur / 2.5);
    let mut amp_env = vec![1.0f64; n];
    let attack = (sr * 0.005) as usize;
    if attack > 0 {
        let a = attack.min(n);
        for (e, g) in amp_env[..a]
            .iter_mut()
            .zip(dsp::linspace(0.0, 1.0, attack, true))
        {
            *e = g;
        }
    }
    squared_fade(&mut amp_env, (sr * 0.15) as usize);
    amp_env
        .iter()
        .zip(&carrier)
        .map(|(e, c)| (amplitude * e * c) as f32)
        .collect()
}

/// `_generate_oil_chime(amplitude)`.
pub fn generate_oil_chime(sr: f64, amplitude: f64) -> Vec<f32> {
    let (dur, fc) = (1.0, 523.25);
    let n = (sr * dur) as usize;
    let t = lin_t(dur, n);
    let (k1, k2, k3) = (2.0 * PI * fc, 2.0 * PI * fc * 2.0, 2.0 * PI * fc * 3.0);
    let mut wave: Vec<f64> = t
        .iter()
        .map(|&x| (k1 * x).sin() + 0.5 * (k2 * x).sin() + 0.2 * (k3 * x).sin())
        .collect();
    div_peak(&mut wave);
    let mut amp_env: Vec<f64> = t
        .iter()
        .map(|&x| (-x * 5.0).exp() * (1.0 - (-x * 30.0).exp()))
        .collect();
    let m = dsp::max(&amp_env);
    for e in amp_env.iter_mut() {
        *e /= m;
    }
    amp_env
        .iter()
        .zip(&wave)
        .map(|(e, w)| (amplitude * e * w) as f32)
        .collect()
}

fn scanner_t(sr: f64) -> (Vec<f64>, Vec<f64>) {
    let n = ms_samples(sr, SCANNER_BEEP_DUR_MS as f64);
    let t = lin_t(SCANNER_BEEP_DUR_MS as f64 / 1000.0, n);
    let k = 1.0 / (SCANNER_BEEP_DUR_MS as f64 / 4000.0);
    let env = t.iter().map(|&x| (-x * k).exp()).collect();
    (t, env)
}

/// `_generate_scanner_beep`: a triangle carrier (arcsine of sine), FM by a sine at 2:1,
/// index 1.0.
pub fn generate_scanner_beep(sr: f64) -> Vec<f32> {
    let (t, env) = scanner_t(sr);
    let fc = SCANNER_BEEP_FREQ_HZ;
    let fm = 2.0 * fc;
    let beta = 1.0;
    let (kc, km) = (TWO_PI * fc, TWO_PI * fm);
    t.iter()
        .zip(&env)
        .map(|(&x, e)| {
            let phase = kc * x + beta * (km * x).sin();
            let wave = (2.0 / PI) * phase.sin().asin();
            (wave * e * 0.25) as f32
        })
        .collect()
}

/// `_generate_scanner_aligned_beep`: the same envelope with odd harmonics added.
pub fn generate_scanner_aligned_beep(sr: f64) -> Vec<f32> {
    let (t, env) = scanner_t(sr);
    let kc = TWO_PI * SCANNER_BEEP_FREQ_HZ;
    let mut wave: Vec<f64> = t
        .iter()
        .map(|&x| {
            let p = kc * x;
            p.sin() + 0.4 * (3.0 * p).sin() + 0.25 * (5.0 * p).sin() + 0.15 * (7.0 * p).sin()
        })
        .collect();
    div_peak(&mut wave);
    wave.iter()
        .zip(&env)
        .map(|(w, e)| (w * e * 0.25) as f32)
        .collect()
}

/// `_generate_obstacle_buzz`: the obstacle-only harmonic pulse. Returns the pulse and
/// the `_obstacle_harmonic_peak` Python stores as a side effect (the summed spectrum's
/// peak before normalisation, 1.0 if it was zero).
pub fn generate_obstacle_buzz(sr: f64, obstacle_amp: f64) -> (Vec<f32>, f64) {
    let n = ms_samples(sr, OBSTACLE_CUE_DUR_MS as f64);
    let t = arange_t(sr, n);
    let k = TWO_PI * OBSTACLE_CUE_FREQ_HZ;
    let phase: Vec<f64> = t.iter().map(|&x| k * x).collect();
    let mut wave = vec![0.0f64; n];
    for &(harmonic, amplitude) in OBSTACLE_CUE_HARMONICS.iter() {
        let h = harmonic as f64;
        for (v, p) in wave.iter_mut().zip(&phase) {
            *v += amplitude * (h * p).sin();
        }
    }
    let peak = if n > 0 { dsp::max_abs(&wave) } else { 0.0 };
    let harmonic_peak = if peak > 0.0 { peak } else { 1.0 };
    if peak > 0.0 {
        for v in wave.iter_mut() {
            *v /= peak;
        }
    }
    let attack = ms_samples(sr, OBSTACLE_CUE_ATTACK_MS as f64);
    let body = ms_samples(sr, OBSTACLE_CUE_BODY_MS as f64);
    let release = 1usize.max(n.saturating_sub(attack + body));
    let mut env = vec![1.0f64; n];
    if attack > 0 {
        for (e, x) in env[..attack.min(n)]
            .iter_mut()
            .zip(dsp::linspace(0.0, PI, attack, true))
        {
            *e = 0.5 - 0.5 * x.cos();
        }
    }
    if attack + body < n {
        for (e, x) in env[attack + body..]
            .iter_mut()
            .zip(dsp::linspace(0.0, PI, release, true))
        {
            *e = 0.5 + 0.5 * x.cos();
        }
    }
    let out = wave
        .iter()
        .zip(&env)
        .map(|(w, e)| (w * e * obstacle_amp) as f32)
        .collect();
    (out, harmonic_peak)
}

/// A square-wave exponential sweep (the terrain warnings).
fn square_sweep(sr: f64, f_start: f64, f_end: f64) -> Vec<f32> {
    let dur = TERRAIN_SWEEP_DUR_MS as f64 / 1000.0;
    let n = ms_samples(sr, TERRAIN_SWEEP_DUR_MS as f64);
    let t = lin_t(dur, n);
    let amplitude = 10.0f64.powf(TERRAIN_SWEEP_AMP_DB / 20.0);
    let ratio = f_end / f_start;
    let freq: Vec<f64> = t.iter().map(|&x| f_start * ratio.powf(x / dur)).collect();
    let phase: Vec<f64> = dsp::cumsum(&freq).iter().map(|c| TWO_PI * c / sr).collect();
    phase
        .iter()
        .zip(&t)
        .map(|(&p, &x)| {
            let s = p.sin();
            let wave = if s > 0.0 {
                1.0
            } else if s < 0.0 {
                -1.0
            } else {
                s // np.sign(0) is 0 (and keeps NaN)
            };
            let env = (-x * 3.0 / dur).exp();
            (wave * env * amplitude) as f32
        })
        .collect()
}

/// `_generate_dropoff_sweep`: terrain drop-off warning, a descending square sweep.
pub fn generate_dropoff_sweep(sr: f64) -> Vec<f32> {
    square_sweep(sr, DROPOFF_FREQ_START, DROPOFF_FREQ_END)
}

/// `_generate_hill_sweep`: steep hill warning, an ascending square sweep.
pub fn generate_hill_sweep(sr: f64) -> Vec<f32> {
    square_sweep(sr, HILL_FREQ_START, HILL_FREQ_END)
}

/// `_generate_road_beep`: off-road guidance beep, F#5 carrier with a sub-octave FM
/// modulator whose index decays through the pulse. `None` for a zero-length pulse.
pub fn generate_road_beep(sr: f64, road_amp: f64) -> Option<Vec<f32>> {
    let n = ms_samples(sr, ROAD_BEEP_DUR_MS as f64);
    if n == 0 {
        return None;
    }
    let t = lin_t(ROAD_BEEP_DUR_MS as f64 / 1000.0, n);
    let (kc, km) = (TWO_PI * ROAD_BEEP_FREQ_HZ, TWO_PI * ROAD_BEEP_MOD_HZ);
    Some(
        t.iter()
            .map(|&x| {
                let modulator = ROAD_BEEP_MOD_INDEX * (-12.0 * x).exp() * (km * x).sin();
                let wave = (kc * x + modulator).sin();
                let env = (1.0 - (-200.0 * x).exp()) * (-12.0 * x).exp();
                (wave * env * road_amp) as f32
            })
            .collect(),
    )
}

/// `_generate_route_beacon_pulse`: C5 carrier with an octave-above FM modulator, a soft
/// bell that sits under the driving.
pub fn generate_route_beacon_pulse(sr: f64, route_beacon_amp: f64) -> Option<Vec<f32>> {
    let n = ms_samples(sr, ROUTE_BEACON_DUR_MS as f64);
    if n == 0 {
        return None;
    }
    let t = lin_t(ROUTE_BEACON_DUR_MS as f64 / 1000.0, n);
    let (kc, km) = (TWO_PI * ROUTE_BEACON_FREQ_HZ, TWO_PI * ROUTE_BEACON_MOD_HZ);
    Some(
        t.iter()
            .map(|&x| {
                let modulator = ROUTE_BEACON_MOD_INDEX * (-18.0 * x).exp() * (km * x).sin();
                let wave = (kc * x + modulator).sin();
                let env = (1.0 - (-120.0 * x).exp()) * (-16.0 * x).exp();
                (wave * env * route_beacon_amp) as f32
            })
            .collect(),
    )
}

/// `_generate_road_chime_tone(freq_hz)`: sine carrier with a 1:1 FM modulator whose
/// index starts high (clicky attack) and decays to a low sustained value.
pub fn generate_road_chime_tone(sr: f64, freq_hz: f64) -> Option<Vec<f32>> {
    let n = ms_samples(sr, ROAD_CHIME_DUR_MS as f64);
    if n == 0 {
        return None;
    }
    let t = lin_t(ROAD_CHIME_DUR_MS as f64 / 1000.0, n);
    let k = TWO_PI * freq_hz;
    let wave: Vec<f64> = t
        .iter()
        .map(|&x| {
            let mod_index = ROAD_CHIME_MOD_INDEX_FLOOR
                + (ROAD_CHIME_MOD_INDEX_PEAK - ROAD_CHIME_MOD_INDEX_FLOOR)
                    * (-ROAD_CHIME_MOD_DECAY * x).exp();
            (k * x + mod_index * (k * x).sin()).sin()
        })
        .collect();
    let mut env: Vec<f64> = t
        .iter()
        .map(|&x| (1.0 - (-100.0 * x).exp()) * (-6.0 * x).exp())
        .collect();
    let fade = (sr * 0.008) as usize;
    if 0 < fade && fade < n {
        mul_tail(&mut env, fade, 1.0, 0.0);
    }
    let amp = 10.0f64.powf(ROAD_CHIME_AMP_DB / 20.0);
    Some(
        wave.iter()
            .zip(&env)
            .map(|(w, e)| (w * e * amp) as f32)
            .collect(),
    )
}

/// `np.minimum(1.0, (samples - np.arange(samples)) / max(1.0, sr * secs))`.
fn release_ramp(samples: usize, sr: f64, secs: f64) -> Vec<f64> {
    let d = py_max(1.0, sr * secs);
    (0..samples)
        .map(|i| py_min_np(1.0, (samples - i) as f64 / d))
        .collect()
}

/// `np.minimum(a, b)` for one element (NaN propagates).
fn py_min_np(a: f64, b: f64) -> f64 {
    if b.is_nan() { b } else { a.min(b) }
}

/// `_generate_road_junction_pip`: the centred junction pip.
pub fn generate_road_junction_pip(sr: f64, road_junction_amp: f64) -> Option<Vec<f32>> {
    let n = ms_samples(sr, ROAD_JUNCTION_PIP_MS as f64);
    if n == 0 {
        return None;
    }
    let t = arange_t(sr, n);
    let k = TWO_PI * ROAD_JUNCTION_FREQ_HZ;
    let release = release_ramp(n, sr, 0.010);
    Some(
        t.iter()
            .zip(&release)
            .map(|(&x, r)| {
                let attack = py_min_np(1.0, x / 0.006);
                ((k * x).sin() * attack * r * road_junction_amp) as f32
            })
            .collect(),
    )
}

/// Exponential glide by integrated frequency, `start * (end / start) ** progress`.
fn glide_phase(sr: f64, n: usize, start: f64, end: f64) -> Vec<f64> {
    let d = 1usize.max(n.saturating_sub(1)) as f64;
    let ratio = end / start;
    let freq: Vec<f64> = (0..n).map(|i| start * ratio.powf(i as f64 / d)).collect();
    dsp::cumsum(&freq).iter().map(|c| TWO_PI * c / sr).collect()
}

/// `_generate_road_junction_entry`: single low descending tone at the junction boundary.
pub fn generate_road_junction_entry(sr: f64, road_junction_amp: f64) -> Option<Vec<f32>> {
    let n = ms_samples(sr, ROAD_JUNCTION_ENTRY_MS as f64);
    if n == 0 {
        return None;
    }
    let t = arange_t(sr, n);
    let phase = glide_phase(
        sr,
        n,
        ROAD_JUNCTION_ENTRY_START_HZ,
        ROAD_JUNCTION_ENTRY_END_HZ,
    );
    let release = release_ramp(n, sr, 0.045);
    Some(
        (0..n)
            .map(|i| {
                let wave = phase[i].sin() + 0.18 * (2.0 * phase[i]).sin();
                let attack = py_min_np(1.0, t[i] / 0.008);
                (wave * attack * release[i] * road_junction_amp) as f32
            })
            .collect(),
    )
}

/// `_generate_road_correction_settled`: short rising confirmation that lane recovery
/// has settled.
pub fn generate_road_correction_settled(sr: f64, road_correction_amp: f64) -> Option<Vec<f32>> {
    let n = ms_samples(sr, ROAD_CORRECTION_SETTLED_MS as f64);
    if n == 0 {
        return None;
    }
    let phase = glide_phase(
        sr,
        n,
        ROAD_CORRECTION_SETTLED_START_HZ,
        ROAD_CORRECTION_SETTLED_END_HZ,
    );
    let ad = py_max(1.0, sr * 0.006);
    let release = release_ramp(n, sr, 0.025);
    Some(
        (0..n)
            .map(|i| {
                let attack = py_min_np(1.0, i as f64 / ad);
                (phase[i].sin() * attack * release[i] * road_correction_amp) as f32
            })
            .collect(),
    )
}

/// `_generate_guidance_tone`: Python returns None; the heading guidance tone is
/// synthesised in the callback.
pub fn generate_guidance_tone(_sr: f64) -> Option<Vec<f32>> {
    None
}

/// `_generate_lowspeed_click(ls_click_amp_db)`: FM, 250 Hz carrier, 375 Hz modulator.
pub fn generate_lowspeed_click(sr: f64, ls_click_amp_db: f64) -> Vec<f32> {
    let n = ms_samples(sr, LS_CLICK_DUR_MS as f64);
    let t = lin_t(LS_CLICK_DUR_MS as f64 / 1000.0, n);
    let (kc, km) = (TWO_PI * LS_CARRIER_HZ, TWO_PI * LS_MOD_HZ);
    let amp = 10.0f64.powf(ls_click_amp_db / 20.0);
    t.iter()
        .map(|&x| {
            let mod_env = LS_MOD_INDEX_PEAK * (-30.0 * x).exp();
            let carrier = (kc * x + mod_env * (km * x).sin()).sin();
            let env = (1.0 - (-500.0 * x).exp()) * (-15.0 * x).exp();
            (amp * env * carrier) as f32
        })
        .collect()
}

/// `_generate_lowspeed_stop_tone(ls_stop_amp_db)`: one low tone confirming the vehicle
/// has come to rest.
pub fn generate_lowspeed_stop_tone(sr: f64, ls_stop_amp_db: f64) -> Vec<f32> {
    let n = ms_samples(sr, LS_STOP_TONE_MS as f64);
    let t = lin_t(LS_STOP_TONE_MS as f64 / 1000.0, n);
    let k = TWO_PI * LS_STOP_TONE_HZ;
    let amp = 10.0f64.powf(ls_stop_amp_db / 20.0);
    t.iter()
        .map(|&x| {
            let env = (1.0 - (-300.0 * x).exp()) * (-12.0 * x).exp();
            (amp * env * (k * x).sin()) as f32
        })
        .collect()
}

fn clickspot_t(sr: f64) -> (usize, Vec<f64>) {
    let n = ms_samples(sr, CLICKSPOT_BEEP_DUR_MS as f64);
    (n, lin_t(CLICKSPOT_BEEP_DUR_MS as f64 / 1000.0, n))
}

/// `_generate_clickspot_beep`: an FM click, stored forward.
pub fn generate_clickspot_beep(sr: f64) -> Vec<f32> {
    let (_, t) = clickspot_t(sr);
    let (kc, km) = (
        TWO_PI * CLICKSPOT_BEEP_FREQ_HZ,
        TWO_PI * CLICKSPOT_BEEP_MOD_HZ,
    );
    let amp = 10.0f64.powf(CLICKSPOT_BEEP_AMP_DB / 20.0);
    t.iter()
        .map(|&x| {
            let modulator = CLICKSPOT_BEEP_MOD_INDEX * (-60.0 * x).exp() * (km * x).sin();
            let wave = (kc * x + modulator).sin() * (-40.0 * x).exp();
            (wave * amp) as f32
        })
        .collect()
}

/// `_generate_clickspot_beep_reverse`: the leaving variant, rising modulation and a
/// rising envelope with a 3 ms fade at the end.
pub fn generate_clickspot_beep_reverse(sr: f64) -> Vec<f32> {
    let (n, t) = clickspot_t(sr);
    let last = t.last().copied().unwrap_or(0.0);
    let (kc, km) = (
        TWO_PI * CLICKSPOT_BEEP_FREQ_HZ,
        TWO_PI * CLICKSPOT_BEEP_MOD_HZ,
    );
    let mut wave: Vec<f64> = t
        .iter()
        .map(|&x| {
            let t_rev = last - x;
            let modulator = CLICKSPOT_BEEP_MOD_INDEX * (-60.0 * t_rev).exp() * (km * x).sin();
            (kc * x + modulator).sin()
        })
        .collect();
    let mut env: Vec<f64> = t.iter().map(|&x| 1.0 - (-40.0 * x).exp()).collect();
    let fade = (sr * 0.003) as usize;
    if fade > 0 && fade < n {
        mul_tail(&mut env, fade, 1.0, 0.0);
    }
    for (w, e) in wave.iter_mut().zip(&env) {
        *w *= e;
    }
    scaled_f32(&wave, 10.0f64.powf(CLICKSPOT_BEEP_AMP_DB / 20.0))
}

fn node_t(sr: f64) -> (usize, Vec<f64>) {
    let n = ms_samples(sr, NODE_BEEP_DUR_MS as f64);
    (n, lin_t(NODE_BEEP_DUR_MS as f64 / 1000.0, n))
}

/// `_generate_node_beep`: a sine template at the base frequency, decaying; pitch is
/// applied at playback.
pub fn generate_node_beep(sr: f64) -> Vec<f32> {
    let (_, t) = node_t(sr);
    let k = TWO_PI * NODE_BEEP_BASE_FREQ_HZ;
    let rate = 5.0 / (NODE_BEEP_DUR_MS as f64 / 1000.0);
    let amp = 10.0f64.powf(NODE_BEEP_AMP_DB / 20.0);
    t.iter()
        .map(|&x| ((k * x).sin() * (-rate * x).exp() * amp) as f32)
        .collect()
}

/// `_generate_node_beep_reverse`: the leaving variant, rising envelope.
pub fn generate_node_beep_reverse(sr: f64) -> Vec<f32> {
    let (n, t) = node_t(sr);
    let k = TWO_PI * NODE_BEEP_BASE_FREQ_HZ;
    let rate = 5.0 / (NODE_BEEP_DUR_MS as f64 / 1000.0);
    let mut env: Vec<f64> = t.iter().map(|&x| 1.0 - (-rate * x).exp()).collect();
    let fade = (sr * 0.003) as usize;
    if fade > 0 && fade < n {
        mul_tail(&mut env, fade, 1.0, 0.0);
    }
    let wave: Vec<f64> = t
        .iter()
        .zip(&env)
        .map(|(&x, e)| (k * x).sin() * e)
        .collect();
    scaled_f32(&wave, 10.0f64.powf(NODE_BEEP_AMP_DB / 20.0))
}

/// The maths of `_slip_voice_tables` (the caching on sample rate is the caller's):
/// `(spin_table, lock_table, noise_lo, noise_hi)`. The tone tables are one cycle each,
/// band-limited to SLIP_HARMONIC_TOP_HZ so the HRTF can place them; the noise loops are
/// periodic (built by inverse FFT), so reading them round and round is seamless.
/// `sr` is truncated to an integer first, as Python's `int(self.samplerate)`.
pub fn slip_voice_tables(sr: f64) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
    let sr = sr.trunc();
    let top = py_min(SLIP_HARMONIC_TOP_HZ, 0.45 * sr);
    let len = SLIP_TABLE_LEN as usize;
    let ph: Vec<f64> = (0..len).map(|i| i as f64 / SLIP_TABLE_LEN as f64).collect();

    let additive = |f0: f64, odd_only: bool| -> Vec<f32> {
        let mut out = vec![0.0f64; len];
        let mut n = 1i64;
        while n as f64 * f0 <= top {
            if !odd_only || n % 2 == 1 {
                let k = 2.0 * PI * n as f64;
                for (v, &p) in out.iter_mut().zip(&ph) {
                    *v += (k * p).sin() / n as f64;
                }
            }
            n += 1;
        }
        let mut rms = dsp::rms(&out);
        if rms == 0.0 {
            rms = 1.0;
        }
        scaled_f32(&out, SLIP_TABLE_RMS / rms)
    };

    let n_noise = (SLIDE_NOISE_S * sr) as usize;
    let bins = n_noise / 2 + 1;
    let mut rng = dsp::default_rng(0x511DE);
    let re = rng.standard_normal_n(bins);
    let im = rng.standard_normal_n(bins);
    // `re + 1j * im`: (0 + 1j) * im is (0*im - 0, im), so the real part is re + 0.0.
    let spec: Vec<Complex64> = re
        .iter()
        .zip(&im)
        .map(|(&r, &i)| Complex64::new(r + (0.0 * i - 0.0), i))
        .collect();
    let freqs = dsp::rfftfreq(n_noise, 1.0 / sr);

    let band = |lo: f64, hi: f64| -> Vec<f32> {
        let s: Vec<Complex64> = spec
            .iter()
            .zip(&freqs)
            .map(|(z, &f)| {
                if f >= lo && f <= hi {
                    *z
                } else {
                    Complex64::new(0.0, 0.0) * *z
                }
            })
            .collect();
        let x = dsp::irfft(&s, n_noise);
        let mut rms = dsp::rms(&x);
        if rms == 0.0 {
            rms = 1.0;
        }
        scaled_f32(&x, SLIP_TABLE_RMS / rms)
    };

    (
        additive(SLIP_SPIN_HZ, false),
        additive(SLIP_LOCK_HZ, true),
        band(SLIDE_BAND_LO.0, SLIDE_BAND_LO.1),
        band(SLIDE_BAND_HI.0, SLIDE_BAND_HI.1),
    )
}
