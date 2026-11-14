"""Golden cases for beam_road::road_guidance, from road_guidance.py."""

from __future__ import annotations

import copy
import json
import random

from _common import fbits, outcome, write_fixture

import road_guidance as rg

NAN, INF = float("nan"), float("inf")


def dumped(fn, *args, **kwargs):
    """outcome(), with an ok value replaced by its json.dumps text."""
    res = outcome(fn, *args, **kwargs)
    if "ok" in res:
        res["ok"] = json.dumps(res["ok"])
    return res


BASE = {
    "state": "onRoad",
    "oneWay": False,
    "roadDirections": [4, -176],
    "offRoad": None,
    "correction": {"active": True, "bearing": 22, "severity": 0.5, "phase": "correct",
                   "settled": False, "lateralRatio": 0.82, "headingError": 7, "timeToEdge": 1.4},
    "diagnostic": {"edgeId": "a|b", "edgeT": 0.5, "steering": -0.2, "withinLateral": True,
                   "contactMaterials": "10:ASPHALT", "targetSide": 1, "rearmTicks": 0},
    "junction": {"id": " n1+n2 ", "phase": "approach", "kind": " crossroads ", "distance": 60.96,
                 "exits": [90, 0, -90], "entered": False},
    "signalStatus": "available",
    "signalContext": "map:1",
    "signal": {"id": "s1", "kind": "trafficLight", "state": "red", "phase": "approach",
               "distance": 35, "groupId": "g1", "preview": False},
    "proximity": {"id": "junction:T", "distance": 120, "speed": 20, "action": "approach",
                  "source": "junctionBoundary", "signedDistance": 120, "kind": "gate",
                  "privateEntry": "private:g2"},
    "speedLimit": 13.9,
    "orientation": True,
}

POOL = [None, True, False, 0, -1, 1, 0.5, -0.0, 1e9, -60, 2000, "x", "", " ", "12", "nan", "1e3",
        [], [1], [1, 2, 3, 4, 5, 6, 7, 8, 9], {}, {"a": 1}, "approach", "near", "onRoad",
        "offRoad", "dormant", "stop", "red", "unknown", "stopSign", "trafficLight", "correct",
        "unwind", "idle", "hold", "available", "none", "inactive", "unsupported", "a" * 41,
        "b" * 161, "c" * 201, "junctionBoundary", "stopPoint", "deadEnd", "fork"]


def mutations(rnd):
    paths = []
    for key, value in BASE.items():
        paths.append((key,))
        if isinstance(value, dict):
            for sub in value:
                paths.append((key, sub))
    # extra keys the parser looks for but BASE does not carry
    for key in ("lateralSpeed", "headingError", "shouldEnter", "speed", "clearTicksAfter"):
        paths.append(("diagnostic", key))
    out = []
    for path in paths:
        for value in rnd.sample(POOL, 5) + ["<delete>"]:
            pkt = copy.deepcopy(BASE)
            target = pkt
            for p in path[:-1]:
                target = target[p] if isinstance(target, dict) else {}
            if not isinstance(target, dict):
                continue
            if value == "<delete>":
                target.pop(path[-1], None)
            else:
                target[path[-1]] = value
            out.append(pkt)
    return out


