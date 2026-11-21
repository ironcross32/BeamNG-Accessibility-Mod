# hrtf.py — HRTF loader and lookup for binaural compass clicks
#
# A baked set (bake_hrtf.py) holds a few rings of measurements around the
# horizontal plane. Loading turns that into ONE ring at the listener's chosen
# elevation, on a regular 1 degree azimuth grid, with all the expensive work done
# here once so get_hrir stays a cheap blend of two neighbouring entries:
#
#  1. Every measured HRIR is split into an interaural time difference (ITD) and a
#     magnitude response. Blending raw HRIRs whose onsets differ is what made the
#     old linear interpolation comb-filter between 5 degree measurements; blending
#     the delay and the magnitude separately does not.
#  2. Those are interpolated to each whole degree, within each measured ring and
#     then between the two rings either side of the requested elevation.
#  3. Left and right are made mirror images. Dummy heads are symmetric by design,
#     and what asymmetry the measurements carry is mostly head placement; left in,
#     it pulled "dead ahead" about 1 dB to one side on the KU100 and SADIE sets,
#     which matters to every cue whose job is to say "centred".
#  4. Optionally, the tone colour the ring shares in every direction (measurement
#     loudspeaker, microphones, the dummy head's ear canal) is equalised out, which
#     is what makes an unequalised set sound boxy or harsh on headphones. The cues
#     that differ between directions are what remain.
#  5. Each entry is rebuilt as a minimum-phase response delayed by its share of the
#     ITD, then the whole ring is level-matched to MIT KEMAR.
#
# Neighbouring whole-degree entries differ in delay by under half a sample, so the
# linear blend in get_hrir no longer comb-filters.

import numpy as np

import hrtf_sets

# Loudness matching across sets. Every cue's level in audio.py was tuned against the
# unprocessed MIT KEMAR set, and the sets differ by several dB as measured (KU100
# about 2 dB quieter, SADIE II about 6 dB), so switching would otherwise change the
# level of every spatialised cue at once. Each processed ring is scaled so its mean
# power response over all positions and both ears, in the band the cues live in,
# equals that of the original 72-position KEMAR ring.
LOUDNESS_BAND_HZ = (300.0, 8000.0)
LOUDNESS_FFT_SIZE = 4096
REFERENCE_BAND_ENERGY = 2.041760777370705

# Analysis / reconstruction
PROC_FFT_SIZE = 4096            # long enough that the cepstral min-phase step does not alias
ITD_LOWPASS_HZ = 1500.0         # ITD is read from the low band, where it is unambiguous
ITD_MAX_S = 0.001               # no human-sized head exceeds this
ITD_UPSAMPLE = 16               # cross-correlation resolution: 1/16 sample
MAG_FLOOR_DB = -100.0           # keeps log() finite in spectral nulls
OUT_BODY_S = 0.010              # min-phase length kept after the delay; MIT KEMAR's low end still holds -33 dB at 6 ms
OUT_LEAD_S = 0.0005             # headroom before the earliest arrival for the fractional delay's pre-ringing
OUT_FADE_FRACTION = 0.15        # half-Hann fade over the tail

# Equalisation (step 4)
EQ_SMOOTH_OCTAVES = 1.0 / 3.0
EQ_LIMIT_DB = 12.0
EQ_BAND_HZ = (150.0, 12000.0)   # outside this, hold the edge value; the measurements are unreliable there
                                # (MIT KEMAR asks for the full +12 dB at 16 kHz, which would only lift noise)

OUTPUT_AZIMUTHS = np.arange(360, dtype=np.float64)


def band_energy(irs, samplerate):
    """Mean power response of every row of irs within LOUDNESS_BAND_HZ.

    Independent of sample rate and IR length, because it averages |H(f)|^2 over
    frequency rather than summing samples.
    """
    n = max(LOUDNESS_FFT_SIZE, irs.shape[1])
    freqs = np.fft.rfftfreq(n, 1.0 / samplerate)
    band = (freqs >= LOUDNESS_BAND_HZ[0]) & (freqs <= LOUDNESS_BAND_HZ[1])
    spectra = np.abs(np.fft.rfft(irs.astype(np.float64), n=n, axis=1)) ** 2
    return float(spectra[:, band].mean())


