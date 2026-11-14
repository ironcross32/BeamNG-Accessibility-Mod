"""Golden cases for beam_road::challenge_results, from challenge_results.py.

The local-time wording ("September 4, 2026 at 8:30:05 AM Eastern Daylight Time") is
the machine's time zone, as in the Python; regenerate on the machine that runs the
tests if the zone differs.
"""

from __future__ import annotations

import json
import os
import random
import tempfile
from datetime import datetime, timedelta, timezone

from _common import fbits, outcome, write_fixture

import challenge_results as cr

NAN, INF = float("nan"), float("inf")
EPOCH = datetime(2026, 9, 4, tzinfo=timezone.utc)


def fmt_cases(rnd):
    values = [None, 0, 0.0, -0.0, -5, 0.0004, 0.0005, 0.65, 59.9994, 59.9995, 60, 61.5, 119.9999,
              120.0, 3599.9, 3600.0, 1e6, NAN, INF, "12.5", "x", True, [], {}]
    values += [rnd.uniform(0, 400) for _ in range(40)]
    out = []
    for v in values:
        jv = json.dumps(v)
        out.append({"value": jv,
                    "time": cr._format_time(v), "words": cr._format_elapsed_words(v),
                    "speed_metric": cr._format_speed(v, "metric"), "speed_imperial": cr._format_speed(v, "imperial"),
                    "speed_other": cr._format_speed(v, "Metric"),
                    "dist_metric": cr._format_distance(v, "metric"), "dist_imperial": cr._format_distance(v, "imperial")})
    return out


def datetime_cases():
    texts = ["2026-09-04T12:30:05.123456+00:00", "2026-09-04T12:30:05+00:00", "2026-09-04T00:00:00Z",
             "2026-01-15T23:59:59.5+00:00", "2026-03-08T07:00:00+00:00", "2026-11-01T05:30:00+00:00",
             "2026-07-04T12:00:00-05:00", "2026-07-04T12:00:00+05:30", "2026-07-04 12:00:00",
             "2026-07-04", "20260704T120000", "2026-07-04T12", "2026-07-04T12:30", "garbage", "",
             "2026-02-30T00:00:00", "1960-01-01T00:00:00+00:00", "1970-01-01T00:00:00+00:00",
             "2999-12-31T00:00:00+00:00", "3001-01-01T00:00:00+00:00", " 2026-09-04T12:30:05Z ",
             "2026-09-04T12:30:05.1Z", "2026-09-04T12:30:05.1234567+00:00", "2026-09-04T24:00:00"]
    out = []
    for t in texts:
        out.append({"value": json.dumps(t), **outcome(cr._format_local_datetime, t)})
    for v in [None, 5, []]:
        out.append({"value": json.dumps(v), **outcome(cr._format_local_datetime, v)})
    return out


def surface_cases(rnd):
    out = []
    for x in [None, 0, 7.5, -7.5, 7.51, 10, 10.01, 12.5, 12.6, -20, "3", NAN]:
        for y in [None, -780, -780.01, 0, 2265, 2265.01, "100", INF]:
            out.append({"x": json.dumps(x), "y": json.dumps(y), "out": cr._surface_for_position(x, y)})
    return out


def sample_rows(rnd, n):
    rows = []
    t = 0.0
    y = -780.0
    for _ in range(n):
        t += rnd.choice([0.05, 0.05, 0.1, 0.3, 0.5])
        y += rnd.uniform(0, 3)
        x = rnd.choice([0.0, 2.0, -8.0, 11.0, 15.0, -13.0, 200.0])
        slip_kind = rnd.choice(["none", "none", "wheelspin", "lockup"])
        rows.append({
            "kind": "sample", "t": round(t, 4),
            "road": {"state": rnd.choice(["onRoad", "offRoad", "dormant"])} if rnd.random() < 0.9 else None,
            "telemetry": {"position": {"x": x, "y": y, "z": rnd.choice([50 + (y + 780) * 0.04, None])},
                          "ground_speed_ms": rnd.choice([0.0, 0.4, 0.6, 10.0, 22.0, None]),
                          "throttle": rnd.choice([0.0, 0.2, 1.0]), "brake": rnd.choice([0.0, 0.5]),
                          "traction_control_active": rnd.random() < 0.2},
            "surface": cr._surface_for_position(x, y) if rnd.random() < 0.95 else None,
            "contacts": {"rumble": rnd.random() < 0.2, "gravel": rnd.random() < 0.2},
            "slip": {"active": slip_kind != "none", "kind": slip_kind, "magnitude_mps": rnd.uniform(0, 10),
                     "slide": rnd.random() < 0.2, "slide_deg": rnd.uniform(0, 40)},
        })
    return rows


