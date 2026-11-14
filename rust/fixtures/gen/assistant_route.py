"""Golden cases for beam_road::assistant_route, from assistant_route.py."""

from __future__ import annotations

import json
import random

from _common import fbits, outcome, write_fixture

import assistant_route as ar

NAN, INF = float("nan"), float("inf")
MILE = 1609.344
FOOT = 1 / 3.28084


def main():
    rnd = random.Random(4445)
    metres = [0.0, -0.0, -5.0, 1.0, 3.0, 7.62, 7.63, 15.24, 152.4, 304.0, 304.8, 304.81, 437.0,
              437 * FOOT, 974.9, 975.0, 999.0, 1000.0, 1049.0, 1050.0, 1500.0, 1609.0, MILE,
              1.04 * MILE, 1.05 * MILE, 2 * MILE, 3.2 * MILE, 3200.0, 1e6, NAN, INF]
    metres += [rnd.uniform(0, 8000) for _ in range(80)]
    spoken = [{"metres": fbits(m), "units": u, "out": ar.spoken_distance(m, u)}
              for m in metres for u in ("imperial", "metric", "Imp", "")]

    turns = [{"name": n, "out": ar.turn_instruction(n)} for n in
             ["left", "right", "sharp right", "keep left", "keep", "roundabout", "Roundabout", "", "u-turn"]]

    directions = [{"bearing": fbits(b), "out": ar.relative_direction(b)}
                  for b in [0.0, 4.99, 5.0, -5.0, 40.0, -40.0, 175.0, 175.01, -178.0, 0.5, 89.5, 90.5,
                            NAN, INF] + [rnd.uniform(-180, 180) for _ in range(30)]]

    arrivals_in = [
        {"offRoad": False, "speed": 0.5},
        {"offRoad": True, "speed": 0, "distance": 140 * FOOT, "bearing": 40},
        {"offRoad": True, "distance": 100, "bearing": -90},
        {"offRoad": True, "distance": 100},
        {"offRoad": True, "distance": None, "bearing": 3},
        {"offRoad": 1, "distance": "12", "bearing": 179},
        {"offRoad": True, "distance": "far", "bearing": 10},
        {"offRoad": True, "distance": 50, "bearing": "40"},
        {"offRoad": True, "distance": 50, "bearing": "2"},
        {"offRoad": True, "distance": [], "bearing": 10},
        {"offRoad": True, "distance": 50, "bearing": True},
        {"offRoad": "", "distance": 50, "bearing": 1},
        {},
    ]
    arrivals = []
    for a in arrivals_in:
        for u in ("imperial", "metric"):
            arrivals.append({"arrival": json.dumps(a), "units": u, **outcome(ar.arrival_phrase, a, u)})

    toasts = []
    for text in ["You have reached your destination.", "bigmap.info.reachedTarget", "  YOU HAVE REACHED YOUR DESTINATION ",
                 "Please make a U-turn when possible", "ui.groundMarkers.wrongDirection", "Fuel low", ""]:
        for following in (False, True):
            for since in (None, 0.0, 3.0, 3.0001, 10.0, -1.0):
                toasts.append({"text": text, "following": following,
                               "since": None if since is None else fbits(since),
                               "out": ar.suppressed_toast(text, following, since)})

    scripts = []
    for n in range(60):
        units = ["imperial"]
        callouts = ar.RouteCallouts(lambda: units[0])
        steps = []
        dest = rnd.uniform(200, 8000)
        turn_id = 1
        turn_d = rnd.uniform(50, dest)
        for i in range(rnd.randrange(10, 50)):
            if rnd.random() < 0.05:
                units[0] = rnd.choice(["imperial", "metric"])
            r = rnd.random()
            report = {"destDistance": dest, "speed": rnd.choice([0, 5.0, 10.0, 30.0, None, "12"])}
            if r < 0.7:
                report.update(turnId=turn_id, turn=rnd.choice(["left", "right", "keep left", "roundabout"]),
                              turnDistance=turn_d)
            if rnd.random() < 0.2:
                report["offRoad"] = True
            if rnd.random() < 0.04:
                report["turnDistance"] = rnd.choice(["far", None, [], "12"])
            if rnd.random() < 0.03:
                report["turn"] = rnd.choice([5, [], ""])
            if rnd.random() < 0.03:
                report["turnId"] = rnd.choice([1.0, True, "1", None])
            kind = "route_set" if (i == 0 and rnd.random() < 0.8) or rnd.random() < 0.03 else "update"
            if rnd.random() < 0.02:
                callouts.reset()
                kind = "reset"
                steps.append({"kind": "reset", "units": units[0]})
                continue
            fn = callouts.route_set if kind == "route_set" else callouts.update
            steps.append({"kind": kind, "units": units[0], "report": json.dumps(report),
                          **outcome(fn, report), "following": callouts.following})
            step = rnd.uniform(5, 400)
            dest = max(0.0, dest - step)
            turn_d -= step
            if turn_d < 0:
                turn_id += 1
                turn_d = rnd.uniform(30, max(31, dest))
        scripts.append(steps)

    write_fixture("assistant_route", {
        "spoken_distance": spoken,
        "turn_instruction": turns,
        "relative_direction": directions,
        "arrival_phrase": arrivals,
        "suppressed_toast": toasts,
        "callout_scripts": scripts,
    })


if __name__ == "__main__":
    main()
