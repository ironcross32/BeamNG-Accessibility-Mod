//! Terrain sonification scanner: `render_scan` and its grain functions (audio.py).
//!
//! One-shot picture of the landscape, rendered whole on the LISTENER thread, never in
//! the callback. Pitch is elevation (Pythagorean, middle C at the player's own ground),
//! time is distance (a fixed SCAN_DURATION_S whatever the reach), pan is bearing, and
//! timbre is what the ground is. Level carries nothing. See the `SCAN_*` constants in
//! [`crate::consts`] for the reasoning behind each number.
//!
//! dtypes follow the Python: envelopes and raw waves are float64, every finished grain is
//! float32, and the cloud is accumulated in float32. The onset jitter uses numpy's own
//! `default_rng(SCAN_RNG_SEED)` stream (reproduced in [`crate::dsp`]), so a scan renders
//! identically to Python's, not merely "identically twice".

use std::collections::HashMap;
use std::f64::consts::PI;

use crate::consts::*;
use crate::dsp;

/// Where Python raises inside `render_scan` (`int()` of a NaN or infinite value).
#[derive(Debug, Clone, PartialEq)]
pub struct ScanError(pub String);

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScanError {}

/// Python `int(x)` on a float: truncate, raising on NaN and infinity.
fn py_int(x: f64) -> Result<i64, ScanError> {
    if x.is_nan() {
        return Err(ScanError("cannot convert float NaN to integer".into()));
    }
    if x.is_infinite() {
        return Err(ScanError("cannot convert float infinity to integer".into()));
    }
    Ok(x.trunc() as i64)
}

/// Equal-power stereo gains for a pan position in [-1, 1]. Shared by the render path and
/// the controller's `_pan_gains`, so the two cannot drift apart.
pub fn equal_power_pan(p: f64) -> (f64, f64) {
    let p = if p < -1.0 {
        -1.0
    } else if p > 1.0 {
        1.0
    } else {
        p
    };
    let ang = (p + 1.0) * (PI / 4.0);
    (ang.cos(), ang.sin())
}

/// Metres of elevation relative to the player's own ground -> a scale degree index.
///
/// The quantization grid is 12 equal steps per octave and the SOUNDED pitch of each step
/// is Pythagorean; the nearest degree on a linear grid keeps the readout monotonic in
/// elevation. Clamped rather than wrapped: a wrapped octave would report a peak as a pit.
/// `Err` for NaN, where Python's `round` raises.
pub fn try_scan_step_from_dz(dz_m: f64) -> Result<i64, ScanError> {
    let mut oct_pos = dz_m / SCAN_M_PER_OCTAVE;
    if oct_pos < -SCAN_OCTAVE_CLAMP {
        oct_pos = -SCAN_OCTAVE_CLAMP;
    } else if oct_pos > SCAN_OCTAVE_CLAMP {
        oct_pos = SCAN_OCTAVE_CLAMP;
    }
    if oct_pos.is_nan() {
        return Err(ScanError("cannot convert float NaN to integer".into()));
    }
    Ok((oct_pos * 12.0).round_ties_even() as i64)
}

/// [`try_scan_step_from_dz`] for finite input (NaN maps to 0 rather than raising).
pub fn scan_step_from_dz(dz_m: f64) -> i64 {
    try_scan_step_from_dz(dz_m).unwrap_or(0)
}

/// The frequency of a scale step, middle C at step 0.
fn pitch_of_step(step: i64, octave_offset: f64) -> f64 {
    let octave = step.div_euclid(12); // floor division, so degree is always 0..11
    let degree = step.rem_euclid(12) as usize;
    SCAN_MIDDLE_C_HZ * SCAN_PYTHAGOREAN_RATIOS[degree] * 2.0f64.powf(octave as f64 + octave_offset)
}

/// Metres of elevation -> a Pythagorean-quantized frequency, middle C at level ground.
pub fn scan_pitch_hz(dz_m: f64, octave_offset: f64) -> f64 {
    pitch_of_step(scan_step_from_dz(dz_m), octave_offset)
}

fn rc_rise(n: usize) -> Vec<f64> {
    dsp::raised_cosine_rise(n)
}

