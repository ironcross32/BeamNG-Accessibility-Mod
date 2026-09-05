"""Replay the hydro-steer tone/click state machine against a fake clock.

Mirrors the render block in audio.py so the envelope, pitch glide, deadzone gating,
FM timbre curve and centre-click latch can be checked without an audio device or the
game running. Run after touching that block:

    python diagnostic/hydro_steer_sim.py

The tuning constants are imported from audio.py rather than copied, so hand-tuning
there can't silently invalidate these checks. Only the *logic* is duplicated here.
"""
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from audio import (  # noqa: E402
    HYDRO_STEER_LO_HZ,
    HYDRO_STEER_HI_HZ,
    HYDRO_STEER_MIN_DB,
    HYDRO_STEER_MAX_DB,
    HYDRO_STEER_DEADZONE,
    HYDRO_STEER_FULL,
    HYDRO_STEER_FM_RATIO_LO,
    HYDRO_STEER_FM_RATIO_HI,
    HYDRO_STEER_FM_INDEX_LO,
    HYDRO_STEER_FM_INDEX_HI,
    HYDRO_STEER_FM_DIRT_CURVE,
    HYDRO_STEER_ATTACK_TAU,
    HYDRO_STEER_REL_TAU,
    HYDRO_STEER_PITCH_TAU,
    HYDRO_STEER_MAX_AZIMUTH_DEG,
    HYDRO_CENTER_REARM,
    HYDRO_CENTER_CLICK_DUR_MS,
)

SR = 48000
FRAMES = 512

print(f"tuning: pitch {HYDRO_STEER_LO_HZ:.0f}-{HYDRO_STEER_HI_HZ:.0f}Hz  "
      f"level {HYDRO_STEER_MIN_DB:.0f}..{HYDRO_STEER_MAX_DB:.0f}dB  "
      f"deadzone {HYDRO_STEER_DEADZONE} full {HYDRO_STEER_FULL}  "
      f"ratio {HYDRO_STEER_FM_RATIO_LO}-{HYDRO_STEER_FM_RATIO_HI} "
      f"index {HYDRO_STEER_FM_INDEX_LO}-{HYDRO_STEER_FM_INDEX_HI} "
      f"curve {HYDRO_STEER_FM_DIRT_CURVE}  azimuth +/-{HYDRO_STEER_MAX_AZIMUTH_DEG:.0f}deg")
print()


def clamp(v, lo=0.0, hi=1.0):
    return max(lo, min(hi, v))


def pan_gains(p):
    ang = (clamp(p, -1.0, 1.0) + 1.0) * (math.pi / 4.0)
    return math.cos(ang), math.sin(ang)


class Sim:
    def __init__(self):
        self.last_samples = np.zeros(0)
        self.last_pan = (0.0, 0.0)
        self.phase = 0.0
        self.mod_phase = 0.0
        self.inc = 0.0
        self.ratio = 0.0
        self.index = 0.0
        self.env = 0.0
        self.sm_mag = 0.0
        self.armed = False
        self.pos = -1.0
        self.clicks = 0
        self.click_len = int(SR * HYDRO_CENTER_CLICK_DUR_MS / 1000.0)

    def block(self, hydro_actual, frames=FRAMES):
        abs_h = abs(hydro_actual)
        dt = frames / SR
        target = 1.0 if abs_h > HYDRO_STEER_DEADZONE else 0.0
        peak = 0.0
        freq = 0.0
        if target > 0.0 or self.env > 1e-4:
            tau = HYDRO_STEER_ATTACK_TAU if target > self.env else HYDRO_STEER_REL_TAU
            env_end = self.env + (1.0 - math.exp(-dt / tau)) * (target - self.env)
            beta = 1.0 - math.exp(-dt / HYDRO_STEER_PITCH_TAU)
            self.sm_mag += beta * (abs_h - self.sm_mag)
            norm = clamp((self.sm_mag - HYDRO_STEER_DEADZONE)
                         / (HYDRO_STEER_FULL - HYDRO_STEER_DEADZONE))
            inc = (HYDRO_STEER_LO_HZ + (HYDRO_STEER_HI_HZ - HYDRO_STEER_LO_HZ) * norm) / SR
            dirt = norm ** HYDRO_STEER_FM_DIRT_CURVE
            ratio = HYDRO_STEER_FM_RATIO_LO + (HYDRO_STEER_FM_RATIO_HI - HYDRO_STEER_FM_RATIO_LO) * dirt
            index = HYDRO_STEER_FM_INDEX_LO + (HYDRO_STEER_FM_INDEX_HI - HYDRO_STEER_FM_INDEX_LO) * dirt
            inc_prev = self.inc if self.inc > 0.0 else inc
            ratio_prev = self.ratio if self.ratio > 0.0 else ratio
            index_prev = self.index if self.index > 0.0 else index
            inc_arr = np.linspace(inc_prev, inc, frames, endpoint=False)
            ratio_arr = np.linspace(ratio_prev, ratio, frames, endpoint=False)
            index_arr = np.linspace(index_prev, index, frames, endpoint=False)
            phase_arr = (self.phase + np.cumsum(inc_arr)) % 1.0
            mod_phase_arr = (self.mod_phase + np.cumsum(inc_arr * ratio_arr)) % 1.0
            self.inc, self.ratio, self.index = inc, ratio, index
            self.phase = float(phase_arr[-1])
            self.mod_phase = float(mod_phase_arr[-1])
            amp = 10.0 ** ((HYDRO_STEER_MIN_DB
                            + (HYDRO_STEER_MAX_DB - HYDRO_STEER_MIN_DB) * norm) / 20.0)
            mono = amp * np.sin(2.0 * np.pi * phase_arr
                                + index_arr * np.sin(2.0 * np.pi * mod_phase_arr))
            mono *= np.linspace(self.env, env_end, frames, endpoint=False)
            self.env = env_end
            self.last_samples = mono
            peak = float(np.max(np.abs(mono)))
            freq = inc * SR
            Lg, Rg = pan_gains(-hydro_actual)
            self.last_pan = (Lg, Rg)
        else:
            self.env = 0.0
            self.inc = 0.0
            self.ratio = 0.0
            self.index = 0.0
            self.sm_mag = abs_h
            self.last_pan = (0.0, 0.0)
            self.last_samples = np.zeros(frames)

        if abs_h > HYDRO_STEER_DEADZONE * HYDRO_CENTER_REARM:
            self.armed = True
        elif self.armed and abs_h <= HYDRO_STEER_DEADZONE:
            self.armed = False
            self.pos = 0.0
            self.clicks += 1

        if self.pos >= 0:
            nxt = int(self.pos) + frames
            self.pos = float(nxt) if nxt < self.click_len else -1.0

        return peak, freq