def estimate_itd(irs_left, irs_right, samplerate, nfft=PROC_FFT_SIZE):
    """Interaural time difference per row, in seconds: right arrival minus left.

    Positive means the left ear hears it first (a source on the left). Read from
    the peak of the low-passed interaural cross-correlation, which works the same
    on minimum-phase and linear-phase measurements; onset thresholds do not, since
    a linear-phase set (SADIE II) rings before its main peak.
    """
    fl = np.fft.rfft(irs_left.astype(np.float64), n=nfft, axis=1)
    fr = np.fft.rfft(irs_right.astype(np.float64), n=nfft, axis=1)
    freqs = np.fft.rfftfreq(nfft, 1.0 / samplerate)
    # Raised-cosine roll-off from the cutoff to 1.5x it, so the correlation does not ring.
    taper = np.clip((1.5 * ITD_LOWPASS_HZ - freqs) / (0.5 * ITD_LOWPASS_HZ), 0.0, 1.0)
    taper = 0.5 - 0.5 * np.cos(np.pi * taper)
    n_up = nfft * ITD_UPSAMPLE
    xcorr = np.fft.irfft(fl * np.conj(fr) * taper, n=n_up, axis=1)
    max_lag = int(np.ceil(ITD_MAX_S * samplerate * ITD_UPSAMPLE))
    # Lags -max_lag..+max_lag, unwrapped from the circular result.
    window = np.concatenate([xcorr[:, n_up - max_lag:], xcorr[:, : max_lag + 1]], axis=1)
    peak = np.argmax(window, axis=1)
    # Parabolic refinement of the peak.
    rows = np.arange(window.shape[0])
    lo = window[rows, np.clip(peak - 1, 0, window.shape[1] - 1)]
    mid = window[rows, peak]
    hi = window[rows, np.clip(peak + 1, 0, window.shape[1] - 1)]
    denom = lo - 2.0 * mid + hi
    shift = np.where(np.abs(denom) > 1e-30, 0.5 * (lo - hi) / np.where(denom == 0, 1, denom), 0.0)
    lag_up = peak - max_lag + np.clip(shift, -0.5, 0.5)
    # xcorr[k] = sum L[n+k] R[n] peaks at k = -(delay_R - delay_L).
    return -lag_up / (ITD_UPSAMPLE * samplerate)


def magnitudes(irs, nfft=PROC_FFT_SIZE):
    return np.abs(np.fft.rfft(irs.astype(np.float64), n=nfft, axis=1))


def _interp_ring(ring_az, values, targets):
    """Linear interpolation around a circle: values (n, ...) at ring_az (sorted) -> targets."""
    n = len(ring_az)
    idx = np.searchsorted(ring_az, targets)
    hi = idx % n
    lo = (idx - 1) % n
    gap = (ring_az[hi] - ring_az[lo]) % 360.0
    gap = np.where(gap < 1e-9, 360.0, gap)  # a single-entry ring, or an exact wrap
    frac = ((targets - ring_az[lo]) % 360.0) / gap
    frac = frac.reshape((-1,) + (1,) * (values.ndim - 1))
    return (1.0 - frac) * values[lo] + frac * values[hi]


def _fractional_octave_smooth(power, freqs, octaves):
    """Average power over +/- octaves/2 around each bin."""
    half = 2.0 ** (octaves / 2.0)
    cums = np.concatenate([[0.0], np.cumsum(power)])
    lo = np.searchsorted(freqs, freqs / half, side="left")
    hi = np.searchsorted(freqs, freqs * half, side="right")
    hi = np.maximum(hi, lo + 1)
    return (cums[hi] - cums[lo]) / (hi - lo)


def equalisation_curve(mag_left, mag_right, samplerate, nfft=PROC_FFT_SIZE):
    """Gain per bin that flattens the power response shared by every direction.

    Smoothed to a third of an octave so only broad colouration goes, never the
    narrow direction-dependent notches; normalised to 0 dB on average across the
    loudness band (level matching happens afterwards anyway); limited to
    +/- EQ_LIMIT_DB so a measurement's weak extremes are not boosted into noise.
    """
    freqs = np.fft.rfftfreq(nfft, 1.0 / samplerate)
    power = 0.5 * (np.mean(mag_left ** 2, axis=0) + np.mean(mag_right ** 2, axis=0))
    smooth = _fractional_octave_smooth(power, freqs, EQ_SMOOTH_OCTAVES)
    eq_db = -10.0 * np.log10(np.maximum(smooth, 1e-20))
    band = (freqs >= LOUDNESS_BAND_HZ[0]) & (freqs <= LOUDNESS_BAND_HZ[1])
    eq_db -= eq_db[band].mean()
    eq_db = np.clip(eq_db, -EQ_LIMIT_DB, EQ_LIMIT_DB)
    lo_i = np.searchsorted(freqs, EQ_BAND_HZ[0])
    hi_i = min(np.searchsorted(freqs, EQ_BAND_HZ[1]), len(freqs) - 1)
    eq_db[:lo_i] = eq_db[lo_i]
    eq_db[hi_i:] = eq_db[hi_i]
    return 10.0 ** (eq_db / 20.0)