/// `(1 - u) ** SCAN_GRAIN_DECAY_POW` with `u = arange(n) / max(1, n - 1)`.
fn cubic_decay(n: usize) -> Vec<f64> {
    let d = 1usize.max(n.saturating_sub(1)) as f64;
    (0..n)
        .map(|i| (1.0 - i as f64 / d).powf(SCAN_GRAIN_DECAY_POW))
        .collect()
}

/// Flat-topped window with a raised-cosine attack and a CUBIC release: the reference
/// ping's envelope. It needs a sustained body (an absolute pitch cannot be judged against
/// a transient) and decays into the cloud on the same (1-u)^3 curve the grains use.
pub fn scan_envelope(n: usize, sr: f64, release_ms: f64) -> Vec<f64> {
    let mut env = vec![1.0f64; n];
    let fade = ((sr * SCAN_GRAIN_FADE_MS / 1000.0) as usize).min(n / 2);
    if fade > 0 {
        for (e, r) in env[..fade].iter_mut().zip(rc_rise(fade)) {
            *e *= r;
        }
    }
    let rel = ((sr * release_ms / 1000.0) as usize).min(n);
    if rel > 0 {
        for (e, d) in env[n - rel..].iter_mut().zip(cubic_decay(rel)) {
            *e *= d;
        }
    }
    env
}

/// The grain window: a short raised-cosine attack times a cubic decay over the WHOLE
/// grain. No plateau, and that absence is the point; it ends at zero value and zero
/// slope, so it needs no release taper.
pub fn scan_grain_envelope(n: usize, sr: f64, attack_mult: f64) -> Vec<f64> {
    let mut env = cubic_decay(n);
    let att = ((sr * SCAN_GRAIN_ATTACK_MS * attack_mult / 1000.0) as usize).min(n);
    if att > 0 {
        for (e, r) in env[..att].iter_mut().zip(rc_rise(att)) {
            *e *= r;
        }
    }
    env
}

/// `sin(k * t)` over a time axis, `k` already the scalar product Python forms first.
fn sin_kt(k: f64, t: &[f64]) -> impl Iterator<Item = f64> + '_ {
    t.iter().map(move |&x| (k * x).sin())
}

/// The raw (pre-envelope) waveform for one surface family.
pub fn scan_family_wave(freq_hz: f64, t: &[f64], sr: f64, family: &str) -> Vec<f64> {
    let nyq = 0.45 * sr;
    if family == "ice" {
        let mut w = vec![0.0f64; t.len()];
        let det = 2.0f64.powf(SCAN_ICE_DETUNE_CENTS / 1200.0);
        for &(mult, db) in SCAN_ICE_PARTIALS.iter() {
            let amp = 10.0f64.powf(db / 20.0);
            if mult <= 2.0 {
                if freq_hz * mult < nyq {
                    let k = 2.0 * PI * freq_hz * mult;
                    for (v, s) in w.iter_mut().zip(sin_kt(k, t)) {
                        *v += amp * s;
                    }
                }
                continue;
            }
            // The upper octaves are DETUNED PAIRS, so the shimmer is a consequence of the
            // spectrum rather than a tremolo bolted on top of it.
            for f in [freq_hz * mult * det, freq_hz * mult / det] {
                if f < nyq {
                    let k = 2.0 * PI * f;
                    let a = amp * 0.5;
                    for (v, s) in w.iter_mut().zip(sin_kt(k, t)) {
                        *v += a * s;
                    }
                }
            }
        }
        return w;
    }

    if family == "dirt" || family == "gravel" {
        let index = if family == "dirt" {
            SCAN_DIRT_FM_INDEX
        } else {
            SCAN_GRAVEL_FM_INDEX
        };
        let fm = freq_hz * SCAN_LOOSE_FM_RATIO;
        let kc = 2.0 * PI * freq_hz;
        let km = 2.0 * PI * fm;
        let mut w: Vec<f64> = t
            .iter()
            .map(|&x| (kc * x + index * (km * x).sin()).sin())
            .collect();
        if freq_hz * 2.0 < nyq {
            let a = 10.0f64.powf(SCAN_LOOSE_H2_DB / 20.0);
            for (v, s) in w.iter_mut().zip(sin_kt(4.0 * PI * freq_hz, t)) {
                *v += a * s;
            }
        }
        return w;
    }

    let (h2, h3) = if family == "grass" {
        (SCAN_GRASS_H2_DB, SCAN_GRASS_H3_DB)
    } else {
        (SCAN_TERRAIN_H2_DB, SCAN_TERRAIN_H3_DB)
    };
    let mut w: Vec<f64> = sin_kt(2.0 * PI * freq_hz, t).collect();
    if freq_hz * 2.0 < nyq {
        let a = 10.0f64.powf(h2 / 20.0);
        for (v, s) in w.iter_mut().zip(sin_kt(4.0 * PI * freq_hz, t)) {
            *v += a * s;
        }
    }
    if freq_hz * 3.0 < nyq {
        let a = 10.0f64.powf(h3 / 20.0);
        for (v, s) in w.iter_mut().zip(sin_kt(6.0 * PI * freq_hz, t)) {
            *v += a * s;
        }
    }
    w
}