def scenario(name, values, expect_clicks=None):
    s = Sim()
    peaks, freqs = [], []
    for v in values:
        p, f = s.block(v)
        peaks.append(p)
        freqs.append(f)
    print(f"{name}: clicks={s.clicks} peak_max={max(peaks):.4f} "
          f"freq_range=({min(f for f in freqs if f > 0) if any(freqs) else 0:.0f},{max(freqs):.0f})")
    if expect_clicks is not None:
        status = "OK" if s.clicks == expect_clicks else "FAIL"
        print(f"   expected {expect_clicks} clicks -> {status}")
    return s, peaks, freqs


# 1. Normal car: actual_steering pinned at 0 forever.
scenario("normal car (always 0)", [0.0] * 400, expect_clicks=0)

# 2. Loader articulates hard left, holds, returns to straight.
seq = ([0.0] * 20 + list(np.linspace(0, 0.6, 60)) + [0.6] * 100
       + list(np.linspace(0.6, 0.0, 60)) + [0.0] * 60)
scenario("articulate left, hold, straighten", seq, expect_clicks=1)

# 3. Hunting around centre: should NOT machine-gun.
hunt = [0.0] * 10
for _ in range(12):
    hunt += list(np.linspace(0.0, 0.09, 8)) + list(np.linspace(0.09, 0.0, 8))
scenario("hunting just past deadzone (below rearm)", hunt, expect_clicks=0)

# 4. Full sweeps both ways: one click per genuine crossing.
sweep = []
for _ in range(3):
    sweep += list(np.linspace(0.0, 0.5, 30)) + list(np.linspace(0.5, 0.0, 30)) + [0.0] * 10
scenario("three full out-and-back sweeps", sweep, expect_clicks=3)

# 5. Crossing straight without stopping (left -> right).
cross = list(np.linspace(0.5, -0.5, 80)) + [-0.5] * 20
scenario("left to right through centre", cross, expect_clicks=1)

# 6. Envelope continuity: no discontinuity > 0.05 between adjacent block peaks.
s, peaks, _ = scenario("continuity check", seq)
jumps = [abs(peaks[i] - peaks[i - 1]) for i in range(1, len(peaks))]
print(f"   max block-to-block peak jump = {max(jumps):.4f} "
      f"({'OK' if max(jumps) < 0.05 else 'FAIL - audible step'})")

# 7. Phase continuity across block boundaries. The peak check above wouldn't catch this:
# a carrier or modulator phase that failed to carry over renders as a click, not a level
# jump. A boundary step must be no worse than the waveform's own steepest slope.
s = Sim()
blocks = []
for v in seq:
    s.block(v)
    blocks.append(s.last_samples)
sig = np.concatenate(blocks)
d = np.abs(np.diff(sig))
bidx = np.arange(FRAMES, len(sig), FRAMES) - 1
b_max, all_max = float(d[bidx].max()), float(d.max())
print(f"phase continuity: max boundary step={b_max:.6f} vs max anywhere={all_max:.6f} "
      f"({'OK' if b_max <= all_max * 1.05 else 'FAIL - phase discontinuity'})")

