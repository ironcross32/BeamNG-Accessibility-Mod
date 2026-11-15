//! HRTF loader and lookup for binaural cues, from hrtf.py.
//!
//! A baked set (bake_hrtf.py) holds a few rings of measurements around the horizontal
//! plane. Loading turns that into ONE ring at the listener's chosen elevation, on a
//! regular 1 degree azimuth grid, with all the expensive work done once so `get_hrir`
//! stays a cheap blend of two neighbouring entries:
//!
//!  1. Every measured HRIR is split into an interaural time difference (ITD) and a
//!     magnitude response. Blending raw HRIRs whose onsets differ comb-filters between
//!     measurements; blending the delay and the magnitude separately does not.
//!  2. Those are interpolated to each whole degree, within each measured ring and then
//!     between the two rings either side of the requested elevation.
//!  3. Left and right are made mirror images (dummy heads are symmetric by design; the
//!     measured asymmetry pulled "dead ahead" about 1 dB to one side on KU100 and SADIE).
//!  4. Optionally, the tone colour shared by every direction is equalised out.
//!  5. Each entry is rebuilt as a minimum-phase response delayed by its share of the ITD,
//!     then the whole ring is level-matched to MIT KEMAR.
//!
//! dtypes follow the Python: analysis in float64, the processed ring float32, the
//! resample in float32 FFTs (numpy 2.x), and `get_hrir`'s blend in float32 (a Python
//! float times a float32 array stays float32 under NEP 50).

use std::path::Path;

use beam_base::hrtf_sets;
use beam_base::pyfmt::{py_f, py_g, py_mod};

use crate::dsp::{self, Complex64};
use crate::npy::Npz;

// Loudness matching across sets. Every cue's level in audio.py was tuned against the
// unprocessed MIT KEMAR set, and the sets differ by several dB as measured, so each
// processed ring is scaled so its mean power response over all positions and both ears,
// in the band the cues live in, equals that of the original 72-position KEMAR ring.
pub const LOUDNESS_BAND_HZ: (f64, f64) = (300.0, 8000.0);
pub const LOUDNESS_FFT_SIZE: usize = 4096;
pub const REFERENCE_BAND_ENERGY: f64 = 2.041760777370705;

// Analysis / reconstruction
/// Long enough that the cepstral min-phase step does not alias.
pub const PROC_FFT_SIZE: usize = 4096;
/// ITD is read from the low band, where it is unambiguous.
pub const ITD_LOWPASS_HZ: f64 = 1500.0;
/// No human-sized head exceeds this.
pub const ITD_MAX_S: f64 = 0.001;
/// Cross-correlation resolution: 1/16 sample.
pub const ITD_UPSAMPLE: usize = 16;
/// Keeps log() finite in spectral nulls.
pub const MAG_FLOOR_DB: f64 = -100.0;
/// Min-phase length kept after the delay; MIT KEMAR's low end still holds -33 dB at 6 ms.
pub const OUT_BODY_S: f64 = 0.010;
/// Headroom before the earliest arrival for the fractional delay's pre-ringing.
pub const OUT_LEAD_S: f64 = 0.0005;
/// Half-Hann fade over the tail.
pub const OUT_FADE_FRACTION: f64 = 0.15;

// Equalisation (step 4)
pub const EQ_SMOOTH_OCTAVES: f64 = 1.0 / 3.0;
pub const EQ_LIMIT_DB: f64 = 12.0;
/// Outside this, hold the edge value; the measurements are unreliable there (MIT KEMAR
/// asks for the full +12 dB at 16 kHz, which would only lift noise).
pub const EQ_BAND_HZ: (f64, f64) = (150.0, 12000.0);

/// `OUTPUT_AZIMUTHS`: 0..359 degrees.
pub const N_OUTPUT_AZIMUTHS: usize = 360;

pub fn output_azimuths() -> Vec<f64> {
    dsp::arange(N_OUTPUT_AZIMUTHS)
}

/// Rows of IRs or spectra, each a `Vec`. Python's 2-D arrays.
pub type Rows = Vec<Vec<f64>>;

fn band_mask(freqs: &[f64], lo: f64, hi: f64) -> Vec<bool> {
    freqs.iter().map(|&f| f >= lo && f <= hi).collect()
}

