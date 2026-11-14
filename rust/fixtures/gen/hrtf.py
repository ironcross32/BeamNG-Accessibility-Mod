"""Golden cases for beam_audio::npy and beam_audio::hrtf, from the real hrtf.py.

For each bundled set at several elevations (the -3 default, ring edges, one inside the
SADIE gap), with equalisation on and off: the processed ring at the native rate (a subset
of azimuths in full plus every row's energy), the ITD curve, the EQ curve, the info and
gain, band_energy; then the ring FFT-resampled to 48 kHz and one other rate, and get_hrir
at fractional azimuths. Arrays are base64 little-endian bytes (`f4`/`f8`) to keep the
file small.
"""

from __future__ import annotations

import base64
import logging
import os

import numpy as np

from _common import REPO_ROOT, fbits, write_fixture

import hrtf
import hrtf_sets


def b64(arr, dtype):
    return base64.b64encode(np.ascontiguousarray(arr, dtype=dtype).tobytes()).decode("ascii")


AZ_FULL = [0, 1, 45, 90, 135, 180, 270, 359]
HRIR_AZ = [10.5, -45.25, 359.7, 720.3, 90.0, 0.0]

CASES = [
    ("mit_kemar", -3.0, True), ("mit_kemar", -3.0, False), ("mit_kemar", 10.0, True),
    ("mit_kemar", -15.0, False),
    ("thk_ku100", -3.0, True), ("thk_ku100", -3.0, False), ("thk_ku100", 0.0, True),
    ("sadie_kemar", -3.0, True), ("sadie_kemar", -8.0, False), ("sadie_kemar", -20.0, True),
]


def npz_summary(path):
    out = []
    with np.load(path) as d:
        for k in d.files:
            a = d[k]
            flat = np.asarray(a, dtype=np.float64).ravel()
            out.append({"name": k, "dtype": a.dtype.str, "shape": list(a.shape),
                        "sum": fbits(np.sum(flat)), "head": b64(flat[:4], "<f8")})
    return out


def ring_summary(left, right, az_full):
    return {
        "ir_len": int(left.shape[1]),
        "rows_l": b64(left[az_full], "<f4"),
        "rows_r": b64(right[az_full], "<f4"),
        "energy_l": b64(np.sum(left.astype(np.float64) ** 2, axis=1), "<f8"),
        "energy_r": b64(np.sum(right.astype(np.float64) ** 2, axis=1), "<f8"),
    }


def main():
    log = logging.getLogger("fixture")
    log.addHandler(logging.NullHandler())
    log.propagate = False

    sets = {}
    for info in hrtf_sets.BUNDLED_SETS:
        sets[info.set_id] = {"file": info.filename,
                             "members": npz_summary(os.path.join(REPO_ROOT, info.filename))}

    cases = []
    for set_id, elev, eq in CASES:
        info = hrtf_sets.resolve(set_id)
        path = os.path.join(REPO_ROOT, info.filename)
        with np.load(path) as data:
            az = data["azimuths"].astype(np.float64)
            el = data["elevations"].astype(np.float64)
            irl = data["irs_left"].astype(np.float64)
            irr = data["irs_right"].astype(np.float64)
            sr = float(data["samplerate"])

        # The intermediates, recomputed exactly as process_ring does.
        el_r = np.round(el, 1)
        out_l, out_r, rinfo = hrtf.process_ring(az, el, irl, irr, sr, elev, eq)

        # ITD and EQ curves: rerun the first half of process_ring.
        rings = np.unique(el_r)
        mag_l = np.zeros((360, hrtf.PROC_FFT_SIZE // 2 + 1))
        mag_r = np.zeros_like(mag_l)
        itd = np.zeros(360)
        pair = [(r, None) for r in rinfo["rings"]]
        if len(pair) == 1:
            weights = [1.0]
        else:
            below, above = rinfo["rings"]
            w = (rinfo["elevation"] - below) / (above - below)
            weights = [1.0 - w, w]
        for (ring_el, _), weight in zip(pair, weights):
            sel = np.flatnonzero(el_r == ring_el)
            order = sel[np.argsort(az[sel] % 360.0)]
            ring_az = az[order] % 360.0
            l, r = irl[order], irr[order]
            mag_l += weight * hrtf._interp_ring(ring_az, hrtf.magnitudes(l), hrtf.OUTPUT_AZIMUTHS)
            mag_r += weight * hrtf._interp_ring(ring_az, hrtf.magnitudes(r), hrtf.OUTPUT_AZIMUTHS)
            itd += weight * hrtf._interp_ring(ring_az, hrtf.estimate_itd(l, r, sr), hrtf.OUTPUT_AZIMUTHS)
        mirror = (360 - np.arange(360)) % 360
        sym_l = 0.5 * (mag_l + mag_r[mirror])
        mag_r = sym_l[mirror]
        mag_l = sym_l
        itd = 0.5 * (itd - itd[mirror])
        eq_curve = hrtf.equalisation_curve(mag_l, mag_r, sr) if eq else None

        hs = hrtf.HRTFSet(path, log, elevation_deg=elev, equalise=eq)
        energy = hrtf.band_energy(np.concatenate([out_l, out_r]), sr)
        gain = float(np.sqrt(hrtf.REFERENCE_BAND_ENERGY / max(energy, 1e-20)))
        case = {
            "set_id": set_id, "elevation": fbits(elev), "equalise": eq,
            "samplerate": fbits(sr),
            "info": {"elevation": fbits(rinfo["elevation"]),
                     "rings": [fbits(r) for r in rinfo["rings"]],
                     "max_itd_us": fbits(rinfo["max_itd_us"])},
            "rings_all": [fbits(r) for r in rings],
            "itd": b64(itd, "<f8"),
            "eq": None if eq_curve is None else b64(eq_curve, "<f8"),
            "band_energy_unscaled": fbits(energy),
            "gain": fbits(gain),
            "band_energy_scaled": fbits(hrtf.band_energy(
                np.concatenate([hs._irs_left, hs._irs_right]), sr)),
            "native": ring_summary(hs._irs_left, hs._irs_right, AZ_FULL),
            "resampled": [],
        }
        other = 96000.0 if abs(sr - 48000.0) >= 1.0 else 44100.0
        for target in (48000.0, other):
            h2 = hrtf.HRTFSet.__new__(hrtf.HRTFSet)
            h2.__dict__.update(hs.__dict__)
            h2.logger = log
            h2.resample(target)
            hr = {"rate": fbits(target), **ring_summary(h2._irs_left, h2._irs_right, [0, 90, 271])}
            if target == 48000.0 and elev == -3.0 and eq:
                hr["hrir"] = []
                for a in HRIR_AZ:
                    il, ir = h2.get_hrir(a)
                    hr["hrir"].append({"az": fbits(a), "dtype": il.dtype.str,
                                       "l": b64(il, "<f8"), "r": b64(ir, "<f8")})
            case["resampled"].append(hr)
        cases.append(case)
        print(set_id, elev, eq, rinfo, f"gain {gain:.6f}")

    write_fixture("hrtf", {"sets": sets, "cases": cases, "az_full": AZ_FULL})


if __name__ == "__main__":
    main()
