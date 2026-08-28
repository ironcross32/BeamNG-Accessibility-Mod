# scanner_hrtf_diag.py — Correlated diagnostic logger for the vehicle scanner + HRTF chain.
#
# Purpose
# -------
# The vehicle scanner beep is meant to pitch up as a target moves toward dead-ahead
# (bearing -> 0) and, simultaneously, the HRTF should place the image dead-center.
# Users report that the pitch peak and the centered image do not always coincide:
# at highest pitch the sound sometimes sits slightly to the left, and the bearing at
# which the pitch starts climbing differs depending on turn direction.
#
# Reading the code, the *math* says peak pitch (bearing == 0) should land at HRTF
# azimuth 0 == dead front. So the discrepancy must come from runtime behaviour that a
# static read can't see:
#   * HRTF overlap-add tail smear across per-frame bearing changes,
#   * intrinsic L/R asymmetry of the measured KEMAR front impulse response,
#   * the reported bearing's zero not actually lining up with perceived "facing".
#
# This module records, per scanner beep frame, the raw bearing, the pitch the beep
# was played at, the HRTF azimuth used, the HRTF's *intrinsic* interaural level
# difference for that azimuth, and the *actual* interaural level difference of the
# convolved output. Correlating these tells us whether:
#   - pitch_mult truly peaks at bearing 0 (it should),
#   - the HRTF places bearing 0 at the acoustic center (out_ild_db ~ 0),
#   - the IR itself is balanced at front (ir_ild_db ~ 0),
#   - and how far the output balance lags the bearing (tail smear shows up as the
#     out_ild_db being biased in the direction of the *previous* beep's bearing).
#
# Enable by setting the environment variable BEAM_SCANNER_DIAG=1 before launching
# beamtel.py. Output: %LOCALAPPDATA%/beamtel/scanner_hrtf_diag.csv
#
# It is intentionally cheap on the audio thread: it only appends a small tuple to a
# deque. A daemon thread drains the deque to disk so no file I/O happens in the
# sounddevice callback.

import os
import time
import threading
from collections import deque

try:
    import numpy as np
except Exception:  # pragma: no cover - numpy is a hard dep elsewhere
    np = None


CSV_HEADER = (
    "t_ms,dt_ms,bearing_deg,dist_m,aligned,pitch_norm,pitch_mult,"
    "hrtf_used,hrtf_az_deg,ir_ild_db,out_ild_db,rms_l,rms_r\n"
)


def _rms(arr):
    """Root-mean-square of a float buffer; 0.0 for empty/None."""
    if arr is None or len(arr) == 0:
        return 0.0
    return float(np.sqrt(np.mean(np.square(arr, dtype=np.float64))))


def _ild_db(rms_l, rms_r):
    """Interaural level difference in dB. Positive => LEFT louder, negative => RIGHT louder.

    Returns 0.0 when both ears are effectively silent so an idle frame doesn't read
    as a huge imbalance.
    """
    eps = 1e-9
    if rms_l < eps and rms_r < eps:
        return 0.0
    return 20.0 * float(np.log10((rms_l + eps) / (rms_r + eps)))


# Default output path, hoisted to module scope so the "View Logs" menu in
# beamtel.py can offer this file without duplicating the filename literal.
DEFAULT_PATH = os.path.join(
    os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "beamtel",
    "scanner_hrtf_diag.csv",
)


class ScannerHRTFDiagnostic:
    """Lightweight, audio-thread-safe recorder for scanner/HRTF correlation."""

    def __init__(self, logger, path=None):
        self.logger = logger
        self.enabled = bool(os.environ.get("BEAM_SCANNER_DIAG"))
        self._buf = deque()
        self._stop = threading.Event()
        self._writer = None
        self._last_t = None

        self.path = path or DEFAULT_PATH

        if not self.enabled or np is None:
            if self.enabled and np is None:
                self.logger.warning("Scanner HRTF diagnostic disabled: numpy unavailable.")
                self.enabled = False
            return

        try:
            os.makedirs(os.path.dirname(self.path), exist_ok=True)
            # Fresh file each session so a run is self-contained.
            with open(self.path, "w", encoding="utf-8") as f:
                f.write(CSV_HEADER)
        except Exception as e:
            self.logger.warning(f"Scanner HRTF diagnostic could not open '{self.path}': {e}")
            self.enabled = False
            return

        self._writer = threading.Thread(
            target=self._drain_loop, name="scanner-hrtf-diag", daemon=True
        )
        self._writer.start()
        self.logger.info(f"Scanner HRTF diagnostic ENABLED -> {self.path}")

    def record_beep(self, *, bearing_deg, dist_m, aligned, pitch_norm, pitch_mult,
                    hrtf_used, hrtf_az_deg, ir_l, ir_r, out_l, out_r):
        """Called from the audio callback for each scanner beep frame.

        ir_l/ir_r are the HRIRs used (or None for the stereo-pan fallback).
        out_l/out_r are the actual samples written to the output buffers this frame.
        Everything heavy (RMS, log) is cheap on a ~hundreds-of-samples buffer; we keep
        it here rather than the drain thread so the recorded balance matches the exact
        samples produced, with no risk of the buffers being mutated afterwards.
        """
        if not self.enabled:
            return
        now = time.perf_counter() * 1000.0
        dt = 0.0 if self._last_t is None else (now - self._last_t)
        self._last_t = now

        rms_l = _rms(out_l)
        rms_r = _rms(out_r)
        out_ild = _ild_db(rms_l, rms_r)
        ir_ild = _ild_db(_rms(ir_l), _rms(ir_r)) if ir_l is not None else 0.0

        self._buf.append((
            now, dt, float(bearing_deg), float(dist_m), 1 if aligned else 0,
            float(pitch_norm), float(pitch_mult), 1 if hrtf_used else 0,
            float(hrtf_az_deg), ir_ild, out_ild, rms_l, rms_r,
        ))

    def _drain_loop(self):
        while not self._stop.is_set():
            self._flush()
            self._stop.wait(0.25)
        self._flush()

    def _flush(self):
        if not self._buf:
            return
        rows = []
        try:
            while True:
                rows.append(self._buf.popleft())
        except IndexError:
            pass
        if not rows:
            return
        try:
            with open(self.path, "a", encoding="utf-8") as f:
                for r in rows:
                    f.write(
                        "%.3f,%.3f,%.4f,%.3f,%d,%.4f,%.4f,%d,%.3f,%.3f,%.3f,%.6f,%.6f\n" % r
                    )
        except Exception as e:
            # Don't spam; disable on persistent failure.
            self.logger.warning(f"Scanner HRTF diagnostic write failed: {e}")
            self.enabled = False

    def stop(self):
        self._stop.set()