/// Mean power response of every row of `irs` within `LOUDNESS_BAND_HZ`.
///
/// Independent of sample rate and IR length, because it averages |H(f)|^2 over frequency
/// rather than summing samples.
pub fn band_energy(irs: &[Vec<f64>], samplerate: f64) -> f64 {
    let ncols = irs.iter().map(Vec::len).max().unwrap_or(0);
    let n = LOUDNESS_FFT_SIZE.max(ncols);
    let freqs = dsp::rfftfreq(n, 1.0 / samplerate);
    let band = band_mask(&freqs, LOUDNESS_BAND_HZ.0, LOUDNESS_BAND_HZ.1);
    let mut flat = Vec::new();
    for row in irs {
        let spec = dsp::rfft(row, n);
        for (z, &b) in spec.iter().zip(&band) {
            if b {
                let a = z.norm();
                flat.push(a * a);
            }
        }
    }
    dsp::mean(&flat)
}

/// [`band_energy`] of float32 rows (`irs.astype(np.float64)` first, as Python does).
pub fn band_energy_f32(irs: &[Vec<f32>], samplerate: f64) -> f64 {
    let rows: Rows = irs
        .iter()
        .map(|r| r.iter().map(|&v| v as f64).collect())
        .collect();
    band_energy(&rows, samplerate)
}

/// Interaural time difference per row, in seconds: right arrival minus left.
///
/// Positive means the left ear hears it first (a source on the left). Read from the peak
/// of the low-passed interaural cross-correlation, which works the same on minimum-phase
/// and linear-phase measurements; onset thresholds do not, since a linear-phase set
/// (SADIE II) rings before its main peak.
pub fn estimate_itd(irs_left: &[Vec<f64>], irs_right: &[Vec<f64>], samplerate: f64) -> Vec<f64> {
    estimate_itd_n(irs_left, irs_right, samplerate, PROC_FFT_SIZE)
}

pub fn estimate_itd_n(
    irs_left: &[Vec<f64>],
    irs_right: &[Vec<f64>],
    samplerate: f64,
    nfft: usize,
) -> Vec<f64> {
    let freqs = dsp::rfftfreq(nfft, 1.0 / samplerate);
    // Raised-cosine roll-off from the cutoff to 1.5x it, so the correlation does not ring.
    let taper: Vec<f64> = freqs
        .iter()
        .map(|&f| {
            let t = dsp::clip(
                (1.5 * ITD_LOWPASS_HZ - f) / (0.5 * ITD_LOWPASS_HZ),
                0.0,
                1.0,
            );
            0.5 - 0.5 * (std::f64::consts::PI * t).cos()
        })
        .collect();
    let n_up = nfft * ITD_UPSAMPLE;
    let max_lag = (ITD_MAX_S * samplerate * ITD_UPSAMPLE as f64).ceil() as usize;
    irs_left
        .iter()
        .zip(irs_right)
        .map(|(l, r)| {
            let fl = dsp::rfft(l, nfft);
            let fr = dsp::rfft(r, nfft);
            let prod: Vec<Complex64> = fl
                .iter()
                .zip(&fr)
                .zip(&taper)
                .map(|((a, b), &t)| a * b.conj() * t)
                .collect();
            let xcorr = dsp::irfft(&prod, n_up);
            // Lags -max_lag..+max_lag, unwrapped from the circular result.
            let mut window = Vec::with_capacity(2 * max_lag + 1);
            window.extend_from_slice(&xcorr[n_up - max_lag..]);
            window.extend_from_slice(&xcorr[..max_lag + 1]);
            let peak = dsp::argmax(&window);
            let last = window.len() - 1;
            // Parabolic refinement of the peak.
            let lo = window[peak.saturating_sub(1)];
            let mid = window[peak];
            let hi = window[(peak + 1).min(last)];
            let denom = lo - 2.0 * mid + hi;
            let shift = if denom.abs() > 1e-30 {
                0.5 * (lo - hi) / if denom == 0.0 { 1.0 } else { denom }
            } else {
                0.0
            };
            let lag_up = (peak as f64 - max_lag as f64) + dsp::clip(shift, -0.5, 0.5);
            // xcorr[k] = sum L[n+k] R[n] peaks at k = -(delay_R - delay_L).
            -lag_up / (ITD_UPSAMPLE as f64 * samplerate)
        })
        .collect()
}