def packet_texts(rnd):
    texts = [
        "", "R2|", "R2|not json", "r2|{}", 'R2|{"state":"flying"}', "R2|[]", "R2|null", "R2|5",
        'R2|{"state":"onRoad"}', 'R2|{"state":"offRoad"}', 'R2|{"state":"dormant"}',
        'R2|{"state":["onRoad"]}', 'R2|{"state":{"a":1}}', 'R2|{"state":null}',
        'R2|{"state":"onRoad","roadDirections":[NaN]}',
        'R2|{"state":"onRoad","roadDirections":{"bad":1}}',
        'R2|{"state":"onRoad","roadDirections":{}}', 'R2|{"state":"onRoad","roadDirections":null}',
        'R2|{"state":"onRoad","correction":{"active":true,"phase":"hold"}}',
        'R2|{"state":"onRoad","correction":{"active":true,"phase":["x"]}}',
        'R2|{"state":"onRoad","correction":{"active":true,"phase":null}}',
        'R2|{"state":"onRoad","correction":{"active":false}}',
        'R2|{"state":"onRoad","correction":{"active":1}}',
        'R2|{"state":"onRoad","correction":{"active":true,"bearing":null}}',
        'R2|{"state":"onRoad","correction":{"active":true,"lateralRatio":null,"timeToEdge":-3,"headingError":540}}',
        'R2|{"state":"onRoad","junction":{"id":"x","phase":"later"}}',
        'R2|{"state":"onRoad","junction":{"id":"x","phase":[1]}}',
        'R2|{"state":"onRoad","junction":{"id":"x","phase":"near","exits":{}}}',
        'R2|{"state":"onRoad","junction":{"id":"x","phase":"near","entered":1}}',
        'R2|{"state":"onRoad","junction":{"id":"   ","phase":"near"}}',
        'R2|{"state":"onRoad","diagnostic":{"withinLateral":1}}',
        'R2|{"state":"onRoad","diagnostic":{"contactMaterials":[]}}',
        'R2|{"state":"onRoad","diagnostic":{"edgeT":null}}',
        'R2|{"state":"onRoad","diagnostic":{"edgeId":null,"contactMaterials":null,"edgeT":"7"}}',
        'R2|{"state":"onRoad","speedLimit":10}', 'R2|{"state":"offRoad","speedLimit":10}',
        'R2|{"state":"onRoad","speedLimit":0}', 'R2|{"state":"onRoad","speedLimit":true}',
        'R2|{"state":"onRoad","speedLimit":"nan"}', 'R2|{"state":"onRoad","speedLimit":null}',
        'R2|{"state":"onRoad","speedLimit":' + "9" * 400 + "}",
        'R2|{"state":"onRoad","signalStatus":"available"}',
        'R2|{"state":"onRoad","orientation":1}', 'R2|{"state":"onRoad","orientation":true}',
        'R2|{"state":"offRoad","offRoad":{"bearing":-200,"distance":-3}}',
        'R2|{"state":"offRoad","offRoad":{"bearing":1}}', 'R2|{"state":"offRoad","offRoad":[]}',
        'R2|{"state":"onRoad","proximity":{"id":"p","distance":5,"speed":1,"action":"stop","source":"junctionBoundary"}}',
        'R2|{"state":"onRoad","proximity":{"id":"p","distance":5,"speed":1,"action":"stop","source":"stopPoint"}}',
        'R2|{"state":"onRoad","proximity":{"id":"p","distance":5,"speed":1,"action":"approach","source":"stopPoint","signedDistance":-50}}',
        'R2|{"state":"onRoad","proximity":{"id":"p","distance":5,"speed":1,"action":"approach","source":"stopPoint","signedDistance":-50.1}}',
        'R2|{"state":"onRoad","proximity":{"id":"p","distance":5,"speed":1,"action":"approach","source":"stopPoint","signedDistance":null}}',
        'R2|{"state":"offRoad","proximity":{"id":"p","distance":5,"speed":1,"action":"approach","source":"stopPoint"}}',
        'R2|{"state":"onRoad","signalStatus":"available","signal":{"id":"1","kind":"stopSign","state":"stop","phase":"near","distance":3,"preview":true}}',
        'R2|{"state":"onRoad","signalStatus":"available","signal":{"id":"1","kind":"trafficLight","state":"red","phase":"near","distance":3,"preview":true}}',
        'R2|{"state":"onRoad","signalStatus":"available","signal":{"id":"1","kind":"trafficLight","state":"unknown","phase":"near","distance":1000,"groupId":null}}',
        'R2|{"state":"onRoad","signalStatus":"none","signal":{"id":"1","kind":"stopSign","state":"stop","phase":"near","distance":3}}',
        'R2|{"state":"onRoad","signalStatus":"available","signal":{"id":"1","kind":"stopSign","state":"red","phase":"near","distance":3}}',
        'R2|{"state":"onRoad","signalContext":5}', 'R2|{"state":"onRoad","signalContext":"' + "x" * 201 + '"}',
        'R2|{"state":"onRoad","oneWay":"yes"}', 'R2|{"state":"onRoad","oneWay":true,"roadDirections":[1,2,3,4,5,6,7,8]}',
        'R2|{"state":"onRoad","roadDirections":[1,2,3,4,5,6,7,8,9]}', 'R2|{"state":"onRoad","roadDirections":["12", 1e3, -0.0]}',
        'R2|{"state":"onRoad"} trailing', 'R2| {"state":"onRoad"} ',
    ]
    texts.append("R2|" + json.dumps(BASE))
    for pkt in mutations(rnd):
        texts.append("R2|" + json.dumps(pkt))
    return texts


def junctions(rnd):
    out = []
    kinds = ["intersection", "fork", "tJunction", "crossroads", "complex", "deadEnd", "gate", "private",
             "roundabout", "x"]
    exit_sets = [[], [0], [90, -90], [90, 0, -90], [30, -30], [179, -179, 170], [20, 20, 160, -16],
                 [45, 45.0001, -45, -120, 120.5, 151], [NAN], [10, INF]]
    for kind in kinds:
        for exits in exit_sets:
            for dist in (0.0, 9.1, 60.96, -5.0, 1e6):
                out.append({"id": "j", "phase": "approach", "entered": False, "kind": kind,
                            "distance": dist, "exits": exits})
    rnd.shuffle(out)
    return out[:250]