fn grain_len(sr: f64, ms: f64) -> usize {
    1usize.max((sr * ms / 1000.0) as usize)
}

fn normalise_to_f32(w: &[f64]) -> Vec<f32> {
    let peak = dsp::max_abs(w);
    if peak > 0.0 {
        w.iter().map(|&v| (v / peak) as f32).collect()
    } else {
        dsp::to_f32(w)
    }
}

/// One terrain sample. `family` selects TIMBRE and nothing else. The families are
/// loudness-matched to paved by RMS, two-sided, with no peak clamp after it (the cloud is
/// peak-normalised as a whole afterwards), so "no family is louder or quieter than its
/// neighbours" is a property of the code rather than of the constants.
pub fn scan_terrain_grain(freq_hz: f64, sr: f64, family: &str) -> Vec<f32> {
    let n = grain_len(sr, SCAN_GRAIN_MS);
    let t = dsp::arange_div(n, sr);
    let mult = if family == "ice" {
        SCAN_ICE_ATTACK_MULT
    } else {
        1.0
    };
    let env = scan_grain_envelope(n, sr, mult);
    let mut w: Vec<f64> = scan_family_wave(freq_hz, &t, sr, family)
        .iter()
        .zip(&env)
        .map(|(a, b)| a * b)
        .collect();
    let peak = dsp::max_abs(&w);
    if peak > 0.0 {
        for v in w.iter_mut() {
            *v /= peak;
        }
    }
    if family != SCAN_PAVED {
        let env1 = scan_grain_envelope(n, sr, 1.0);
        let mut reference: Vec<f64> = scan_family_wave(freq_hz, &t, sr, SCAN_PAVED)
            .iter()
            .zip(&env1)
            .map(|(a, b)| a * b)
            .collect();
        let rpeak = dsp::max_abs(&reference);
        if rpeak > 0.0 {
            for v in reference.iter_mut() {
                *v /= rpeak;
            }
        }
        let rms = dsp::rms(&w);
        let rms_ref = dsp::rms(&reference);
        if rms > 1e-12 && rms_ref > 0.0 {
            let g = rms_ref / rms;
            for v in w.iter_mut() {
                *v *= g;
            }
        }
    }
    dsp::to_f32(&w)
}

/// One water sample: the same pitch rule on a bright 1/k saw, with vibrato as a phase
/// deviation so the partials stay locked and the grain keeps one pitch.
pub fn scan_water_grain(freq_hz: f64, sr: f64) -> Vec<f32> {
    let n = grain_len(sr, SCAN_GRAIN_MS);
    let t = dsp::arange_div(n, sr);
    let kv = 2.0 * PI * SCAN_WATER_VIBRATO_HZ;
    let depth = SCAN_WATER_VIBRATO_CENTS / 1200.0;
    let kf = 2.0 * PI * freq_hz;
    let phase: Vec<f64> = t
        .iter()
        .map(|&x| kf * x * (1.0 + depth * (kv * x).sin()))
        .collect();
    let mut w = vec![0.0f64; n];
    let nyq = 0.45 * sr;
    for k in 1..=SCAN_WATER_HARMONICS {
        if freq_hz * k as f64 >= nyq {
            break;
        }
        let kf = k as f64;
        for (v, p) in w.iter_mut().zip(&phase) {
            *v += (p * kf).sin() / kf;
        }
    }
    for (v, e) in w.iter_mut().zip(scan_grain_envelope(n, sr, 1.0)) {
        *v *= e;
    }
    normalise_to_f32(&w)
}