/// `|rfft(row, nfft)|` for every row.
pub fn magnitudes(irs: &[Vec<f64>]) -> Rows {
    magnitudes_n(irs, PROC_FFT_SIZE)
}

pub fn magnitudes_n(irs: &[Vec<f64>], nfft: usize) -> Rows {
    irs.iter()
        .map(|row| dsp::rfft(row, nfft).iter().map(|z| z.norm()).collect())
        .collect()
}

/// Linear interpolation around a circle: `values` rows at `ring_az` (sorted) -> `targets`.
fn interp_ring_rows(ring_az: &[f64], values: &[Vec<f64>], targets: &[f64]) -> Rows {
    let n = ring_az.len() as i64;
    targets
        .iter()
        .map(|&t| {
            let (lo, hi, frac) = interp_ring_index(ring_az, n, t);
            values[lo]
                .iter()
                .zip(&values[hi])
                .map(|(&a, &b)| (1.0 - frac) * a + frac * b)
                .collect()
        })
        .collect()
}

fn interp_ring_scalar(ring_az: &[f64], values: &[f64], targets: &[f64]) -> Vec<f64> {
    let n = ring_az.len() as i64;
    targets
        .iter()
        .map(|&t| {
            let (lo, hi, frac) = interp_ring_index(ring_az, n, t);
            (1.0 - frac) * values[lo] + frac * values[hi]
        })
        .collect()
}

fn interp_ring_index(ring_az: &[f64], n: i64, t: f64) -> (usize, usize, f64) {
    let idx = dsp::searchsorted_left(ring_az, t) as i64;
    let hi = idx.rem_euclid(n) as usize;
    let lo = (idx - 1).rem_euclid(n) as usize;
    let mut gap = py_mod(ring_az[hi] - ring_az[lo], 360.0);
    if gap < 1e-9 {
        gap = 360.0; // a single-entry ring, or an exact wrap
    }
    let frac = py_mod(t - ring_az[lo], 360.0) / gap;
    (lo, hi, frac)
}

/// Average power over +/- octaves/2 around each bin.
fn fractional_octave_smooth(power: &[f64], freqs: &[f64], octaves: f64) -> Vec<f64> {
    let half = 2.0f64.powf(octaves / 2.0);
    let mut cums = Vec::with_capacity(power.len() + 1);
    cums.push(0.0);
    cums.extend(dsp::cumsum(power));
    freqs
        .iter()
        .map(|&f| {
            let lo = dsp::searchsorted_left(freqs, f / half);
            let hi = dsp::searchsorted_right(freqs, f * half).max(lo + 1);
            (cums[hi] - cums[lo]) / (hi - lo) as f64
        })
        .collect()
}

/// Column means of rows (`np.mean(x, axis=0)`): numpy accumulates row by row.
fn column_mean_sq(rows: &[Vec<f64>]) -> Vec<f64> {
    let m = rows[0].len();
    let mut acc = vec![0.0f64; m];
    for row in rows {
        for (a, &v) in acc.iter_mut().zip(row) {
            *a += v * v;
        }
    }
    let n = rows.len() as f64;
    acc.iter().map(|a| a / n).collect()
}

