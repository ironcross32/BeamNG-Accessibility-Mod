"""Golden cases for beam_reports::fuel_stations, from fuel_stations.py."""

from __future__ import annotations

import json
import random

from _common import fbits, write_fixture
from _pyvalue import enc

import fuel_stations as fs

ODD = [None, True, False, 0, 1, 3, -2, 2.7, -0.0, float("nan"), float("inf"), "", "x", "4", "2.5",
       [], ["a"], {}, {"k": 1}, 10**400]


def row_out(r):
    return {"id": r["id"], "name": r["name"], "official": r["official"], "energy": r["energy"],
            "distance": fbits(r["distance"]), "bearing": fbits(r["bearing"]), "pumps": str(r["pumps"]),
            "compatible": enc(r["compatible"]), "target": r["target"]}


def outcome_row(packet):
    try:
        return {"ok": row_out(fs.parse_row(packet))}
    except Exception as exc:  # noqa: BLE001
        return {"err": type(exc).__name__, "msg": str(exc)}


def main():
    rnd = random.Random(4486)
    good = {"id": "st1", "name": "Gas & Go", "official": True, "energy": ["gasoline", "diesel"],
            "distance": 812.4, "bearing": -33.0, "pumps": 4, "compatible": True, "target": False}
    packets = ["", "FS", "FS:", "FS:{", "FS:[]", "FS:null", "FS:{}", 'FS:{"id":""}', 'FS:{"id":0}',
               'FS:{"id":1}', 'FS:{"id":"a","distance":"x"}', 'FS:{"id":"a","distance":null}',
               'FS:{"id":"a","bearing":[1]}', 'FS:{"id":"a","distance":NaN}', 'FS:{"id":"a","bearing":Infinity}',
               'FS:{"id":"a","pumps":"x"}', 'FS:{"id":"a","pumps":[1]}', 'FS:{"id":"a","pumps":Infinity}',
               'FS:{"id":"a","pumps":NaN}', 'FS:{"id":"a","pumps":2.9}', 'FS:{"id":"a","pumps":"7"}',
               'FS:{"id":"a","distance":1' + "0" * 400 + "}", 'FS:{"id":"a","energy":"gasoline"}',
               'FS:{"id":"a","energy":[1,null,true,2.5,{"x":"it\'s"}]}', 'FS:{"id":["x"],"name":0}',
               'FS:{"id":"a","name":5,"official":null,"target":"no","compatible":false}',
               "FS_EVENT:{}", "FS:" + json.dumps(good), " FS:{}"]
    for _ in range(150):
        r = dict(good)
        for _ in range(rnd.randint(1, 4)):
            k = rnd.choice(list(good))
            if rnd.random() < 0.15:
                r.pop(k, None)
            elif k == "distance":
                r[k] = rnd.choice([0.0, 0.99, 1.0, 304.8, 999.0, 1500.0, 20000.0, -5.0, rnd.uniform(0, 30000)])
            elif k == "bearing":
                r[k] = rnd.choice([0.0, 22.5, -22.5, 67.4, 67.5, 112.5, -157.5, 157.4, 180.0, 540.0,
                                   rnd.uniform(-720, 720)])
            elif k == "energy":
                r[k] = rnd.sample(["gasoline", "diesel", "kerosine", "n2o", "electricEnergy", "any",
                                   "fuel", "hydrogen", "", "diesel"], rnd.randint(0, 4))
            else:
                r[k] = rnd.choice(ODD[:-1] + [good[k]])
        packets.append("FS:" + json.dumps(r))
    rows = [{"packet": p, **outcome_row(p)} for p in packets]

    lines = []
    for case in rows:
        if "ok" in case:
            r = fs.parse_row(case["packet"])
            for units in ("metric", "imperial"):
                lines.append({"packet": case["packet"], "units": units, "out": fs.row_line(r, units)})

    event_packets = ["", "FS_EVENT:", "FS_EVENT:{", "FS_EVENT:[]", "FS_EVENT:{}", 'FS_EVENT:{"kind":""}',
                     'FS_EVENT:{"kind":0}', 'FS_EVENT:{"kind":"target"}', "FS:{}"]
    events = [
        {"kind": "target"}, {"kind": "target", "here": True, "station": "Gas & Go"},
        {"kind": "target", "station": ""}, {"kind": "target", "station": 5},
        {"kind": "in_range", "station": "S"}, {"kind": "parking"}, {"kind": "parking", "pump": 3, "side": "left"},
        {"kind": "parking", "pump": 0, "side": "up"}, {"kind": "parking", "pump": "2B", "official": False},
        {"kind": "parking", "official": None, "side": "right"}, {"kind": "parked", "official": False},
        {"kind": "parked", "electric": True, "inZone": True, "pump": 2, "side": "right"},
        {"kind": "parked", "inZone": False, "pump": 1}, {"kind": "parked", "inZone": 1, "electric": 0},
        {"kind": "park_refused", "message": "Too fast"}, {"kind": "park_refused"},
        {"kind": "park_failed", "message": "Blocked"}, {"kind": "park_failed", "message": ""},
        {"kind": "error", "message": ["x"]}, {"kind": "park_cancelled"}, {"kind": "cancelled"},
        {"kind": "left_range"}, {"kind": "scan_done"}, {"kind": 5}, {}, {"kind": "parking", "pump": 2.0},
        {"kind": "parking", "pump": 1e300}, {"kind": "target", "station": {"a": 'b"c'}},
    ]
    for e in events:
        event_packets.append("FS_EVENT:" + json.dumps(e))
    parsed_events = []
    for p in event_packets:
        try:
            parsed = fs.parse_event(p)
            parsed_events.append({"packet": p, "ok": enc(parsed), "phrase": enc(fs.event_phrase(parsed))})
        except Exception as exc:  # noqa: BLE001
            parsed_events.append({"packet": p, "err": type(exc).__name__, "msg": str(exc)})

    energy = [{"in": e, "out": fs.energy_phrase(e)} for e in
              [[], [""], ["gasoline"], ["gasoline", "diesel"], ["gasoline", "diesel", "n2o"],
               ["diesel", "diesel"], ["fuel", "any", "electricEnergy", "kerosine"], ["hydrogen", ""],
               ["gasoline", "x", "gasoline", "y"]]]
    directions = [{"b": fbits(b), "out": fs.direction_phrase(b)} for b in
                  [0.0, -0.0, 22.4999, 22.5, -22.5, 67.5, 112.4, 112.5, 157.49, 157.5, -157.5, 180.0,
                   -180.0, 202.5, 359.0, 720.0, float("nan")] + [rnd.uniform(-400, 400) for _ in range(30)]]

    write_fixture("fuel_stations", {
        "constants": {"ROW_PREFIX": fs.ROW_PREFIX, "EVENT_PREFIX": fs.EVENT_PREFIX,
                      "ENERGY_WORDS": [list(kv) for kv in fs._ENERGY_WORDS.items()]},
        "parse_row": rows, "row_line": lines, "events": parsed_events,
        "energy_phrase": energy, "direction_phrase": directions,
    })


if __name__ == "__main__":
    main()