/// Fade in with a raised cosine and out with the cubic, over `fade` samples each end.
fn ping_edges(w: &mut [f64], fade: usize) {
    if fade > 0 {
        for (v, r) in w[..fade].iter_mut().zip(rc_rise(fade)) {
            *v *= r;
        }
        let n = w.len();
        for (v, d) in w[n - fade..].iter_mut().zip(cubic_decay(fade)) {
            *v *= d;
        }
    }
}

/// One spawned object. `kind` is the mod's p/v tag and selects LENGTH only: a prop
/// ticks, a vehicle rings.
pub fn scan_object_ping(freq_hz: f64, sr: f64, kind: &str) -> Vec<f32> {
    let ms = if kind == "p" {
        SCAN_PROP_PING_MS
    } else {
        SCAN_PING_MS
    };
    let n = grain_len(sr, ms);
    let t = dsp::arange_div(n, sr);
    let mut w = vec![0.0f64; n];
    let nyq = 0.45 * sr;
    for &k in SCAN_OBJECT_HARMONICS.iter() {
        if freq_hz * k as f64 >= nyq {
            break;
        }
        let kk = 2.0 * PI * freq_hz * k as f64;
        for (v, s) in w.iter_mut().zip(sin_kt(kk, &t)) {
            *v += s / k as f64;
        }
    }
    let dur = ms / 1000.0;
    for (v, &x) in w.iter_mut().zip(&t) {
        *v *= (-x * SCAN_OBJECT_DECAY / dur).exp();
    }
    let fade = ((sr * SCAN_GRAIN_FADE_MS / 1000.0) as usize).min(n / 2);
    ping_edges(&mut w, fade);
    normalise_to_f32(&w)
}

/// One map POI: the same short bell struck TWICE, both at the same pitch. Rhythm, not
/// timbre, makes it unmistakable, and pitch stays the elevation channel.
pub fn scan_poi_doublet(freq_hz: f64, sr: f64) -> Vec<f32> {
    let ping_n = grain_len(sr, SCAN_POI_PING_MS);
    let gap_n = (sr * SCAN_POI_GAP_MS / 1000.0) as usize;
    let t = dsp::arange_div(ping_n, sr);
    let mut one = vec![0.0f64; ping_n];
    let nyq = 0.45 * sr;
    for (i, &mult) in SCAN_POI_PARTIALS.iter().enumerate() {
        let f = freq_hz * mult;
        if f >= nyq {
            break;
        }
        let k = 2.0 * PI * f;
        let d = (i + 1) as f64;
        for (v, s) in one.iter_mut().zip(sin_kt(k, &t)) {
            *v += s / d;
        }
    }
    let dur = SCAN_POI_PING_MS / 1000.0;
    for (v, &x) in one.iter_mut().zip(&t) {
        *v *= (-x * SCAN_POI_DECAY / dur).exp();
    }
    let fade = ((sr * SCAN_GRAIN_FADE_MS / 1000.0) as usize).min(ping_n / 2);
    ping_edges(&mut one, fade);
    let mut w = vec![0.0f64; ping_n * 2 + gap_n];
    w[..ping_n].copy_from_slice(&one);
    w[ping_n + gap_n..].copy_from_slice(&one);
    normalise_to_f32(&w)
}

/// Middle C, centre, at the head of every scan, at a fixed level. Without it an
/// absolute-pitch elevation readout has nothing to be absolute against.
pub fn scan_reference_ping(sr: f64) -> Vec<f32> {
    let n = grain_len(sr, SCAN_REF_PING_MS);
    let t = dsp::arange_div(n, sr);
    let env = scan_envelope(n, sr, SCAN_REF_RELEASE_MS);
    let mut w: Vec<f64> = sin_kt(2.0 * PI * SCAN_MIDDLE_C_HZ, &t)
        .zip(&env)
        .map(|(s, e)| s * e)
        .collect();
    let peak = dsp::max_abs(&w);
    if peak > 0.0 {
        for v in w.iter_mut() {
            *v /= peak;
        }
    }
    let level = 10.0f64.powf(SCAN_REF_PING_DB / 20.0);
    w.iter().map(|&v| (v * level) as f32).collect()
}