# 8. FM timbre. Two things matter: it must be harmonic at straight and inharmonic at
# lock, AND the middle of the range must stay near-integer — scaling the ratio linearly
# made ordinary steering angles piercing, which is what the curve exists to fix.
def fm_params(norm):
    dirt = norm ** HYDRO_STEER_FM_DIRT_CURVE
    fc = HYDRO_STEER_LO_HZ + (HYDRO_STEER_HI_HZ - HYDRO_STEER_LO_HZ) * norm
    ratio = HYDRO_STEER_FM_RATIO_LO + (HYDRO_STEER_FM_RATIO_HI - HYDRO_STEER_FM_RATIO_LO) * dirt
    idx = HYDRO_STEER_FM_INDEX_LO + (HYDRO_STEER_FM_INDEX_HI - HYDRO_STEER_FM_INDEX_LO) * dirt
    return fc, ratio, idx


def timbre(norm):
    fc, ratio, idx = fm_params(norm)
    n = SR // 2
    t = np.arange(n) / SR
    w = np.sin(2 * np.pi * fc * t + idx * np.sin(2 * np.pi * fc * ratio * t)) * np.hanning(n)
    P = np.abs(np.fft.rfft(w)) ** 2
    f = np.fft.rfftfreq(n, 1 / SR)
    harm = np.zeros(len(f), bool)
    for k in range(1, 40):
        harm |= np.abs(f - k * fc) < fc * 0.06
    W = np.sqrt(P)
    return float(P[~harm].sum() / P.sum()), float((W * f).sum() / W.sum())

print("FM timbre across the range:")
for nrm in (0.0, 0.25, 0.5, 0.75, 1.0):
    _, ratio, idx = fm_params(nrm)
    inh, cen = timbre(nrm)
    print(f"   norm={nrm:4.2f} ratio={ratio:5.3f} index={idx:4.2f} "
          f"inharmonic={inh * 100:5.1f}% centroid={cen:6.0f}Hz")

clean = timbre(0.0)[0]
dirty = timbre(1.0)[0]
print(f"   clean at straight, dirty at lock: "
      f"{'OK' if clean < 0.02 < dirty else 'FAIL'}")
mid_ratio = fm_params(0.5)[1]
print(f"   ratio stays near-integer at half lock ({mid_ratio:.3f}): "
      f"{'OK' if abs(mid_ratio - HYDRO_STEER_FM_RATIO_LO) < 0.15 else 'FAIL - piercing mid-range'}")
peak_centroid = timbre(1.0)[1]
print(f"   centroid at full lock ({peak_centroid:.0f}Hz) below the shrill threshold: "
      f"{'OK' if peak_centroid < 3000 else 'FAIL - too piercing'}")

# 9. HRTF image width. The KEMAR set's interaural level difference peaks around 67.5°
# and *falls* by ~5 dB out to 90°, so mapping full lock to 90° made a harder turn read as
# coming back toward centre. Full lock must sit below that peak's far side.
try:
    import logging

    import hrtf as _hrtf

    _log = logging.getLogger("hydro_steer_sim")
    _log.addHandler(logging.NullHandler())
    _set = _hrtf.HRTFSet("hrtf_kemar_horizontal.npz", _log)

    def _ild_db(az):
        l, r = _set.get_hrir(az)
        el = float(np.sum(l.astype(np.float64) ** 2))
        er = float(np.sum(r.astype(np.float64) ** 2))
        return 10.0 * np.log10(max(el, 1e-20) / max(er, 1e-20))

    near = _ild_db(HYDRO_STEER_MAX_AZIMUTH_DEG * 0.75)
    full = _ild_db(HYDRO_STEER_MAX_AZIMUTH_DEG)
    print(f"HRTF width: at +/-{HYDRO_STEER_MAX_AZIMUTH_DEG:.0f}deg, ILD "
          f"{near:.2f}dB at 3/4 lock -> {full:.2f}dB at full lock "
          f"({'OK' if full > near else 'FAIL - harder turn reads as narrower'})")
except Exception as exc:  # npz absent in a bare checkout; not a logic failure
    print(f"HRTF width: skipped ({type(exc).__name__}: {exc})")

# 10. Pan sanity: positive articulation must be LEFT-dominant.
s = Sim()
s.block(0.4)
Lg, Rg = s.last_pan
print(f"pan at +0.4 (should be left-dominant): L={Lg:.3f} R={Rg:.3f} "
      f"({'OK' if Lg > Rg else 'FAIL'})")
s = Sim()
s.block(-0.4)
Lg, Rg = s.last_pan
print(f"pan at -0.4 (should be right-dominant): L={Lg:.3f} R={Rg:.3f} "
      f"({'OK' if Rg > Lg else 'FAIL'})")