/// Gain per bin that flattens the power response shared by every direction.
///
/// Smoothed to a third of an octave so only broad colouration goes, never the narrow
/// direction-dependent notches; normalised to 0 dB on average across the loudness band
/// (level matching happens afterwards anyway); limited to +/- EQ_LIMIT_DB so a
/// measurement's weak extremes are not boosted into noise.
pub fn equalisation_curve(
    mag_left: &[Vec<f64>],
    mag_right: &[Vec<f64>],
    samplerate: f64,
) -> Vec<f64> {
    let freqs = dsp::rfftfreq(PROC_FFT_SIZE, 1.0 / samplerate);
    let pl = column_mean_sq(mag_left);
    let pr = column_mean_sq(mag_right);
    let power: Vec<f64> = pl.iter().zip(&pr).map(|(a, b)| 0.5 * (a + b)).collect();
    let smooth = fractional_octave_smooth(&power, &freqs, EQ_SMOOTH_OCTAVES);
    let mut eq_db: Vec<f64> = smooth
        .iter()
        .map(|&s| -10.0 * (if s > 1e-20 || s.is_nan() { s } else { 1e-20 }).log10())
        .collect();
    let band = band_mask(&freqs, LOUDNESS_BAND_HZ.0, LOUDNESS_BAND_HZ.1);
    let in_band: Vec<f64> = eq_db
        .iter()
        .zip(&band)
        .filter(|(_, b)| **b)
        .map(|(v, _)| *v)
        .collect();
    let m = dsp::mean(&in_band);
    for v in eq_db.iter_mut() {
        *v = dsp::clip(*v - m, -EQ_LIMIT_DB, EQ_LIMIT_DB);
    }
    let lo_i = dsp::searchsorted_left(&freqs, EQ_BAND_HZ.0);
    let hi_i = dsp::searchsorted_left(&freqs, EQ_BAND_HZ.1).min(freqs.len() - 1);
    let lo_v = eq_db[lo_i];
    let hi_v = eq_db[hi_i];
    for v in eq_db[..lo_i].iter_mut() {
        *v = lo_v;
    }
    for v in eq_db[hi_i..].iter_mut() {
        *v = hi_v;
    }
    eq_db.iter().map(|&d| 10.0f64.powf(d / 20.0)).collect()
}

/// Minimum-phase responses with the given magnitudes, each delayed by `delays_s`.
///
/// The delay is applied as a linear phase on the min-phase spectrum, which is an exact
/// band-limited fractional delay; OUT_LEAD_S of headroom (included in `delays_s` by the
/// caller) holds its pre-ringing.
pub fn rebuild(mag: &[Vec<f64>], delays_s: &[f64], samplerate: f64) -> Vec<Vec<f32>> {
    let nfft = PROC_FFT_SIZE;
    let mut fold = vec![0.0f64; nfft];
    fold[0] = 1.0;
    for v in fold[1..nfft / 2].iter_mut() {
        *v = 2.0;
    }
    fold[nfft / 2] = 1.0;
    let freqs = dsp::rfftfreq(nfft, 1.0 / samplerate);
    let max_delay = dsp::max(delays_s);
    let length = ((max_delay + OUT_BODY_S) * samplerate).ceil() as usize;
    let fade = 1usize.max((length as f64 * OUT_FADE_FRACTION) as usize);
    let fade_win: Vec<f64> = dsp::linspace(0.0, std::f64::consts::PI, fade, true)
        .into_iter()
        .map(|x| 0.5 + 0.5 * x.cos())
        .collect();
    let floor_mult = 10.0f64.powf(MAG_FLOOR_DB / 20.0);
    // `-2j * np.pi`: (-0.0, -2.0) times pi, whose imaginary part is what reaches the phase.
    let neg_two_pi = -2.0 * std::f64::consts::PI;

    mag.iter()
        .zip(delays_s)
        .map(|(row, &delay)| {
            let floor = dsp::max(row) * floor_mult;
            let fl = if floor > 1e-30 || floor.is_nan() {
                floor
            } else {
                1e-30
            };
            let log_mag: Vec<Complex64> = row
                .iter()
                .map(|&m| Complex64::new((if m > fl || m.is_nan() { m } else { fl }).ln(), 0.0))
                .collect();
            let cep = dsp::irfft(&log_mag, nfft);
            let folded: Vec<f64> = cep.iter().zip(&fold).map(|(c, w)| c * w).collect();
            let spec = dsp::rfft(&folded, nfft);
            let spectrum: Vec<Complex64> = spec
                .iter()
                .zip(&freqs)
                .map(|(z, &f)| {
                    let min_phase = z.exp();
                    let theta = neg_two_pi * f * delay;
                    min_phase * Complex64::new(theta.cos(), theta.sin())
                })
                .collect();
            let irs = dsp::irfft(&spectrum, nfft);
            let mut out: Vec<f64> = irs[..length.min(nfft)].to_vec();
            let start = out.len() - fade.min(out.len());
            for (v, w) in out[start..].iter_mut().zip(&fade_win) {
                *v *= w;
            }
            dsp::to_f32(&out)
        })
        .collect()
}

