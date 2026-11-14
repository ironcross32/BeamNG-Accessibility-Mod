"""Golden cases for beam_reports::wheel_slip, from wheel_slip.py."""

from __future__ import annotations

import random

from _common import fbits, write_fixture
from _pyvalue import enc, outcome_enc

import wheel_slip as ws


def res(r):
    return {
        "active": r.active, "kind": r.kind, "magnitude_ms": fbits(r.magnitude_ms),
        "slide_active": r.slide_active, "slide_deg": fbits(r.slide_deg),
        "azimuth_deg": fbits(r.azimuth_deg), "v_long_ms": fbits(r.v_long_ms),
        "filtered_ms": fbits(r.filtered_ms), "legacy": r.legacy, "ratio": fbits(r.ratio),
    }


def opt(x):
    return None if x is None else fbits(x)


def main():
    rnd = random.Random(1501)
    nan, inf = float("nan"), float("inf")

    ratios = []
    for kind in (ws.KIND_SPIN, ws.KIND_LOCK, ws.KIND_NONE, 5):
        for mag in (0.0, -0.0, -1.0, 1e-7, 0.5, 3.0, 10.0, nan, inf):
            for v in (0.0, -2.0, 1e-7, 0.5, 3.0, 27.0, nan, inf):
                ratios.append({"kind": kind, "mag": fbits(mag), "v": fbits(v),
                               "out": fbits(ws.slip_ratio(kind, mag, v))})

    bearings = []
    for vx, vy, h in [(0.0, 5.0, 180.0), (-5.0, 0.0, 180.0), (5.0, 0.0, 180.0), (0.0, -5.0, 180.0),
                      (0.0, 0.0, 0.0), (1.0, 1.0, 45.0), (nan, 1.0, 0.0)] + [
            (rnd.uniform(-30, 30), rnd.uniform(-30, 30), rnd.uniform(0, 360)) for _ in range(100)]:
        bearings.append({"vx": fbits(vx), "vy": fbits(vy), "h": fbits(h),
                         "out": fbits(ws.travel_bearing(vx, vy, h))})

    sideslip = [{"b": fbits(b), "out": fbits(ws.sideslip_deg(b))}
                for b in [0.0, -0.0, 12.0, 90.0, 91.0, 179.0, 180.0, -180.0, 270.0, 400.0, nan, inf]
                + [rnd.uniform(-400, 400) for _ in range(60)]]

    azimuths = []
    for b in [None, 0.0, 5.0, 9.999, 10.0, -10.0, 30.0, -30.0, 60.0, 90.0, 169.9, 170.0, 170.1,
              -175.0, 180.0, 350.0, nan] + [rnd.uniform(-360, 360) for _ in range(4)]:
        for g in (0.0, 0.1, 0.5, 0.51, 1.25, 1.999, 2.0, 10.0, nan):
            for rev in (False, True):
                azimuths.append({"b": opt(b), "g": fbits(g), "rev": rev,
                                 "out": fbits(ws.voice_azimuth(b, g, rev))})

    thresholds = [{"v": fbits(v), "out": fbits(ws._threshold(v))}
                  for v in (0.0, 5.0, 6.0, 6.0001, 100.0, -5.0, nan)]

    samples = [
        {}, {"ground_speed_ms": 10.0, "travel_bearing_deg": 40.0, "wheel_max_driven_ms": 7.7, "wheel_min_ms": 7.6},
        {"ground_speed_ms": 0.2, "wheel_max_driven_ms": 9.0, "wheel_min_ms": 0.2},
        {"ground_speed_ms": 5.0, "wheel_speed_ms": 1.0}, {"ground_speed_ms": 5.0, "wheel_speed_ms": 9.0},
        {"ground_speed_ms": 1.5, "wheel_speed_ms": 9.0}, {"ground_speed_ms": "5", "wheel_speed_ms": "x"},
        {"ground_speed_ms": None, "wheel_speed_ms": [1]}, {"ground_speed_ms": float("nan")},
        {"ground_speed_ms": 10**400}, {"ground_speed_ms": 5.0, "wheel_max_driven_ms": -1.0, "wheel_min_ms": -1.0, "wheel_speed_ms": 2.0},
        {"ground_speed_ms": 10.0, "wheel_max_driven_ms": 10.0, "wheel_min_ms": 5.0},
        {"ground_speed_ms": 10.0, "wheel_max_driven_ms": 20.0, "wheel_min_ms": 0.0},
        {"ground_speed_ms": 10.0, "travel_bearing_deg": 5.0, "wheel_max_driven_ms": 9.9, "wheel_min_ms": 9.9},
        {"ground_speed_ms": 10.0, "travel_bearing_deg": "inf", "wheel_max_driven_ms": 10, "wheel_min_ms": 10},
        {"ground_speed_ms": -0.0, "wheel_speed_ms": -0.0}, {"ground_speed_ms": True, "wheel_speed_ms": False},
        {"ground_speed_ms": 3.0, "travel_bearing_deg": 20.0, "wheel_max_driven_ms": 3.0, "wheel_min_ms": 3.0},
        {"ground_speed_ms": 3.0001, "travel_bearing_deg": 20.0, "wheel_max_driven_ms": 3.0, "wheel_min_ms": 2.0},
        {"ground_speed_ms": 2.0, "wheel_max_driven_ms": 0.0, "wheel_min_ms": 0.0},
        {"ground_speed_ms": 2.1, "wheel_max_driven_ms": 0.0, "wheel_min_ms": 0.0},
    ]
    for _ in range(80):
        s = {"ground_speed_ms": rnd.uniform(0, 30)}
        if rnd.random() < 0.6:
            s["travel_bearing_deg"] = rnd.uniform(-180, 180)
        if rnd.random() < 0.7:
            s["wheel_max_driven_ms"] = rnd.choice([-1.0, rnd.uniform(0, 40)])
            s["wheel_min_ms"] = rnd.choice([-1.0, rnd.uniform(0, 30)])
        else:
            s["wheel_speed_ms"] = rnd.uniform(0, 40)
        samples.append(s)
    classify = []
    for s in samples:
        out = outcome_enc(ws.classify, s)
        classify.append({"sample": enc(s), **out})

    # Detector sequences: every step's full result.
    sequences = []
    for seq_i in range(24):
        det = ws.SlipDetector()
        t = rnd.choice([0.0, 1.0, 5.0])
        steps = []
        mode = seq_i % 4
        g = rnd.uniform(0, 25)
        for _ in range(rnd.randint(5, 50)):
            r = rnd.random()
            if r < 0.03:
                t += rnd.choice([2.0, 0.0, -0.5])  # rebase: gap, repeat, backwards
            else:
                t += rnd.uniform(0.005, 0.05)
            g = max(0.0, g + rnd.uniform(-1.5, 1.5))
            bearing = None if rnd.random() < 0.2 else rnd.uniform(-180, 180) if mode == 2 else rnd.uniform(-20, 20)
            if mode == 1 or rnd.random() < 0.05:
                wmax, wmin = rnd.choice([(-1.0, -1.0), (None, None), (-1.0, 3.0)])
            else:
                wmax = max(0.0, g + rnd.uniform(-6, 12))
                wmin = max(0.0, g + rnd.uniform(-12, 2))
            legacy = rnd.choice([None, 0.0, -0.0, g + rnd.uniform(-8, 8), nan])
            rev = rnd.random() < 0.2
            out = det.step(t, g, bearing, wmax, wmin, legacy, rev)
            steps.append({"now": fbits(t), "ground": fbits(g), "bearing": opt(bearing),
                          "wmax": opt(wmax), "wmin": opt(wmin), "legacy_wheel": opt(legacy),
                          "reverse": rev, "out": res(out)})
            if rnd.random() < 0.01:
                det.reset()
                steps.append({"reset": True})
        sequences.append(steps)

    write_fixture("wheel_slip", {
        "constants": {k: fbits(getattr(ws, k)) for k in (
            "SLIP_TAU_S", "SLIP_SUSTAIN_S", "SLIP_ABS_THRESHOLD_MS", "SLIP_REL_THRESHOLD",
            "SLIP_MIN_GROUND_MS", "LEGACY_TAU_S", "LEGACY_SUSTAIN_S", "SLIDE_MIN_GROUND_MS",
            "SLIDE_ENTER_DEG", "SLIDE_EXIT_DEG", "SLIDE_SUSTAIN_S", "AZIMUTH_SNAP_DEG",
            "DIR_MIN_MS", "DIR_FULL_MS")},
        "inactive": res(ws.INACTIVE),
        "slip_ratio": ratios,
        "travel_bearing": bearings,
        "sideslip_deg": sideslip,
        "voice_azimuth": azimuths,
        "threshold": thresholds,
        "classify": classify,
        "sequences": sequences,
    })


if __name__ == "__main__":
    main()
