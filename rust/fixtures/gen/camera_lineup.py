"""Golden cases for beam_reports::camera_lineup, from camera_lineup.py."""

from __future__ import annotations

import random

from _common import fbits, outcome, write_fixture

import camera_lineup as cl


def opt(x):
    return None if x is None else fbits(x)


def geom_out(g):
    return [fbits(v) for v in (g.cam_x, g.cam_y, g.cam_fx, g.cam_fy, g.nose_x, g.nose_y, g.veh_fx, g.veh_fy)]


def tone(t):
    return [fbits(t.amp), fbits(t.index), fbits(t.ratio), fbits(t.az)]


def main():
    rnd = random.Random(4489)
    nan, inf = float("nan"), float("inf")

    texts = ["LINEUP:OFF", " LINEUP:OFF\n", "LINEUP:off", "LINEUP:", "", "LINEUP:1,2,3",
             "LINEUP:1,2,0,1,3,4,0,-1", "LINEUP:1,2,0,1,3,4,0,-1,extra", "LINEUP:nan,2,0,1,3,4,0,-1",
             "LINEUP:inf,2,0,1,3,4,0,-1", "LINEUP:1,2,0,1,3,4,0,x", "LINEUP: 1 ,2,0,1,3,4,0,-1 ",
             "lineup:1,2,0,1,3,4,0,-1", "LINEUP:1_0,2,0,1,3,4,0,-1", "LINEUP:Ù¡,2,0,1,3,4,0,-1",
             "LINEUP:OFF,1", "XLINEUP:OFF", "LINEUP:1e400,2,0,1,3,4,0,-1"]
    packets = []
    for t in texts:
        p = cl.parse_packet(t)
        packets.append({"text": t, "out": p if p is None or p == "OFF" else geom_out(p)})

    geoms = [
        (0.0, 0.0, 0.0, 1.0, 0.0, 50.0, 0.0, -1.0), (0.0, 0.0, 0.0, 0.0, 0.0, 50.0, 0.0, -1.0),
        (0.0, 0.0, 0.0, 1.0, 0.0, 50.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0),
        (0.0, 0.0, 0.0, 1.0, -30.0, 10.0, 1.0, 0.0), (0.0, 0.0, 1e-7, 0.0, 0.0, 0.0, 1.0, 0.0),
        (0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 0.0, 1.0), (5.0, 5.0, 3.0, 4.0, 5.0, 5.0, -3.0, -4.0),
    ]
    geoms += [tuple(rnd.uniform(-60, 60) if i not in (2, 3, 6, 7) else rnd.uniform(-1, 1)
                    for i in range(8)) for _ in range(25)]
    solves = []
    for g in geoms:
        geom = cl.Geometry(*g)
        for mode in (cl.HEAD_ON, cl.CROSSING, "other"):
            for off, depth in ((0.0, 10.0), (3.0, 10.0), (-7.0, 30.0)):
                st, miss, brg = cl.solve(geom, mode, off, depth)
                tp = cl.target_point(geom, mode, off, depth)
                solves.append({"geom": [fbits(v) for v in g], "mode": mode, "offset": fbits(off),
                               "depth": fbits(depth), "status": st, "miss": opt(miss),
                               "bearing": opt(brg), "target": None if tp is None else [fbits(v) for v in tp],
                               "tone": tone(cl.tone_for(st, miss, brg)),
                               "tone_nobearing": tone(cl.tone_for(st, miss))})

    tones = []
    for st in ("ok", "behind", "unknown", "x"):
        for miss in (None, 0.0, -0.0, 0.5, 0.50001, -0.6, 3.0, -3.0, 8.0, 13.0, -13.0, nan, inf):
            for brg in (None, 0.0, -0.0, 3.4, -3.4, 90.0, -90.0, 179.9, 180.0, -180.0, 270.0, nan):
                tones.append({"status": st, "miss": opt(miss), "bearing": opt(brg),
                              "out": tone(cl.tone_for(st, miss, brg))})

    steps = []
    for mode in (cl.HEAD_ON, cl.CROSSING):
        for off in (0.0, 49.5, -50.0, 50.0, nan):
            for depth in (1.0, 10.0, 199.5, 200.0, nan):
                for delta in (-10, -1, 0, 1, 10, 1000):
                    o, d = cl.step_value(mode, off, depth, delta)
                    steps.append({"mode": mode, "offset": fbits(off), "depth": fbits(depth),
                                  "delta": delta, "out": [fbits(o), fbits(d)]})

    phrases = []
    for mode in (cl.HEAD_ON, cl.CROSSING, "other"):
        for off in (0.0, 0.04999, -0.05, 0.05, 0.5, 1.0, 1.5, -1.0, 2.5, 3.0, -3.0, 0.3048, 0.15,
                    50.0, nan, inf):
            for depth in (0.0, 0.3048, 1.0, 12.0, 2.5, nan):
                for units in ("metric", "imperial", "Imperial"):
                    phrases.append({"mode": mode, "offset": fbits(off), "depth": fbits(depth),
                                    "units": units,
                                    "value": outcome(cl.value_phrase, mode, off, depth, units),
                                    "target": outcome(cl.target_phrase, mode, off, depth, units)})

    write_fixture("camera_lineup", {
        "constants": {k: (fbits(getattr(cl, k)) if isinstance(getattr(cl, k), float) else getattr(cl, k))
                      for k in ("HEAD_ON", "CROSSING", "DEFAULT_OFFSET_M", "DEFAULT_DEPTH_M", "STEP_M",
                                "OFFSET_LIMIT_M", "DEPTH_MIN_M", "DEPTH_MAX_M", "DEADZONE_M", "FULL_M",
                                "INDEX_MIN", "INDEX_MAX", "RATIO_DRIFT", "CARRIER_HZ",
                                "SIDE_AZIMUTH_DEG", "BEHIND_AZIMUTH_DEG", "FEET_PER_M")},
        "silent": tone(cl.SILENT),
        "mode_name": [[m, cl.mode_name(m)] for m in (cl.HEAD_ON, cl.CROSSING, "x")],
        "packets": packets,
        "solves": solves,
        "tones": tones,
        "steps": steps,
        "phrases": phrases,
    })


if __name__ == "__main__":
    main()