/// One terrain/water sample: `(bearing_deg, range_m, dz_m, depth_m[, family])`.
///
/// `dz_m` `None` means no surface there, which renders as SILENCE (zero would be level
/// ground and would report a plateau where the map ends). `depth_m` present, even zero,
/// selects the water voice. `family` is a name from SCAN_FAMILY_CODES and optional:
/// absent, empty, unrecognised or paved all give the paved tone.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanSample {
    pub bearing_deg: f64,
    pub range_m: f64,
    pub dz_m: Option<f64>,
    pub depth_m: Option<f64>,
    pub family: Option<String>,
}

/// One object: `(bearing_deg, range_m, dz_m[, kind])`, kind `"p"`/`"v"`, defaulting to
/// `"v"` (a vehicle is the thing you must not hit).
#[derive(Debug, Clone, PartialEq)]
pub struct ScanObject {
    pub bearing_deg: f64,
    pub range_m: f64,
    pub dz_m: f64,
    pub kind: Option<String>,
}

/// One map POI: `(bearing_deg, range_m, dz_m)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanPoi {
    pub bearing_deg: f64,
    pub range_m: f64,
    pub dz_m: f64,
}

#[derive(Hash, PartialEq, Eq)]
enum CacheKey {
    Water(i64),
    Terrain(i64, String),
    Object(i64, String),
    Poi(i64),
}

