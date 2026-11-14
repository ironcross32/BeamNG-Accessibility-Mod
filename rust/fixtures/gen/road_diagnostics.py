"""Golden cases for beam_road::road_diagnostics, from road_diagnostics.py.

- analyze: record sets (as NDJSON lines) and the summary text json.dump writes.
- recorder: a whole session run on fake clocks, and the exact bytes of the NDJSON and
  .summary.json files it leaves, with the directory spelled "<DIR>".
"""

from __future__ import annotations

import json
import os
import random
import tempfile
from datetime import datetime, timezone
from unittest.mock import patch

from _common import fbits, write_fixture

import road_diagnostics as rd


def maybe(rnd, p, value):
    return value if rnd.random() < p else None


def record_set(rnd, n):
    rows = []
    t = 0.0
    x = 0.0
    if rnd.random() < 0.5:
        rows.append({"kind": "meta", "schema": 1})
    phase_cycle = ["correct", "unwind", "idle", 5, None]
    for _ in range(n):
        t += rnd.choice([0.05, 0.05, 0.1, 0.2, 1.0, 2.5])
        if rnd.random() < 0.08:
            rows.append({"kind": "marker", "t": rnd.choice([t, "x", None]),
                         "note": rnd.choice(["note", 5, None])} if rnd.random() < 0.8 else {"kind": "marker"})
            continue
        active = rnd.random() < 0.5
        settled = (not active) and rnd.random() < 0.3
        road = {"state": rnd.choice(["onRoad", "onRoad", "offRoad", "dormant", None, 3])}
        if rnd.random() < 0.85:
            road["correction"] = {"active": active, "settled": settled,
                                  "bearing": rnd.uniform(-30, 30),
                                  "severity": rnd.choice([0.1, 0.6, 0.0, None, "0.4"]),
                                  "phase": rnd.choice(phase_cycle)}
            if rnd.random() < 0.1:
                road["correction"].pop("phase")
        r = rnd.random()
        if r < 0.7:
            err = rnd.uniform(-2, 2)
            road["diagnostic"] = {
                "edgeId": rnd.choice(["a|b", "c|d", "", None, 7]),
                "activeBefore": active or settled,
                "targetSide": rnd.choice([1, -1, None]),
                "targetError": err,
                "targetTolerance": rnd.choice([1.125, 0.9, None]),
                "settledHeadingTolerance": rnd.choice([6.0, None]),
                "settledLateralSpeedTolerance": rnd.choice([0.6, 0.5, None]),
                "lateralSpeed": rnd.uniform(-1, 1),
                "headingError": rnd.uniform(-10, 10),
                "correctionBearing": rnd.choice([rnd.uniform(-20, 20), None]),
                "withinLateral": abs(err) < 1.1,
                "withinHeading": rnd.random() < 0.7,
                "withinLateralSpeed": rnd.random() < 0.5,
                "settledCandidate": rnd.random() < 0.4,
                "steeringInput": maybe(rnd, 0.8, rnd.uniform(-1, 1)),
                "steering": maybe(rnd, 0.7, rnd.uniform(-1, 1)),
                "contactMaterials": rnd.choice(["10:ASPHALT,30:RUBBER", "11:DIRT,30:RUBBER", "", None,
                                                " 12:GRAVEL "]),
            }
        elif r < 0.75:
            road["diagnostic"] = {}
        ground = rnd.choice([0.0, 1.0, 5.0, 10.0, 20.0, None, "7.5"])
        x += rnd.choice([0.5, 1.0, 2.0, 60.0])
        telemetry = {
            "position": {"x": x, "y": rnd.choice([0.0, 1.0]), "z": rnd.choice([50.0, 51.5, None])}
            if rnd.random() < 0.9 else {},
            "ground_speed_ms": ground,
            "wheel_speed_ms": rnd.choice([0.0, 10.0, 15.0, 3.0, None]),
            "steering_input": maybe(rnd, 0.8, rnd.uniform(-1, 1)),
            "steering": maybe(rnd, 0.5, rnd.uniform(-1, 1)),
            "throttle": rnd.choice([0.0, 0.5, 1.0, None]),
            "brake": rnd.choice([0.0, 0.8, None]),
            "traction_control_active": rnd.random() < 0.2,
            "pitch_rad": rnd.choice([0.4, -0.1, None]),
        }
        if rnd.random() < 0.4:
            telemetry["wheel_max_driven_ms"] = rnd.uniform(0, 25)
            telemetry["wheel_min_ms"] = rnd.uniform(0, 12)
            telemetry["travel_bearing_deg"] = rnd.choice([0.0, 20.0, -170.0, None])
        audio = {"settled_triggered": True} if settled and rnd.random() < 0.7 else {}
        row = {"kind": "sample", "t": round(t, 6), "road": road, "telemetry": telemetry,
               "audio": audio if rnd.random() < 0.9 else None}
        rows.append(row)
    if rnd.random() < 0.5:
        rows.append({"kind": "end", "t": t})
    return rows


