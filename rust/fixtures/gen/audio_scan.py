"""Golden cases for beam_audio::scan, from audio.py's terrain scan block.

Scalars (pan, step, pitch) exact. Envelopes and grains in full (base64 float64 /
float32). Whole render_scan buffers as summaries -- length, per-channel sums, peak, and
every 67th frame -- since a full buffer is about a megabyte.
"""

from __future__ import annotations

import base64

import numpy as np

from _common import fbits, write_fixture

import audio


def b64(arr, dtype):
    return base64.b64encode(np.ascontiguousarray(arr, dtype=dtype).tobytes()).decode("ascii")


def summary(buf):
    buf = np.asarray(buf)
    return {
        "frames": int(buf.shape[0]),
        "sum_l": fbits(float(np.sum(buf[:, 0].astype(np.float64)))),
        "sum_r": fbits(float(np.sum(buf[:, 1].astype(np.float64)))),
        "abs_l": fbits(float(np.sum(np.abs(buf[:, 0].astype(np.float64))))),
        "abs_r": fbits(float(np.sum(np.abs(buf[:, 1].astype(np.float64))))),
        "peak": fbits(float(np.max(np.abs(buf)))),
        "sampled": b64(buf[::67], "<f4"),
    }


def flat_scan(dz=0.0, bearings=24, rings=25, reach=200.0, water=None, families=None):
    out = []
    denom = max(1, rings - 1)
    k = 0
    for s in range(bearings):
        b = -90.0 + s * (180.0 / max(1, bearings - 1))
        for r in range(rings):
            row = [b, (r / denom) * reach, dz, water]
            if families:
                row.append(families[k % len(families)])
            out.append(row)
            k += 1
    return out