/// What `process_ring` used: Python's `info` dict.
#[derive(Debug, Clone, PartialEq)]
pub struct RingInfo {
    pub elevation: f64,
    pub rings: Vec<f64>,
    pub max_itd_us: f64,
}

/// The intermediate products of [`process_ring`], for tests and diagnostics.
#[derive(Debug, Clone)]
pub struct RingProducts {
    pub itd: Vec<f64>,
    pub eq: Option<Vec<f64>>,
}

/// `np.round(x, 1)`: scale by 10, round half to even, divide.
fn np_round1(x: f64) -> f64 {
    (x * 10.0).round_ties_even() / 10.0
}

/// Steps 1-5 of the module comment. Returns `(irs_left, irs_right, info)` on the 360
/// output azimuths. `elevation_deg` is clamped to the measured rings; `info` returns
/// what was used.
pub fn process_ring(
    azimuths: &[f64],
    elevations: &[f64],
    irs_left: &[Vec<f64>],
    irs_right: &[Vec<f64>],
    samplerate: f64,
    elevation_deg: f64,
    equalise: bool,
) -> (Vec<Vec<f32>>, Vec<Vec<f32>>, RingInfo) {
    let (l, r, info, _) = process_ring_detail(
        azimuths,
        elevations,
        irs_left,
        irs_right,
        samplerate,
        elevation_deg,
        equalise,
    );
    (l, r, info)
}

/// [`process_ring`] that also returns the ITD curve and EQ curve.
pub fn process_ring_detail(
    azimuths: &[f64],
    elevations: &[f64],
    irs_left: &[Vec<f64>],
    irs_right: &[Vec<f64>],
    samplerate: f64,
    elevation_deg: f64,
    equalise: bool,
) -> (Vec<Vec<f32>>, Vec<Vec<f32>>, RingInfo, RingProducts) {
    let el_rounded: Vec<f64> = elevations.iter().map(|&e| np_round1(e)).collect();
    let mut rings = el_rounded.clone();
    rings.sort_by(|a, b| a.total_cmp(b));
    rings.dedup_by(|a, b| a == b);
    let first = rings[0];
    let last = rings[rings.len() - 1];
    let target = dsp::clip(elevation_deg, first, last);
    let upper_i = dsp::searchsorted_left(&rings, target);
    let pair: Vec<(f64, f64)> = if upper_i < rings.len() && (rings[upper_i] - target).abs() < 0.05 {
        vec![(rings[upper_i], 1.0)]
    } else {
        let below = rings[upper_i - 1];
        let above = rings[upper_i];
        let w_above = (target - below) / (above - below);
        vec![(below, 1.0 - w_above), (above, w_above)]
    };

    let out_az = output_azimuths();
    let bins = PROC_FFT_SIZE / 2 + 1;
    let mut mag_l = vec![vec![0.0f64; bins]; N_OUTPUT_AZIMUTHS];
    let mut mag_r = vec![vec![0.0f64; bins]; N_OUTPUT_AZIMUTHS];
    let mut itd = vec![0.0f64; N_OUTPUT_AZIMUTHS];
    for &(ring_el, weight) in &pair {
        let mut sel: Vec<usize> = (0..el_rounded.len())
            .filter(|&i| el_rounded[i] == ring_el)
            .collect();
        sel.sort_by(|&a, &b| py_mod(azimuths[a], 360.0).total_cmp(&py_mod(azimuths[b], 360.0)));
        let ring_az: Vec<f64> = sel.iter().map(|&i| py_mod(azimuths[i], 360.0)).collect();
        let l: Rows = sel.iter().map(|&i| irs_left[i].clone()).collect();
        let r: Rows = sel.iter().map(|&i| irs_right[i].clone()).collect();
        let il = interp_ring_rows(&ring_az, &magnitudes(&l), &out_az);
        let ir = interp_ring_rows(&ring_az, &magnitudes(&r), &out_az);
        let it = interp_ring_scalar(&ring_az, &estimate_itd(&l, &r, samplerate), &out_az);
        for (acc, row) in mag_l.iter_mut().zip(&il) {
            for (a, v) in acc.iter_mut().zip(row) {
                *a += weight * v;
            }
        }
        for (acc, row) in mag_r.iter_mut().zip(&ir) {
            for (a, v) in acc.iter_mut().zip(row) {
                *a += weight * v;
            }
        }
        for (a, v) in itd.iter_mut().zip(&it) {
            *a += weight * v;
        }
    }

    // Mirror: the left ear at azimuth a is the right ear at -a.
    let mirror: Vec<usize> = (0..N_OUTPUT_AZIMUTHS)
        .map(|i| (N_OUTPUT_AZIMUTHS - i) % N_OUTPUT_AZIMUTHS)
        .collect();
    let sym_l: Rows = (0..N_OUTPUT_AZIMUTHS)
        .map(|i| {
            mag_l[i]
                .iter()
                .zip(&mag_r[mirror[i]])
                .map(|(a, b)| 0.5 * (a + b))
                .collect()
        })
        .collect();
    let mut mag_r: Rows = mirror.iter().map(|&m| sym_l[m].clone()).collect();
    let mut mag_l = sym_l;
    let itd: Vec<f64> = (0..N_OUTPUT_AZIMUTHS)
        .map(|i| 0.5 * (itd[i] - itd[mirror[i]]))
        .collect();

    let mut eq_out = None;
    if equalise {
        let eq = equalisation_curve(&mag_l, &mag_r, samplerate);
        for row in mag_l.iter_mut().chain(mag_r.iter_mut()) {
            for (v, g) in row.iter_mut().zip(&eq) {
                *v *= g;
            }
        }
        eq_out = Some(eq);
    }

    let max_abs_itd = dsp::max_abs(&itd);
    let centre = OUT_LEAD_S + 0.5 * max_abs_itd;
    let dl: Vec<f64> = itd.iter().map(|&t| centre - 0.5 * t).collect();
    let dr: Vec<f64> = itd.iter().map(|&t| centre + 0.5 * t).collect();
    let out_l = rebuild(&mag_l, &dl, samplerate);
    let out_r = rebuild(&mag_r, &dr, samplerate);
    let info = RingInfo {
        elevation: target,
        rings: pair.iter().map(|p| p.0).collect(),
        max_itd_us: 1e6 * max_abs_itd,
    };
    (out_l, out_r, info, RingProducts { itd, eq: eq_out })
}