def rebuild(mag, delays_s, samplerate, nfft=PROC_FFT_SIZE):
    """Minimum-phase responses with the given magnitudes, each delayed by delays_s.

    The delay is applied as a linear phase on the min-phase spectrum, which is an
    exact band-limited fractional delay; OUT_LEAD_S of headroom (included in
    delays_s by the caller) holds its pre-ringing.
    """
    floor = np.max(mag, axis=1, keepdims=True) * (10.0 ** (MAG_FLOOR_DB / 20.0))
    log_mag = np.log(np.maximum(mag, np.maximum(floor, 1e-30)))
    cep = np.fft.irfft(log_mag, n=nfft, axis=1)
    fold = np.zeros(nfft)
    fold[0] = 1.0
    fold[1 : nfft // 2] = 2.0
    fold[nfft // 2] = 1.0
    min_phase = np.exp(np.fft.rfft(cep * fold, n=nfft, axis=1))
    freqs = np.fft.rfftfreq(nfft, 1.0 / samplerate)
    spectrum = min_phase * np.exp(-2j * np.pi * freqs[None, :] * delays_s[:, None])
    irs = np.fft.irfft(spectrum, n=nfft, axis=1)
    length = int(np.ceil((np.max(delays_s) + OUT_BODY_S) * samplerate))
    irs = irs[:, :length]
    fade = max(1, int(length * OUT_FADE_FRACTION))
    irs[:, length - fade :] *= 0.5 + 0.5 * np.cos(np.linspace(0.0, np.pi, fade))
    return irs.astype(np.float32)


def process_ring(azimuths, elevations, irs_left, irs_right, samplerate,
                 elevation_deg, equalise):
    """Steps 1-5 of the module comment. Returns (irs_left, irs_right) on OUTPUT_AZIMUTHS.

    elevation_deg is clamped to the measured rings; info returns what was used.
    """
    el_rounded = np.round(elevations, 1)
    rings = np.unique(el_rounded)
    target = float(np.clip(elevation_deg, rings[0], rings[-1]))
    upper_i = int(np.searchsorted(rings, target))
    if upper_i < len(rings) and abs(rings[upper_i] - target) < 0.05:
        pair = [(rings[upper_i], 1.0)]
    else:
        below, above = rings[upper_i - 1], rings[upper_i]
        w_above = (target - below) / (above - below)
        pair = [(below, 1.0 - w_above), (above, w_above)]

    bins = PROC_FFT_SIZE // 2 + 1
    mag_l = np.zeros((len(OUTPUT_AZIMUTHS), bins))
    mag_r = np.zeros((len(OUTPUT_AZIMUTHS), bins))
    itd = np.zeros(len(OUTPUT_AZIMUTHS))
    for ring_el, weight in pair:
        sel = np.flatnonzero(el_rounded == ring_el)
        order = sel[np.argsort(azimuths[sel] % 360.0)]
        ring_az = azimuths[order] % 360.0
        l, r = irs_left[order], irs_right[order]
        mag_l += weight * _interp_ring(ring_az, magnitudes(l), OUTPUT_AZIMUTHS)
        mag_r += weight * _interp_ring(ring_az, magnitudes(r), OUTPUT_AZIMUTHS)
        itd += weight * _interp_ring(ring_az, estimate_itd(l, r, samplerate), OUTPUT_AZIMUTHS)

    # Mirror: the left ear at azimuth a is the right ear at -a.
    mirror = (360 - np.arange(360)) % 360
    sym_l = 0.5 * (mag_l + mag_r[mirror])
    mag_r = sym_l[mirror]
    mag_l = sym_l
    itd = 0.5 * (itd - itd[mirror])

    if equalise:
        eq = equalisation_curve(mag_l, mag_r, samplerate)
        mag_l = mag_l * eq
        mag_r = mag_r * eq

    centre = OUT_LEAD_S + 0.5 * float(np.max(np.abs(itd)))
    out_l = rebuild(mag_l, centre - 0.5 * itd, samplerate)
    out_r = rebuild(mag_r, centre + 0.5 * itd, samplerate)
    info = {
        "elevation": target,
        "rings": [float(p[0]) for p in pair],
        "max_itd_us": 1e6 * float(np.max(np.abs(itd))),
    }
    return out_l, out_r, info


class HRTFSet:
    """Loads a baked HRIR set and provides interpolated lookup by azimuth on one elevation ring."""

    def __init__(self, hrir_path, logger, elevation_deg=None, equalise=None):
        self.logger = logger
        self._azimuths = None   # sorted 1-D float array of azimuths (degrees)
        self._irs_left = None   # shape (M, N) — M positions, N samples per IR
        self._irs_right = None
        self._samplerate = 44100.0  # native rate of the baked HRIR set
        if elevation_deg is None:
            elevation_deg = hrtf_sets.DEFAULT_ELEVATION_DEG
        if equalise is None:
            equalise = hrtf_sets.DEFAULT_EQUALISE

        # The HRIRs are pre-extracted from each SOFA file into a compressed .npz at
        # build time (see bake_hrtf.py). This avoids bundling h5py + the HDF5 native
        # libraries (~6.5 MB) just to parse a static data file at startup. A file
        # without "elevations" is a single horizontal ring.
        try:
            with np.load(hrir_path) as data:
                azimuths = data["azimuths"].astype(np.float64)
                if "elevations" in data.files:
                    elevations = data["elevations"].astype(np.float64)
                else:
                    elevations = np.zeros_like(azimuths)
                irs_left = data["irs_left"].astype(np.float64)
                irs_right = data["irs_right"].astype(np.float64)
                self._samplerate = float(data["samplerate"])

            out_l, out_r, info = process_ring(
                azimuths, elevations, irs_left, irs_right, self._samplerate,
                float(elevation_deg), bool(equalise),
            )

            energy = band_energy(np.concatenate([out_l, out_r]), self._samplerate)
            gain = float(np.sqrt(REFERENCE_BAND_ENERGY / max(energy, 1e-20)))
            self._irs_left = out_l * np.float32(gain)
            self._irs_right = out_r * np.float32(gain)
            self._azimuths = OUTPUT_AZIMUTHS.copy()

            logger.info(
                f"HRTF loaded from {hrir_path}: elevation {info['elevation']:+.1f} deg "
                f"(from rings {', '.join(f'{r:+g}' for r in info['rings'])}), "
                f"{'equalised' if equalise else 'unequalised'}, "
                f"max ITD {info['max_itd_us']:.0f} us, "
                f"{self._irs_left.shape[1]} samples/IR at {int(self._samplerate)} Hz, "
                f"level matched by {20.0 * np.log10(gain):+.1f} dB"
            )
        except Exception as e:
            logger.warning(f"Failed to load HRTF from '{hrir_path}': {e}")
            self._azimuths = None

    @property
    def is_loaded(self):
        return self._azimuths is not None

    def resample(self, target_sr):
        """FFT-based resample all IRs from native rate to target_sr."""
        if not self.is_loaded:
            return
        if abs(self._samplerate - target_sr) < 1.0:
            return  # already at target rate

        ratio = target_sr / self._samplerate
        old_n = self._irs_left.shape[1]
        new_n = max(1, int(round(old_n * ratio)))

        self._irs_left = self._fft_resample(self._irs_left, new_n)
        self._irs_right = self._fft_resample(self._irs_right, new_n)
        self._samplerate = target_sr

        self.logger.info(f"HRTF resampled: {old_n} -> {new_n} samples/IR at {int(target_sr)} Hz")

    @staticmethod
    def _fft_resample(irs, new_n):
        """Resample each row of irs (M×N) to new_n samples using FFT."""
        m, old_n = irs.shape
        out = np.zeros((m, new_n), dtype=np.float32)
        for i in range(m):
            spectrum = np.fft.rfft(irs[i])
            out[i] = np.fft.irfft(spectrum, n=new_n).astype(np.float32)
        return out

    def get_hrir(self, azimuth_deg):
        """Return (left_ir, right_ir) for the given SOFA azimuth (0°=front, 90°=left, CCW).

        Linearly interpolates between the two nearest whole-degree entries, which
        differ in delay by under half a sample, so the blend does not comb-filter.
        """
        if not self.is_loaded:
            return None, None

        az = azimuth_deg % 360.0
        azimuths = self._azimuths

        # Find insertion point
        idx = np.searchsorted(azimuths, az)
        n = len(azimuths)

        # Wrap-around indices
        idx_hi = idx % n
        idx_lo = (idx - 1) % n

        az_lo = azimuths[idx_lo]
        az_hi = azimuths[idx_hi]

        # Calculate fractional position between the two nearest measurements
        gap = (az_hi - az_lo) % 360.0
        if gap < 1e-6:
            # Exact match or degenerate — just return one
            return self._irs_left[idx_lo].copy(), self._irs_right[idx_lo].copy()

        frac = ((az - az_lo) % 360.0) / gap

        ir_l = (1.0 - frac) * self._irs_left[idx_lo] + frac * self._irs_left[idx_hi]
        ir_r = (1.0 - frac) * self._irs_right[idx_lo] + frac * self._irs_right[idx_hi]

        return ir_l, ir_r