def main():
    pans = [{"p": fbits(p), "out": [fbits(v) for v in audio._equal_power_pan(p)]}
            for p in [-2.0, -1.0, -0.5, -0.0, 0.0, 0.3, 1.0, 1.5, 0.5]]
    steps = []
    for dz in [-1e6, -60.0, -40.0, -20.0, -10.0, -0.84, -0.83, 0.0, 0.83, 0.84, 1.0, 2.5,
               5.0, 11.6667, 20.0, 39.9, 40.0, 1e6, float("inf"), float("-inf"), -0.0,
               0.8333333333333334, 2.4999999999999996]:
        steps.append({"dz": fbits(dz), "step": audio.scan_step_from_dz(dz),
                      "pitch": fbits(audio.scan_pitch_hz(dz)),
                      "pitch_obj": fbits(audio.scan_pitch_hz(dz, audio.SCAN_OBJECT_OCTAVE))})

    envs = []
    for n, sr in [(12000, 48000), (10, 48000), (1, 48000)]:
        envs.append({"kind": "ref", "n": n, "sr": fbits(sr), "out": b64(audio._scan_envelope(n, sr), "<f8")})
    for n, sr, mult in [(9120, 48000, 2.5), (40, 44100, 1.0), (3, 48000, 1.0)]:
        envs.append({"kind": "grain", "n": n, "sr": fbits(sr), "mult": fbits(mult),
                     "out": b64(audio._scan_grain_envelope(n, sr, mult), "<f8")})

    grains = []
    for sr in (48000, 44100):
        c = audio.SCAN_MIDDLE_C_HZ
        hi = audio.scan_pitch_hz(40.0)
        lo = audio.scan_pitch_hz(-40.0)
        for fam in ("paved", "grass", "dirt", "gravel", "ice"):
            for hz in ((c,) if sr == 48000 else ()) + ((hi,) if sr == 48000 and fam in ("paved", "ice") else ()) + ((lo,) if sr == 44100 and fam == "dirt" else ()):
                grains.append({"fn": "terrain", "family": fam, "hz": fbits(hz), "sr": fbits(sr),
                               "out": b64(audio._scan_terrain_grain(hz, sr, fam), "<f4")})
        grains.append({"fn": "water", "hz": fbits(c), "sr": fbits(sr),
                       "out": b64(audio._scan_water_grain(c, sr), "<f4")})
        for kind in (("v", "p") if sr == 48000 else ("p",)):
            grains.append({"fn": "object", "kind": kind, "hz": fbits(c * 2.0), "sr": fbits(sr),
                           "out": b64(audio._scan_object_ping(c * 2.0, sr, kind), "<f4")})
        grains.append({"fn": "poi", "hz": fbits(c * 2.0), "sr": fbits(sr),
                       "out": b64(audio._scan_poi_doublet(c * 2.0, sr), "<f4")})
        grains.append({"fn": "ref", "sr": fbits(sr), "out": b64(audio._scan_reference_ping(sr), "<f4")})
    # A very high pitch, so the Nyquist guards in every voice are exercised.
    for fn, wave in (("water", audio._scan_water_grain(9000.0, 22050)),
                     ("object", audio._scan_object_ping(9000.0, 22050, "v")),
                     ("poi", audio._scan_poi_doublet(4000.0, 22050))):
        grains.append({"fn": fn, "kind": "v", "hz": fbits(9000.0 if fn != "poi" else 4000.0),
                       "sr": fbits(22050), "out": b64(wave, "<f4")})
    for fam in ("ice", "dirt", "paved"):
        grains.append({"fn": "terrain", "family": fam, "hz": fbits(4000.0), "sr": fbits(22050),
                       "out": b64(audio._scan_terrain_grain(4000.0, 22050, fam), "<f4")})

    rng = np.random.default_rng(99)
    fams = ["paved", "grass", "dirt", "gravel", "ice", "r", "", None, "quicksand"]
    mixed = []
    for s in flat_scan(0.0, bearings=12, rings=25, families=None):
        dz = float(np.round(rng.uniform(-50, 50), 2))
        roll = rng.uniform()
        depth = None if roll < 0.7 else float(np.round(rng.uniform(0, 5), 2))
        dz = None if rng.uniform() < 0.05 else dz
        mixed.append([s[0], s[1], dz, depth, fams[int(rng.integers(0, len(fams)))]])
    objects = [[float(b), float(r), float(dz), k] for b, r, dz, k in
               [(-80, 10, 2.0, "v"), (30, 55, -3.0, "p"), (0, 120, 0.0, None), (75, 199, 12.0, "?"),
                (-10, -5.0, 1.0, "v"), (20, 300.0, 0.0, "p")]]
    pois = [[10.0, 50.0, 0.0], [-45.0, 150.0, -6.0], [90.0, 199.9, 30.0]]

    def to_tuples(rows):
        return [tuple(r) for r in rows]

    renders = []
    scen = [
        ("flat", dict(samples=flat_scan(0.0), objects=[], reach_m=200.0)),
        ("flat_reach60", dict(samples=flat_scan(0.0, reach=60.0), objects=[], reach_m=60.0)),
        ("mixed", dict(samples=mixed, objects=objects, reach_m=200.0, pois=pois)),
        ("mixed_0db", dict(samples=mixed, objects=objects, reach_m=200.0, pois=pois, level_db=0.0)),
        ("mixed_44k", dict(samples=mixed, objects=objects, reach_m=150.0, pois=pois, sr=44100)),
        ("missing", dict(samples=flat_scan(None), objects=[], reach_m=200.0)),
        ("tiny_reach", dict(samples=[[45.0, 0.5, 3.0, None]], objects=[], reach_m=0.2)),
        ("empty", dict(samples=[], objects=[], reach_m=200.0)),
    ]
    for name, kw in scen:
        kw = dict(kw)
        args = dict(kw)
        args["samples"] = to_tuples(kw["samples"])
        args["objects"] = to_tuples(kw["objects"])
        args["pois"] = to_tuples(kw.get("pois", []))
        buf = audio.render_scan(**args)
        renders.append({"name": name,
                        "samples": [list(s) for s in kw["samples"]],  # finite floats round-trip as JSON
                        "objects": [list(o) for o in kw["objects"]],
                        "pois": [list(p) for p in kw.get("pois", [])],
                        "reach": fbits(kw["reach_m"]), "sr": fbits(kw.get("sr", 48000)),
                        "level_db": fbits(kw.get("level_db", audio.SCAN_TERRAIN_DB)),
                        "out": summary(buf)})

    errors = []
    for name, samples, objs in [
        ("nan_range", [(0.0, float("nan"), 0.0, None)], []),
        ("neg_inf_range", [(0.0, float("-inf"), 0.0, None)], []),
        ("nan_dz", [(0.0, 10.0, float("nan"), None)], []),
        ("nan_object_dz", [], [(0.0, 10.0, float("nan"))]),
    ]:
        try:
            audio.render_scan(samples, objs, 200.0)
            errors.append({"name": name, "err": None})
        except Exception as e:  # noqa: BLE001
            errors.append({"name": name, "err": type(e).__name__, "msg": str(e)})

    write_fixture("audio_scan", {"pan": pans, "steps": steps, "envelopes": envs,
                                 "grains": grains, "renders": renders, "errors": errors})


if __name__ == "__main__":
    main()