/// `{x:+g}`.
fn plus_g(x: f64) -> String {
    let s = py_g(x, 6);
    if s.starts_with('-') {
        s
    } else {
        format!("+{s}")
    }
}

/// `{x:+.1f}`.
fn plus_f1(x: f64) -> String {
    let s = py_f(x, 1);
    if s.starts_with('-') {
        s
    } else {
        format!("+{s}")
    }
}

/// A baked HRIR set processed into one ring, with interpolated lookup by azimuth.
#[derive(Debug, Clone)]
pub struct HrtfSet {
    azimuths: Option<Vec<f64>>,
    irs_left: Vec<Vec<f32>>,
    irs_right: Vec<Vec<f32>>,
    samplerate: f64,
    /// The log line Python writes on success (`logger.info`) or failure
    /// (`logger.warning`); also emitted through the `log` facade by [`HrtfSet::load`].
    pub load_message: String,
    pub info: Option<RingInfo>,
    /// The level-match gain applied (linear).
    pub gain: f64,
}

impl HrtfSet {
    /// `HRTFSet(hrir_path, logger, elevation_deg, equalise)`. `None` takes the
    /// `hrtf_sets` defaults. A set that fails to load is returned unloaded
    /// (`is_loaded() == false`) with the warning logged, as Python does.
    pub fn load(hrir_path: &Path, elevation_deg: Option<f64>, equalise: Option<bool>) -> HrtfSet {
        let set = Self::load_quiet(hrir_path, elevation_deg, equalise);
        if set.is_loaded() {
            log::info!("{}", set.load_message);
        } else {
            log::warn!("{}", set.load_message);
        }
        set
    }

