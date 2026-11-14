"""Golden cases for beam_road::assistant_diagnostics, from assistant_diagnostics.py.

Scripts of record / record_road / read / control calls on fake clocks, with every
result and the exact bytes of the session files (directory spelled "<DIR>").
"""

from __future__ import annotations

import json
import os
import random
import tempfile
from datetime import datetime, timedelta, timezone
from unittest.mock import patch

from _common import write_fixture

import assistant_diagnostics as ad
import road_guidance as rg

EPOCH = datetime(2026, 9, 4, 3, 4, 5, 678901, tzinfo=timezone.utc)

PACKETS = [
    'R2|{"state":"onRoad","proximity":{"id":"junction:T","distance":120,"speed":20,"action":"approach","source":"junctionBoundary"},"speedLimit":13.9}',
    'R2|{"state":"offRoad"}',
    'R2|{"state":"onRoad","signalStatus":"available","signal":{"id":"s","kind":"stopSign","state":"stop","phase":"near","distance":8}}',
]

EVENTS = [
    {"kind": "on", "token": "abc", "sequence": 1, "vehicle": 42, "message": "Driving assistant on"},
    {"kind": "diagnostic", "token": "abc", "sequence": 2,
     "data": {"vehicle": {"locked": "j1", "chosen": "left", "automatic": True}}},
    {"kind": "diagnostic", "token": "abc", "sequence": 3,
     "data": {"vehicle": {"locked": "j1", "chosen": "left", "automatic": True}}},
    {"kind": "diagnostic", "sequence": 4, "data": {"vehicle": {"locked": "j2", "chosen": "right", "automatic": False}}},
    {"kind": "diagnostic", "sequence": 5, "data": {"vehicle": {"locked": None, "chosen": "x", "automatic": True}}},
    {"kind": "diagnostic", "sequence": 6, "data": None},
    {"kind": "say", "message": "Gate ahead é", "seq": 99, "t": -1},
    {"kind": "alive", "active": True},
    {"kind": "route", "data": {"turn": "left"}},
    {"kind": "junction", "message": "left,right"},
    {"kind": "error", "data": {"detail": "boom"}},
    {"message": "no kind"},
    {"kind": "off", "message": "Driving assistant off", "value": float("nan")},
]


def run(rnd):
    clock = [500.0]
    stamp = [0]
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        esc = tmp.replace("\\", "\\\\")

        def clean(value):
            return json.dumps(value).replace(esc, "<DIR>")

        with patch.object(ad.time, "monotonic", lambda: clock[0]), \
                patch.object(ad, "datetime", FakeDatetime(stamp)):
            rec = ad.AssistantDiagnosticRecorder(tmp)
            for _ in range(rnd.randrange(30, 70)):
                clock[0] += rnd.choice([0.05, 0.1, 0.19, 0.2, 0.21, 1.0])
                stamp[0] += 1_000_003
                r = rnd.random()
                if r < 0.1:
                    label = rnd.choice([None, "", "test", "drive / junction é", "x" * 70])
                    res = run_call(rec.start, label)
                    op = {"op": "start", "label": label}
                elif r < 0.15:
                    res = run_call(rec.stop)
                    op = {"op": "stop"}
                elif r < 0.45:
                    event = rnd.choice(EVENTS)
                    rec.record(event)
                    res = {"ok": None}
                    op = {"op": "record", "event": json.dumps(event)}
                elif r < 0.65:
                    text = rnd.choice(PACKETS)
                    audio = rnd.choice([{"active": True}, {"active": False, "mode": "pulse"}, None])
                    rec.record_road(rg.parse_r2_packet(text), audio)
                    res = {"ok": None}
                    op = {"op": "record_road", "packet": text, "audio": json.dumps(audio)}
                elif r < 0.75:
                    since = rnd.choice([0, None, 3, "5", 10, -1, "x"])
                    limit = rnd.choice([1, 2, 100, 0, -5, 1000, "3"])
                    res = run_call(rec.read, since, limit)
                    op = {"op": "read", "since": json.dumps(since), "limit": json.dumps(limit)}
                elif r < 0.85:
                    session = rnd.choice([None, None, "../escape.ndjson", "missing.ndjson", "x.txt", "C:a.ndjson"])
                    res = run_call(rec.review, session)
                    op = {"op": "review", "session": session}
                else:
                    action = rnd.choice(["status", "list", "mark", "bogus", "read"])
                    note = rnd.choice([None, "note", 5])
                    limit = rnd.choice([1, 20])
                    res = run_call(rec.control, action, note=note, limit=limit, since_seq=0)
                    op = {"op": "control", "action": action, "note": json.dumps(note), "limit": limit}
                if "ok" in res:
                    res["ok"] = clean(res["ok"])
                elif "err" in res:
                    res["msg"] = res["msg"].replace(tmp, "<DIR>")
                op.update(clock=clock[0], stamp=stamp[0], res=res, status=clean(rec.status()))
                out.append(op)
            rec.stop()
            files = {}
            for name in sorted(os.listdir(tmp)):
                with open(os.path.join(tmp, name), "rb") as fh:
                    files[name] = fh.read().decode("utf-8")
    return {"ops": out, "files": files}


def run_call(fn, *args, **kwargs):
    try:
        return {"ok": fn(*args, **kwargs)}
    except Exception as exc:  # noqa: BLE001
        return {"err": type(exc).__name__, "msg": str(exc)}


class FakeDatetime:
    def __init__(self, stamp):
        self.stamp = stamp

    def now(self, tz=None):
        return EPOCH + timedelta(microseconds=self.stamp[0])


def main():
    rnd = random.Random(4481)
    write_fixture("assistant_diagnostics", {"scripts": [run(rnd) for _ in range(6)]})


if __name__ == "__main__":
    main()
