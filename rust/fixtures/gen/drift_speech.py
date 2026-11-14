"""Golden cases for beam_reports::drift_speech, from drift_speech.py."""

from __future__ import annotations

import random

from _common import write_fixture
from _pyvalue import enc, outcome_enc

import drift_speech as ds

ODD = [None, True, False, 0, 1, -1, 450, 1240, 18916, 10**30, 0.0, -0.0, 1.62, 2.5, -3.7, 1e20,
       float("nan"), float("inf"), "", "0", "12", " 7 ", "1_000", "٣", "x", "1.5", [], {}, [1], 10**400]
TEXTS = [None, 5, "", "  ", "Great Drift!", "Close call! +80", "+ 1200 bonus", "+ 5", "+x", "++3",
         "a+١٢b", "+0007", "Score: 12000.", "+1234567", "Spinout!", "Crashed.", "Lost?", "drift",
         " padded ", "+", "+ ", "+12+34"]


def main():
    rnd = random.Random(1997)
    events = [None, "x", 5, [], {}, {"event": "alive"}, {"event": "nope"}, {"event": 3},
              {"event": "quick", "text": 5}, {"event": "shown"}, {"event": "hidden"},
              {"event": "spot_start"}, {"event": "spot_start", "name": "", "highScore": 0},
              {"event": "spot_start", "name": 5, "goals": "x"},
              {"event": "spot_start", "name": "Docks", "highScore": 3200,
               "goals": [{"points": 1000, "done": True}, {"points": 5000, "done": False}, 3,
                         {"points": "x"}, {"points": 7, "done": 0}, {"points": float("inf")}]},
              {"event": "spot_start", "goals": [{"points": float("inf")}]},
              {"event": "fail"}, {"event": "fail", "text": "", "lost": 5, "total": 7}]
    kinds = ["shown", "hidden", "slide", "combo", "tier", "quick", "spot_done", "spot_failed",
             "spot_highscore", "out_of_bounds", "fail", "scored", "wrapup", "spot_start", "alive", "x"]
    for _ in range(500):
        ev = {"event": rnd.choice(kinds)}
        for key in ("points", "combo", "text", "direction", "lost", "total", "name", "highScore", "goals"):
            if rnd.random() < 0.45:
                if key == "text":
                    ev[key] = rnd.choice(TEXTS)
                elif key == "direction":
                    ev[key] = rnd.choice(["left", "right", "up", None, 1])
                elif key == "name":
                    ev[key] = rnd.choice(["Docks", "", None, 7])
                elif key == "goals":
                    ev[key] = [{"points": rnd.choice(ODD), "done": rnd.choice([True, False, None, 0])}
                               for _ in range(rnd.randint(0, 3))]
                else:
                    ev[key] = rnd.choice(ODD)
        events.append(ev)
    phrases = [{"event": enc(e), **outcome_enc(ds.drift_phrase, e)} for e in events]

    # A scripted announcer run: (op, arg) steps with the clock under test control.
    now = [100.0]
    spoken = []
    ann = ds.DriftAnnouncer(lambda text, interrupt, source: spoken.append([text, interrupt, source]),
                            clock=lambda: now[0])
    script = []
    ops = [("quiet", None), ("handle", {"event": "shown", "points": 0, "combo": 1}), ("quiet", None),
           ("advance", 9.0), ("handle", {"event": "alive"}), ("advance", 9.0), ("quiet", None),
           ("advance", 1.0), ("quiet", None), ("advance", 0.0001), ("quiet", None),
           ("handle", {"event": "hidden"}), ("quiet", None), ("handle", {"event": "shown"}),
           ("advance", 10.1), ("quiet", None), ("set_enabled", False),
           ("handle", {"event": "shown", "points": 10, "combo": 1}), ("quiet", None),
           ("set_enabled", True), ("quiet", None), ("handle", "garbage"), ("handle", {"event": 3}),
           ("handle", {"event": "fail", "text": "Spinout!", "lost": 640}), ("quiet", None),
           ("handle", {"event": "combo", "combo": float("inf")}), ("quiet", None),
           ("handle", {"event": "hidden"}), ("quiet", None)]
    for op, arg in ops:
        step = {"op": op}
        before = len(spoken)
        if op == "quiet":
            step["out"] = ann.quiet()
        elif op == "advance":
            now[0] += arg
            step["arg"] = arg
        elif op == "set_enabled":
            ann.set_enabled(arg)
            step["arg"] = arg
        else:
            step["arg"] = enc(arg)
            try:
                ann.handle(arg)
                step["raised"] = None
            except Exception as exc:  # noqa: BLE001
                step["raised"] = type(exc).__name__
        step["spoken"] = spoken[before:]
        script.append(step)

    write_fixture("drift_speech", {
        "QUIET_TIMEOUT_S": ds.QUIET_TIMEOUT_S,
        "drift_phrase": phrases,
        "announcer_script": script,
    })


if __name__ == "__main__":
    main()