    /// [`HrtfSet::load`] without logging.
    pub fn load_quiet(
        hrir_path: &Path,
        elevation_deg: Option<f64>,
        equalise: Option<bool>,
    ) -> HrtfSet {
        let elevation_deg = elevation_deg.unwrap_or(hrtf_sets::DEFAULT_ELEVATION_DEG);
        let equalise = equalise.unwrap_or(hrtf_sets::DEFAULT_EQUALISE);
        let mut set = HrtfSet {
            azimuths: None,
            irs_left: Vec::new(),
            irs_right: Vec::new(),
            samplerate: 44100.0,
            load_message: String::new(),
            info: None,
            gain: 1.0,
        };
        match Self::try_load(hrir_path, elevation_deg, equalise) {
            Ok((l, r, sr, info, gain)) => {
                set.samplerate = sr;
                set.load_message = format!(
                    "HRTF loaded from {}: elevation {} deg (from rings {}), {}, max ITD {} us, {} samples/IR at {} Hz, level matched by {} dB",
                    hrir_path.display(),
                    plus_f1(info.elevation),
                    info.rings
                        .iter()
                        .map(|r| plus_g(*r))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if equalise { "equalised" } else { "unequalised" },
                    py_f(info.max_itd_us, 0),
                    l.first().map_or(0, Vec::len),
                    sr.trunc() as i64,
                    plus_f1(20.0 * gain.log10()),
                );
                set.irs_left = l;
                set.irs_right = r;
                set.azimuths = Some(output_azimuths());
                set.info = Some(info);
                set.gain = gain;
            }
            Err((sr, e)) => {
                set.samplerate = sr;
                set.load_message =
                    format!("Failed to load HRTF from '{}': {}", hrir_path.display(), e);
            }
        }
        set
    }

    #[allow(clippy::type_complexity)]
    fn try_load(
        path: &Path,
        elevation_deg: f64,
        equalise: bool,
    ) -> Result<(Vec<Vec<f32>>, Vec<Vec<f32>>, f64, RingInfo, f64), (f64, String)> {
        let data = Npz::open(path).map_err(|e| (44100.0, e.to_string()))?;
        let load = || -> Result<_, String> {
            let azimuths = data.get("azimuths").map_err(|e| e.to_string())?.to_f64();
            let elevations = if data.contains("elevations") {
                data.get("elevations").map_err(|e| e.to_string())?.to_f64()
            } else {
                vec![0.0; azimuths.len()]
            };
            let irs_left = data
                .get("irs_left")
                .map_err(|e| e.to_string())?
                .rows_f64()
                .map_err(|e| e.to_string())?;
            let irs_right = data
                .get("irs_right")
                .map_err(|e| e.to_string())?
                .rows_f64()
                .map_err(|e| e.to_string())?;
            let sr = data
                .get("samplerate")
                .map_err(|e| e.to_string())?
                .scalar_f64()
                .map_err(|e| e.to_string())?;
            Ok((azimuths, elevations, irs_left, irs_right, sr))
        };
        let (az, el, l, r, sr) = load().map_err(|e| (44100.0, e))?;
        if az.is_empty() || l.len() != az.len() || r.len() != az.len() || el.len() != az.len() {
            return Err((sr, "HRIR arrays do not agree in length".to_string()));
        }
        let (out_l, out_r, info) = process_ring(&az, &el, &l, &r, sr, elevation_deg, equalise);
        let mut both = out_l.clone();
        both.extend(out_r.iter().cloned());
        let energy = band_energy_f32(&both, sr);
        let gain = (REFERENCE_BAND_ENERGY / if energy > 1e-20 { energy } else { 1e-20 }).sqrt();
        let g32 = gain as f32;
        let scale = |rows: Vec<Vec<f32>>| -> Vec<Vec<f32>> {
            rows.into_iter()
                .map(|row| row.into_iter().map(|v| v * g32).collect())
                .collect()
        };
        Ok((scale(out_l), scale(out_r), sr, info, gain))
    }

    /// A loaded set built directly from finished rows, one per whole degree (0..359), with
    /// no processing. For tests that stand in a known IR (the Python sims' `FakeHRTF`).
    /// Returns `None` unless both sides have exactly 360 rows.
    pub fn from_rows(
        irs_left: Vec<Vec<f32>>,
        irs_right: Vec<Vec<f32>>,
        samplerate: f64,
    ) -> Option<HrtfSet> {
        if irs_left.len() != N_OUTPUT_AZIMUTHS || irs_right.len() != N_OUTPUT_AZIMUTHS {
            return None;
        }
        Some(HrtfSet {
            azimuths: Some(output_azimuths()),
            irs_left,
            irs_right,
            samplerate,
            load_message: String::new(),
            info: None,
            gain: 1.0,
        })
    }

