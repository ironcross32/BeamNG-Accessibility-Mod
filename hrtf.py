# hrtf.py — HRTF loader and lookup for binaural compass clicks

import numpy as np


class HRTFSet:
    """Loads a SOFA file (MIT KEMAR) and provides interpolated HRIR lookup by azimuth."""

    def __init__(self, hrir_path, logger):
        self.logger = logger
        self._azimuths = None   # sorted 1-D float array of measured azimuths (degrees)
        self._irs_left = None   # shape (M, N) — M positions, N samples per IR
        self._irs_right = None
        self._samplerate = 44100.0  # native rate of the baked HRIR set

        # The horizontal-plane HRIRs are pre-extracted from the MIT KEMAR SOFA file
        # into a compressed .npz at build time (see bake_hrtf.py). This avoids
        # bundling h5py + the HDF5 native libraries (~6.5 MB) just to parse a static
        # data file at startup.
        try:
            with np.load(hrir_path) as data:
                self._azimuths = data["azimuths"].astype(np.float64)
                self._irs_left = data["irs_left"].astype(np.float32)
                self._irs_right = data["irs_right"].astype(np.float32)
                self._samplerate = float(data["samplerate"])

            logger.info(
                f"HRTF loaded: {len(self._azimuths)} positions, "
                f"{self._irs_left.shape[1]} samples/IR at {int(self._samplerate)} Hz"
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

        Linearly interpolates between the two nearest measured positions.
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
