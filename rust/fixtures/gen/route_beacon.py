"""Golden cases for beam_base::route_beacon, from route_beacon.py."""

from __future__ import annotations

import random

from _common import deep_fbits, fbits, outcome, write_fixture

import route_beacon as rb
from road_guidance import direction_label


def packet_outcome(text):
    res = outcome(rb.parse_route_packet, text)
    if "ok" in res and res["ok"] is not None:
        p = res["ok"]
        res["ok"] = {"dest": deep_fbits(list(p["dest"])), "route_m": fbits(p["route_m"])}
    return {"text": text, **res}


def main():
    rnd = random.Random(4482)
    nan, inf = float("nan"), float("inf")

    packets = [
        "ROUTE:1200.5,-340.25,61.0,4821.5", "ROUTE:CLEAR", "ROUTE: CLEAR ", "ROUTE:CLEAR\n",
        "ROUTE:clear", "ROUTE:1,2,3", "ROUTE:a,b,c,d", "ROUTE:1,2,nan,4", "ROUTE:1,2,inf,4",
        "TRAILER:0,1,x", "", "ROUTE:", "ROUTE:0,0,0,-5", "ROUTE:0,0,0,-0.0",
        "ROUTE:1,2,3,4,5,extra", "ROUTE: 1 , 2 ,3,4 ", "ROUTE:1_0,2,3,4", "ROUTE:1e3,2E-2,+3,.5",
        "route:1,2,3,4", " ROUTE:1,2,3,4", "ROUTE:1,,3,4", "ROUTE:x,2,nan,4", "ROUTE:nan,2,x,4",
        "ROUTE:1,2,3,4\x00", "ROUTE: 1,2,3,4 ", "ROUTE:1e400,2,3,4", "ROUTE:CLEAR,1",
    ]

    coords = [0.0, -0.0, 5.0, -5.0, 100.0, -100.0, 1e-9, 12345.678]
    bearings = []
    headings = [0.0, -0.0, 1.0, 90.0, 179.999, 180.0, 180.001, 270.0, 359.0, 360.0, 720.5, -90.0,
                -400.0, nan]
    for px, py, dx, dy in [
        (0.0, 0.0, 0.0, 100.0), (0.0, 0.0, -100.0, 0.0), (0.0, 0.0, 100.0, 0.0),
        (0.0, 0.0, 0.0, -100.0), (0.0, 0.0, 0.0, 0.0), (0.0, 0.0, -0.0, -0.0),
        (5.0, 5.0, 5.0, 5.0), (0.0, 0.0, 5.0, 5.0), (1.0, 2.0, 1.0, 1.0),
    ] + [(rnd.choice(coords), rnd.choice(coords), rnd.choice(coords), rnd.choice(coords))
         for _ in range(40)] + [
        (rnd.uniform(-3000, 3000), rnd.uniform(-3000, 3000), rnd.uniform(-3000, 3000),
         rnd.uniform(-3000, 3000)) for _ in range(80)
    ]:
        for h in headings if rnd.random() < 0.3 else [rnd.choice(headings), rnd.uniform(0, 360)]:
            dist, err = rb.relative_bearing(px, py, h, dx, dy)
            bearings.append({
                "pos": deep_fbits([px, py]),
                "dest": deep_fbits([dx, dy]),
                "heading": fbits(h),
                "target": fbits(rb.target_bearing(px, py, dx, dy)),
                "distance": fbits(dist),
                "err": fbits(err),
            })

    normalize = [
        {"x": fbits(v), "out": fbits(rb.normalize_bearing(v))}
        for v in [0.0, -0.0, 180.0, 180.0000001, -180.0, -180.0000001, 359.9, -359.9, 540.0,
                  -540.0, 900.0, nan, inf, -inf, 1e-300] + [rnd.uniform(-720, 720) for _ in range(60)]
    ]

    distances = [0.0, -0.0, -5.0, nan, inf, 0.4, 0.5, 1.5, 2.5, 100, 304.79, 304.8, 304.8006,
                 1000 / 3.28084 - 1, 1000 / 3.28084, 1000 / 3.28084 + 1, 500, 999.4, 999.5, 999.6,
                 1000.0, 1049.99, 1050.0, 4999.9, 5000, 8046.72, 1609.344, 1e7, 1e300]
    distances += [rnd.uniform(0, 20000) for _ in range(60)]
    long_dist = []
    for d in distances:
        for units in ("imperial", "metric", "IMPERIAL", "Imp", "im", "", "freedom"):
            long_dist.append({"metres": fbits(d), "units": units,
                              "out": rb.format_long_distance(d, units)})

    phrases = []
    for dist in [2.0, 15.0, 15.000001, 3000.0, nan, inf]:
        for brg in [0.0, 2.0, 15.0, 15.1, 30.0, -30.0, 45.0, 45.5, 100.0, -120.0, 121.0, -150.0,
                    150.5, 179.0, -179.0, 180.0, 359.0, -359.0, 1000.0, nan, inf]:
            for route_m in [0.0, -0.0, 4200.0, -1.0, nan]:
                for units in ("metric", "imperial"):
                    res = outcome(rb.route_beacon_phrase, dist, brg, route_m, units)
                    phrases.append({"distance": fbits(dist), "bearing": fbits(brg),
                                    "route_m": fbits(route_m), "units": units, **res})

    labels = []
    for brg in [0.0, -0.0, 15.0, -15.0, 15.0000001, 45.0, -45.0, 45.0001, 120.0, -120.0,
                120.0001, 150.0, -150.0, 150.0001, 179.9, 180.0, -180.0, 190.0, 345.0, 540.0,
                -900.0, nan, inf, -inf] + [rnd.uniform(-720, 720) for _ in range(80)]:
        for u_turn in (False, True):
            labels.append({"bearing": fbits(brg), "u_turn": u_turn,
                           **outcome(direction_label, brg, u_turn=u_turn)})

    write_fixture("route_beacon", {
        "constants": {
            "ROUTE_PREFIX": rb.ROUTE_PREFIX,
            "ROUTE_CLEAR": rb.ROUTE_CLEAR,
            "AT_DESTINATION_M": fbits(rb.AT_DESTINATION_M),
        },
        "packets": [packet_outcome(t) for t in packets],
        "bearings": bearings,
        "normalize": normalize,
        "format_long_distance": long_dist,
        "phrases": phrases,
        "direction_label": labels,
    })


if __name__ == "__main__":
    main()