    pub fn is_loaded(&self) -> bool {
        self.azimuths.is_some()
    }

    pub fn samplerate(&self) -> f64 {
        self.samplerate
    }

    /// The whole processed ring, `(left, right)`, one row per whole degree.
    pub fn irs(&self) -> (&[Vec<f32>], &[Vec<f32>]) {
        (&self.irs_left, &self.irs_right)
    }

    /// Samples per IR.
    pub fn ir_len(&self) -> usize {
        self.irs_left.first().map_or(0, Vec::len)
    }

    /// FFT-based resample of every IR from the current rate to `target_sr`. Returns the
    /// log line Python writes, when it resampled.
    pub fn resample(&mut self, target_sr: f64) -> Option<String> {
        if !self.is_loaded() {
            return None;
        }
        if (self.samplerate - target_sr).abs() < 1.0 {
            return None; // already at target rate
        }
        let ratio = target_sr / self.samplerate;
        let old_n = self.ir_len();
        let new_n = 1usize.max((old_n as f64 * ratio).round_ties_even() as usize);
        self.irs_left = Self::fft_resample(&self.irs_left, new_n);
        self.irs_right = Self::fft_resample(&self.irs_right, new_n);
        self.samplerate = target_sr;
        let msg = format!(
            "HRTF resampled: {old_n} -> {new_n} samples/IR at {} Hz",
            target_sr.trunc() as i64
        );
        log::info!("{msg}");
        Some(msg)
    }

    /// Resample each row to `new_n` samples using float32 FFTs, as numpy 2.x does for a
    /// float32 array.
    pub fn fft_resample(irs: &[Vec<f32>], new_n: usize) -> Vec<Vec<f32>> {
        irs.iter()
            .map(|row| {
                let spectrum = dsp::rfft_f32(row, row.len().max(1));
                dsp::irfft_f32(&spectrum, new_n)
            })
            .collect()
    }

    /// `(left_ir, right_ir)` for the given SOFA azimuth (0 = front, 90 = left, CCW), or
    /// `None` when not loaded.
    ///
    /// Linearly interpolates between the two nearest whole-degree entries, which differ
    /// in delay by under half a sample, so the blend does not comb-filter. The result is
    /// float64, as in Python: `frac` is a numpy float64 scalar, which promotes the float32
    /// rows.
    pub fn get_hrir(&self, azimuth_deg: f64) -> Option<(Vec<f64>, Vec<f64>)> {
        let azimuths = self.azimuths.as_ref()?;
        let az = py_mod(azimuth_deg, 360.0);
        let idx = dsp::searchsorted_left(azimuths, az) as i64;
        let n = azimuths.len() as i64;
        let idx_hi = idx.rem_euclid(n) as usize;
        let idx_lo = (idx - 1).rem_euclid(n) as usize;
        let az_lo = azimuths[idx_lo];
        let az_hi = azimuths[idx_hi];
        let gap = py_mod(az_hi - az_lo, 360.0);
        let widen = |row: &[f32]| -> Vec<f64> { row.iter().map(|&v| v as f64).collect() };
        if gap < 1e-6 {
            // Exact match or degenerate: just return one. (Unreachable on the whole-degree
            // ring, where every gap is 1; Python returns float32 copies here.)
            return Some((
                widen(&self.irs_left[idx_lo]),
                widen(&self.irs_right[idx_lo]),
            ));
        }
        let frac = py_mod(az - az_lo, 360.0) / gap;
        let a = 1.0 - frac;
        let blend = |lo: &[f32], hi: &[f32]| -> Vec<f64> {
            lo.iter()
                .zip(hi)
                .map(|(&x, &y)| a * x as f64 + frac * y as f64)
                .collect()
        };
        Some((
            blend(&self.irs_left[idx_lo], &self.irs_left[idx_hi]),
            blend(&self.irs_right[idx_lo], &self.irs_right[idx_hi]),
        ))
    }
}