def signals():
    out = []
    for kind, states in (("stopSign", ["stop"]), ("trafficLight", sorted(rg._SIGNAL_STATES - {"stop"}) + ["weird"])):
        for state in states:
            for preview in (False, True):
                for dist in (0.0, 8.0, 35.0, 1000.0):
                    out.append({"id": "s", "kind": kind, "state": state, "phase": "near",
                                "distance": dist, "groupId": "g", "preview": preview})
    return out


class Clock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now


def feed_script(rnd):
    pool = []
    for state in ("onRoad", "onRoad", "onRoad", "offRoad", "dormant"):
        for _ in range(6):
            pkt = {"state": state, "signalContext": rnd.choice(["map:1", "map:1", "map:2"]),
                   "oneWay": rnd.random() < 0.3,
                   "roadDirections": rnd.sample([0, 4, -176, 90, -90, 30, 160, -12], rnd.randrange(4))}
            if state == "offRoad" and rnd.random() < 0.7:
                pkt["offRoad"] = {"bearing": rnd.uniform(-180, 180), "distance": rnd.uniform(0, 80)}
            if rnd.random() < 0.6:
                pkt["correction"] = {"active": rnd.random() < 0.6, "bearing": rnd.uniform(-90, 90),
                                     "severity": rnd.random(),
                                     "phase": rnd.choice(["correct", "unwind", "idle"])}
            if state == "onRoad":
                if rnd.random() < 0.6:
                    pkt["speedLimit"] = rnd.choice([8.333333333333, 13.9, 22.2, 27.78])
                if rnd.random() < 0.5:
                    pkt["junction"] = {"id": rnd.choice(["j1", "j2"]),
                                       "phase": rnd.choice(["approach", "near"]),
                                       "entered": rnd.random() < 0.2,
                                       "kind": rnd.choice(["fork", "crossroads", "deadEnd", "tJunction"]),
                                       "distance": rnd.uniform(0, 200),
                                       "exits": rnd.sample([90, 0, -90, 45], rnd.randrange(4))}
                pkt["signalStatus"] = rnd.choice(["available", "available", "none", "inactive", "unsupported"])
                if pkt["signalStatus"] == "available":
                    if rnd.random() < 0.5:
                        pkt["signal"] = {"id": "s1", "kind": "stopSign", "state": "stop",
                                         "phase": rnd.choice(["approach", "near"]),
                                         "distance": rnd.uniform(0, 100), "groupId": rnd.choice(["g1", "g2"])}
                    else:
                        pkt["signal"] = {"id": "l1", "kind": "trafficLight",
                                         "state": rnd.choice(["red", "green", "yellow", "unknown"]),
                                         "phase": rnd.choice(["approach", "near"]),
                                         "distance": rnd.uniform(0, 100), "groupId": "lights"}
                if rnd.random() < 0.1:
                    pkt["orientation"] = True
            pool.append("R2|" + json.dumps(pkt))

    clock = Clock()
    feed = rg.RoadGuidanceFeed(clock)
    ops = []
    for step in range(500):
        clock.now += rnd.choice([0.05, 0.1, 0.2, 0.2, 0.3, 0.8, 1.2, 3.5])
        r = rnd.random()
        op = {"now": fbits(clock.now)}
        if r < 0.6:
            text = rnd.choice(pool)
            flags = [rnd.random() < 0.85 for _ in range(3)]
            explicit = rnd.random() < 0.3
            events = feed.accept_r2(rg.parse_r2_packet(text), now=clock.now if explicit else None,
                                    stop_signs=flags[0], traffic_lights=flags[1], speed_limits=flags[2])
            op.update(op="accept_r2", text=text, flags=flags, explicit=explicit, out=json.dumps(events))
        elif r < 0.7:
            state = rnd.choice(["DORMANT", "ON_ROAD", "OFF_ROAD"])
            bearing = rnd.choice([0.0, 45.0, -100.0, 170.0, NAN])
            distance = rnd.choice([0.0, 10.0, -3.0, 250.5])
            directions = rnd.choice([[], [10.0, -170.0]])
            out = feed.accept_legacy(state, bearing=bearing, distance=distance, directions=directions)
            op.update(op="accept_legacy", state=state, bearing=fbits(bearing), distance=fbits(distance),
                      directions=directions, out=out)
        elif r < 0.78:
            op.update(op="check_timeout", out=feed.check_timeout())
        elif r < 0.83:
            op.update(op="r2_recent", out=feed.r2_recent())
        elif r < 0.86:
            feed.arm_orientation()
            op.update(op="arm_orientation")
        elif r < 0.87:
            feed.reset()
            op.update(op="reset")
        else:
            enabled = rnd.random() < 0.9
            units = rnd.choice(["metric", "imperial"])
            op.update(op="status", enabled=enabled, units=units,
                      status=outcome(feed.status_phrase, enabled, units),
                      speed=feed.speed_limit_phrase(enabled, units),
                      mode=feed.snapshot()["mode"])
        ops.append(op)
    return ops