def analyze_cases(rnd):
    out = []
    events = [{}, {"official_time_s": 6.0, "raw_time_s": 1.0, "penalty_s": 5.0, "recovery_count": 1},
              {"official_time_s": 10.0, "penalty_s": 2.5}, {"race_time_s": 7.25}, {"penalty_s": -3},
              {"official_time_s": "x", "raw_time_s": NAN, "recovery_count": 2.9, "official_confirmed": 1},
              {"race_time_s": 0}]
    for _ in range(30):
        samples = sample_rows(rnd, rnd.randrange(0, 40))
        checkpoints = [{"index": 1, "name": "Checkpoint 1", "time_s": 0.65, "speed_mps": 14.0}] if rnd.random() < 0.5 else []
        event = rnd.choice(events)
        res = outcome(cr.analyze_attempt, samples, checkpoints, event)
        if "ok" in res:
            res["ok"] = json.dumps(res["ok"], indent=2)
        out.append({"samples": json.dumps(samples), "checkpoints": json.dumps(checkpoints),
                    "event": json.dumps(event), **res})
    return out


def summary_cases(rnd):
    base = {
        "status": "completed", "started_utc": "2026-09-04T12:30:05.123456+00:00",
        "ended_utc": "2026-09-04T12:31:10+00:00",
        "statistics": {"official_time_s": 65.25, "raw_time_s": 60.25, "penalty_s": 5.0,
                       "distance_m": 1234.5, "elevation_gain_m": 120.0, "average_speed_mps": 20.5,
                       "moving_average_speed_mps": 21.0, "max_speed_mps": 40.0, "finish_speed_mps": 30.0,
                       "durations": {"stopped_s": 1.5, "asphalt_s": 50.0, "wheelspin_s": 2.0},
                       "departures": {"left": 1, "right": 2, "total": 3},
                       "max_lateral_excursion_m": 11.0,
                       "slip": {"wheelspin_episodes": 2, "lockup_episodes": 0, "max_magnitude_mps": 5.5,
                                "slide_episodes": 1, "max_slide_deg": 12.5},
                       "recoveries": 1,
                       "checkpoints": [{"index": 1, "time_s": 30.5, "speed_mps": 20.0},
                                       {"index": 2, "time_s": 75.0, "speed_mps": 25.0}]},
        "personal_best": {"new_best": False, "previous_best_s": 64.0, "delta_s": 1.25},
    }
    variants = [base]
    tweaks = [
        ("status", "aborted"), ("status", None), ("personal_best", {"new_best": True}),
        ("personal_best", {"delta_s": -2.5}), ("personal_best", {"delta_s": None}), ("personal_best", None),
        ("statistics", None), ("statistics", []), ("statistics", {"penalty_s": 0}),
        ("statistics", {"penalty_s": 0.1}), ("statistics", {"penalty_s": 1e-7}),
        ("statistics", {"slip": {"max_slide_deg": 2.5}}), ("statistics", {"slip": {"max_slide_deg": None}}),
        ("statistics", {"slip": {"max_slide_deg": "3"}}), ("statistics", {"slip": None}),
        ("statistics", {"departures": None}), ("statistics", {"departures": {"left": "2"}}),
        ("statistics", {"checkpoints": {"a": 1}}), ("statistics", {"checkpoints": [5]}),
        ("statistics", {"checkpoints": 7}), ("statistics", {"checkpoints": [{}]}),
        ("statistics", {"durations": []}), ("statistics", {"recoveries": None}),
        ("started_utc", "1960-01-01T00:00:00+00:00"), ("started_utc", None), ("ended_utc", 5),
        ("statistics", {"official_time_s": 3725.5}),
    ]
    for key, value in tweaks:
        v = json.loads(json.dumps(base))
        v[key] = value
        variants.append(v)
    out = []
    for v in variants:
        for units in ("imperial", "metric"):
            out.append({"summary": json.dumps(v), "units": units,
                        "report": outcome(cr.HillClimbChallengeRecorder.report_lines, v, units),
                        "history": outcome(cr.HillClimbChallengeRecorder.history_line, v, units),
                        "speech": outcome(cr.completion_speech, v)})
    return out