def analyze_cases(rnd):
    out = []
    special = [
        [], [{"kind": "marker", "t": 1.23456, "note": "x"}],
        [{"kind": "sample"}],
        [{"kind": "sample", "t": 1, "road": [1]}],
        [{"kind": "sample", "t": 1, "road": {"diagnostic": [1]}}],
        [{"kind": "sample", "t": 1, "road": {"diagnostic": 0}}],
        [{"kind": "sample", "t": None}],
        [{"kind": "sample", "t": "2.5", "telemetry": {"position": "here"}}],
        [{"kind": "sample", "t": 1, "road": {"state": [1]}}],
        [{"kind": "sample", "t": 1, "road": {"correction": 5}}],
        [{"kind": "sample", "t": 1, "telemetry": {"pitch_rad": 1e308}}],
    ]
    for rows in special:
        out.append(case(rows))
    for _ in range(45):
        out.append(case(record_set(rnd, rnd.randrange(1, 30))))
    return out


def case(rows):
    lines = [json.dumps(r) for r in rows]
    try:
        summary = rd.analyze_records([json.loads(line) for line in lines], path="session.ndjson")
        text = json.dumps(summary, indent=2, allow_nan=False)
        return {"lines": lines, "ok": text}
    except Exception as exc:  # noqa: BLE001
        return {"lines": lines, "err": type(exc).__name__, "msg": str(exc)}


class FakeDatetime(datetime):
    utc = datetime(2026, 9, 4, 12, 30, 5, 123456, tzinfo=timezone.utc)
    local = datetime(2026, 9, 4, 8, 30, 5, 123456)

    @classmethod
    def now(cls, tz=None):
        return cls.utc if tz is not None else cls.local


def recorder_case(rnd):
    clock = [100.0]
    wall = [1788480000.25]
    rows = record_set(rnd, 25)
    script = []
    with tempfile.TemporaryDirectory() as tmp:
        with patch.object(rd, "datetime", FakeDatetime):
            rec = rd.RoadDiagnosticRecorder(tmp, clock=lambda: clock[0], wall_clock=lambda: wall[0])
            label = rnd.choice(["hill / climb", "", "--x--", "Ünïcode label", "a" * 60])
            start = rec.start(label)
            script.append({"clock": fbits(clock[0]), "wall": fbits(wall[0]), "op": "start", "label": label, "out": json.dumps(start).replace(tmp.replace("\\", "\\\\"), "<DIR>")})
            for row in rows:
                clock[0] += 0.05
                wall[0] += 0.05
                if row["kind"] == "marker":
                    note = row.get("note") if isinstance(row.get("note"), str) else "note é"
                    out = rec.mark(note)
                    script.append({"clock": fbits(clock[0]), "wall": fbits(wall[0]), "op": "mark", "note": note, "out": json.dumps(out).replace(tmp.replace("\\", "\\\\"), "<DIR>")})
                elif row["kind"] == "sample":
                    ok = rec.record(row["road"], row["telemetry"], row["audio"])
                    script.append({"clock": fbits(clock[0]), "wall": fbits(wall[0]), "op": "record", "road": json.dumps(row["road"]),
                                   "telemetry": json.dumps(row["telemetry"]),
                                   "audio": json.dumps(row["audio"]), "out": ok})
            clock[0] += 0.05
            summary = rec.stop()
            names = sorted(os.listdir(tmp))
            files = {}
            for name in names:
                with open(os.path.join(tmp, name), "rb") as fh:
                    files[name] = fh.read().decode("utf-8").replace(tmp.replace("\\", "\\\\"), "<DIR>")
            script.append({"clock": fbits(clock[0]), "wall": fbits(wall[0]), "op": "stop", "out": json.dumps(summary).replace(tmp.replace("\\", "\\\\"), "<DIR>")})
    return {"clock0": fbits(100.0), "wall0": fbits(1788480000.25), "script": script, "files": files}


def main():
    rnd = random.Random(4463)
    write_fixture("road_diagnostics", {
        "constants": {
            "SCHEMA_VERSION": rd.SCHEMA_VERSION,
            "REPEATED_SETTLED_WINDOW_S": fbits(rd.REPEATED_SETTLED_WINDOW_S),
            "STEERING_DEADZONE": fbits(rd.STEERING_DEADZONE),
            "BEARING_DEADZONE_DEG": fbits(rd.BEARING_DEADZONE_DEG),
            "SLIP_SUSTAIN_S": fbits(rd.SLIP_SUSTAIN_S),
        },
        "analyze": analyze_cases(rnd),
        "recorder": [recorder_case(rnd) for _ in range(4)],
    })


if __name__ == "__main__":
    main()