def main():
    rnd = random.Random(4462)
    packets = [{"text": t, **dumped(rg.parse_r2_packet, t)} for t in packet_texts(rnd)]

    bearings = [0.0, -0.0, 15.0, -15.0, 15.0000001, 45.0, -45.0, 45.0001, 120.0, -120.0, 120.0001,
                150.0, -150.0, 150.0001, 179.9, 180.0, -180.0, 190.0, 345.0, 540.0, -900.0, NAN, INF, -INF]
    bearings += [rnd.uniform(-720, 720) for _ in range(40)]
    labels = [{"bearing": fbits(b), "u_turn": u, **outcome(rg.direction_label, b, u_turn=u)}
              for b in bearings for u in (False, True)]

    lists = []
    for _ in range(120):
        bs = [rnd.choice(bearings[:21] + [rnd.uniform(-200, 200)]) for _ in range(rnd.randrange(6))]
        conj = rnd.choice(["and", "or"])
        lists.append({"bearings": [fbits(b) for b in bs], "conjunction": conj,
                      "direction_list": outcome(rg.direction_list, bs, conj),
                      "road_direction_list": outcome(rg.road_direction_list, bs, conj)})
    lists.append({"bearings": [fbits(NAN)], "conjunction": "and",
                  "direction_list": outcome(rg.direction_list, [NAN], "and"),
                  "road_direction_list": outcome(rg.road_direction_list, [NAN], "and")})

    one_way = [{"bearing": fbits(b), "out": rg.one_way_phrase(b)}
               for b in [0.0, 10.0, -10.0, 10.5, 92.0, -175.0, 0.4999, NAN, INF] + [rnd.uniform(-180, 180) for _ in range(20)]]

    distances = []
    for m in [0.0, -0.0, -5.0, 0.4, 0.5, 1.5, 2.5, 9.1, 10.0, 60.96, 152.4, 1e6, NAN, INF]:
        for units in ("imperial", "metric", "IMPERIAL", "Imp", "", "freedom"):
            distances.append({"metres": fbits(m), "units": units, "out": rg.format_road_distance(m, units)})

    junction_cases = []
    for j in junctions(rnd):
        for units in ("imperial", "metric"):
            junction_cases.append({"junction": {**j, "distance": fbits(j["distance"]),
                                                "exits": [fbits(e) for e in j["exits"]]},
                                   "units": units, **outcome(rg.junction_phrase, j, units)})

    signal_cases = []
    for s in signals():
        for units in ("imperial", "metric"):
            for change in (False, True):
                signal_cases.append({"signal": {**s, "distance": fbits(s["distance"])}, "units": units,
                                     "change": change, "out": rg.signal_phrase(s, units, change=change)})

    surfaces = [{"text": t, "out": rg.parse_surface_packet(t)} for t in [
        "SURFACE|Dirt|1", "SURFACE|Grass|0|future", "SURFACE|Dirt", "SURFACE||1", "SURFACE|Dirt road|1",
        "SURFACE|Dirt|2", "R2|x", "SURFACE|Asphalt9|1", "SURFACE|Ã‰Ã©Ã©|0", "SURFACE|" + "a" * 20 + "|1",
        "SURFACE|" + "a" * 21 + "|1", "surface|Dirt|1", "SURFACE|Dirt|1|", "", "SURFACE|æ—¥æœ¬|1"]]

    limits = []
    for v in [None, 8.333333333333, 13.9, 0.1, 27.78, 1e-9, 44.704, 2.0 / 3.6 * 0.5 * 3.6, 6.9444]:
        for units in ("metric", "imperial", "Imperial"):
            for label in ("Map speed limit", "Speed limit"):
                limits.append({"speed": None if v is None else fbits(v), "units": units, "label": label,
                               "out": rg.map_speed_limit_phrase(v, units, label)})

    write_fixture("road_guidance", {
        "constants": {"R2_PREFIX": rg.R2_PREFIX, "R2_STALE_SECONDS": fbits(rg.R2_STALE_SECONDS)},
        "packets": packets,
        "direction_label": labels,
        "lists": lists,
        "one_way": one_way,
        "format_road_distance": distances,
        "junction_phrase": junction_cases,
        "signal_phrase": signal_cases,
        "surface": surfaces,
        "speed_limit_phrase": limits,
        "feed": feed_script(rnd),
    })


if __name__ == "__main__":
    main()