/// Render one whole scan to a finished `(N, 2)` float32 buffer (`[left, right]` frames).
///
/// Pure: called from the listener thread, never the audio callback (six hundred grains
/// per block would blow the callback's budget for every voice in the mod). `Err` where
/// Python raises (a NaN or infinite range or elevation).
pub fn render_scan(
    samples: &[ScanSample],
    objects: &[ScanObject],
    reach_m: f64,
    sr: f64,
    level_db: f64,
    pois: &[ScanPoi],
) -> Result<Vec<[f32; 2]>, ScanError> {
    let reach = py_max(1.0, reach_m);
    let total_n = py_int(sr * (SCAN_REF_LEAD_S + SCAN_DURATION_S))? as usize
        + [
            (sr * SCAN_GRAIN_MS / 1000.0) as usize,
            (sr * SCAN_PING_MS / 1000.0) as usize,
            // The doublet is the longest single event whenever the grain is short, so it
            // belongs in this max even though it is not the longest today.
            (sr * (SCAN_POI_GAP_MS + 2.0 * SCAN_POI_PING_MS) / 1000.0) as usize,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
        + (sr * SCAN_TIME_JITTER_MS / 1000.0) as usize
        + 2;
    let mut cloud_l = vec![0.0f32; total_n];
    let mut cloud_r = vec![0.0f32; total_n];

    let mut rng = dsp::default_rng(SCAN_RNG_SEED as u128);
    let lead = (sr * SCAN_REF_LEAD_S) as i64;
    let span = sr * SCAN_DURATION_S;
    let jitter_n = 1i64.max((sr * SCAN_TIME_JITTER_MS / 1000.0) as i64);
    let mut cache: HashMap<CacheKey, Vec<f32>> = HashMap::new();

    let place = |wave: &[f32], start: i64, gain: f64, pan: f64, l: &mut [f32], r: &mut [f32]| {
        let start = start.max(0) as usize;
        if start >= total_n {
            // Python would slice the wave from the end here and fail to broadcast; no
            // onset can reach past the buffer (it is sized for the latest one).
            return;
        }
        let len = wave.len().min(total_n - start);
        if len == 0 {
            return;
        }
        let (lg, rg) = equal_power_pan(pan);
        // A Python float times a float32 array stays float32 (NEP 50).
        let gl = (gain * lg) as f32;
        let gr = (gain * rg) as f32;
        for i in 0..len {
            l[start + i] += wave[i] * gl;
            r[start + i] += wave[i] * gr;
        }
    };

    let water_gain = 10.0f64.powf(SCAN_WATER_DB_OFFSET / 20.0);
    let object_gain = 10.0f64.powf(SCAN_OBJECT_DB_OFFSET / 20.0);
    let poi_gain = 10.0f64.powf(SCAN_POI_DB_OFFSET / 20.0);

    let onset = |range_m: f64| -> Result<i64, ScanError> {
        let r = py_min(range_m, reach);
        Ok(lead + py_int((r / reach) * span)?)
    };

    for s in samples {
        let Some(dz_m) = s.dz_m else {
            continue; // no surface: silence, never a plateau at level
        };
        // The depth field is the wire-level water tag; its presence, not its magnitude,
        // selects the water voice.
        let is_water = s.depth_m.is_some();
        // Depth is authoritative over the surface family.
        let mut family = SCAN_PAVED.to_string();
        if !is_water {
            if let Some(fam) = s.family.as_deref().filter(|f| !f.is_empty()) {
                family = if SCAN_FAMILY_VOICES.contains(&fam) {
                    fam.to_string()
                } else {
                    // A code from a newer mod half: today's tone, not an error.
                    SCAN_PAVED.to_string()
                };
            }
        }
        let step = try_scan_step_from_dz(dz_m)?;
        let key = if is_water {
            CacheKey::Water(step)
        } else {
            CacheKey::Terrain(step, family.clone())
        };
        let wave = cache.entry(key).or_insert_with(|| {
            let hz = pitch_of_step(step, 0.0);
            if is_water {
                scan_water_grain(hz, sr)
            } else {
                scan_terrain_grain(hz, sr, &family)
            }
        });
        let mut t0 = onset(s.range_m)?;
        t0 += rng.integers(-jitter_n, jitter_n + 1);
        place(
            wave,
            t0,
            if is_water { water_gain } else { 1.0 },
            -s.bearing_deg / 90.0,
            &mut cloud_l,
            &mut cloud_r,
        );
    }

    for o in objects {
        let kind = match o.kind.as_deref() {
            Some("p") => "p",
            _ => "v",
        };
        let step = try_scan_step_from_dz(o.dz_m)?;
        let wave = cache
            .entry(CacheKey::Object(step, kind.to_string()))
            .or_insert_with(|| scan_object_ping(pitch_of_step(step, SCAN_OBJECT_OCTAVE), sr, kind));
        let t0 = onset(o.range_m)?;
        place(
            wave,
            t0,
            object_gain,
            -o.bearing_deg / 90.0,
            &mut cloud_l,
            &mut cloud_r,
        );
    }

    // POIs are placed like objects: sparse landmarks, not a surface, so jittering them
    // would only blur a position that is exactly known.
    for p in pois {
        let step = try_scan_step_from_dz(p.dz_m)?;
        let wave = cache
            .entry(CacheKey::Poi(step))
            .or_insert_with(|| scan_poi_doublet(pitch_of_step(step, SCAN_POI_OCTAVE), sr));
        let t0 = onset(p.range_m)?;
        place(
            wave,
            t0,
            poi_gain,
            -p.bearing_deg / 90.0,
            &mut cloud_l,
            &mut cloud_r,
        );
    }

    // Normalise the CLOUD, not the finished buffer: the reference ping is a calibration
    // and has to sit at a fixed level whatever the terrain did.
    let pl = dsp::max_abs_f32(&cloud_l);
    let pr = dsp::max_abs_f32(&cloud_r);
    // Python max(a, b): a unless b > a.
    let peak = py_max(pl as f64, pr as f64);
    if peak > 1e-9 {
        let scale = (10.0f64.powf(level_db / 20.0) / peak) as f32;
        for v in cloud_l.iter_mut().chain(cloud_r.iter_mut()) {
            *v *= scale;
        }
    }

    let ping = scan_reference_ping(sr);
    let n = ping.len().min(total_n);
    for i in 0..n {
        cloud_l[i] += ping[i];
        cloud_r[i] += ping[i];
    }

    Ok(cloud_l
        .into_iter()
        .zip(cloud_r)
        .map(|(l, r)| [l, r])
        .collect())
}