def recorder_case(rnd):
    micros = [0]
    clock = lambda: 100.0 + micros[0] / 1e6  # noqa: E731
    now_utc = lambda: EPOCH + timedelta(microseconds=micros[0])  # noqa: E731
    log = []
    script = []
    with tempfile.TemporaryDirectory() as tmp:
        esc = tmp.replace("\\", "\\\\")

        def clean(text):
            return text.replace(esc, "<DIR>")

        def finalize(summary, auto):
            log.append([clean(json.dumps(summary)), auto,
                        cr.HillClimbChallengeRecorder.report_lines(summary, "metric"),
                        cr.HillClimbChallengeRecorder.history_line(summary), cr.completion_speech(summary)])

        rec = cr.HillClimbChallengeRecorder(tmp, clock=clock, now_utc=now_utc, finalize_callback=finalize)
        attempt = 0
        pending = False
        for _ in range(rnd.randrange(30, 80)):
            micros[0] += rnd.choice([50_000, 50_000, 100_000, 500_000, 1_234_567])
            r = rnd.random()
            if pending:
                kind = rnd.choice(["attempt_aggregated", "aborted", "started"])
            elif r < 0.55:
                kind = "record"
            else:
                kind = rnd.choice(["started", "started", "checkpoint", "race_complete", "attempt_aggregated",
                                   "aborted", "mission_stopped", "bogus", "other_challenge"])
            start = len(log)
            if kind == "record":
                packet = rnd.choice([{"state": "onRoad", "diagnostic": {"contactMaterials": "20:rumble_strip"}},
                                     {"state": "offRoad"}, None, {"diagnostic": None}])
                y = rnd.uniform(-800, 2300)
                tel = rnd.choice([
                    {"position": {"x": rnd.uniform(-15, 15), "y": y, "z": 50.0}, "ground_speed_ms": rnd.uniform(0, 30),
                     "wheel_speed_ms": rnd.uniform(0, 35), "throttle": 0.5, "brake": 0.0,
                     "traction_control_active": rnd.random() < 0.2},
                    None, {}])
                out = rec.record(packet, tel)
                op = {"op": "record", "packet": json.dumps(packet), "telemetry": json.dumps(tel), "out": out}
            else:
                event = {"type": "challenge_event", "challenge_id": "hill_climb" if kind != "other_challenge" else "drift",
                         "event": kind if kind != "other_challenge" else "started"}
                if kind == "started":
                    attempt += 1
                    event["attempt_id"] = rnd.choice([f"sim/{attempt}", None, "..", f"run {attempt} é"])
                    if rnd.random() < 0.3:
                        event["replace_active"] = True
                if kind == "checkpoint":
                    event.update(checkpoint_index=rnd.choice([1, 2, None]), checkpoint_name=rnd.choice(["Checkpoint 1", None]),
                                 race_time_s=rnd.uniform(0, 100))
                if kind == "race_complete":
                    event.update(race_time_s=rnd.uniform(30, 90), recovery_count=rnd.choice([0, 1]))
                if kind == "attempt_aggregated":
                    official = rnd.uniform(30, 90)
                    event.update(official_time_s=official, raw_time_s=official - 5, penalty_s=5.0,
                                 recovery_count=1)
                    if rnd.random() < 0.4:
                        event["new_best"] = rnd.choice([True, False, None])
                tel = {"ground_speed_ms": 14.0} if kind == "checkpoint" else None
                out = rec.handle_event(event, telemetry=tel)
                op = {"op": "event", "event": json.dumps(event), "telemetry": json.dumps(tel), "out": json.dumps(out)}
                if kind == "race_complete" and rec._attempt:
                    pending = True
                if kind in ("attempt_aggregated", "aborted", "started"):
                    pending = False
            op.update(micros=micros[0], log=log[start:], capturing=rec.is_capturing())
            script.append(op)
        micros[0] += 100_000
        start = len(log)
        rec.shutdown()
        script.append({"op": "shutdown", "micros": micros[0], "log": log[start:], "capturing": rec.is_capturing()})
        reports = [clean(json.dumps(r)) for r in rec.list_reports()]
        files = {}
        for name in sorted(os.listdir(tmp)):
            with open(os.path.join(tmp, name), "rb") as fh:
                files[name] = clean(fh.read().decode("utf-8"))
    return {"script": script, "reports": reports, "files": files}


def main():
    rnd = random.Random(6693)
    write_fixture("challenge_results", {
        "format": fmt_cases(rnd),
        "datetime": datetime_cases(),
        "surface": surface_cases(rnd),
        "contacts": [{"packet": json.dumps(p), **outcome(cr._contact_flags, p)} for p in [
            None, {}, {"diagnostic": None}, {"diagnostic": {"contactMaterials": "20:rumble_strip,30:gravel"}},
            {"diagnostic": {"contactMaterials": 5}}, {"diagnostic": []}, {"diagnostic": [1]},
            {"diagnostic": {"contactMaterials": "Gravel"}}]],
        "analyze": analyze_cases(rnd),
        "summaries": summary_cases(rnd),
        "recorder": [recorder_case(rnd) for _ in range(3)],
    })


if __name__ == "__main__":
    main()
