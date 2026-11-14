"""Golden session scripts for beam_road::driving_assistant, from driving_assistant.py.

Each script is a list of operations (toggle, status, receive, heartbeat, snapshot) at
given monotonic times. Tokens are random in both builds, so payloads name them by
role ("{token}", "{status}", "{old}") and every recorded output spells a token as
"<tokN>", N counting tokens in the order they were created.
"""

from __future__ import annotations

import json
import random
from unittest.mock import patch

from _common import fbits, write_fixture

import driving_assistant as dam


class Clock:
    def __init__(self):
        self.now = 1000.0

    def __call__(self):
        return self.now


KINDS = ["on", "off", "say", "slow", "status", "alive", "route", "route_set", "route_clear",
         "junction", "diagnostic", "choice", "error", "mystery", ["list"], {"d": 1}, None, 5]


def report(rnd):
    value = {"destDistance": rnd.uniform(0, 5000), "speed": rnd.choice([0, 10.0, 30.0])}
    if rnd.random() < 0.7:
        value.update(turnId=rnd.choice([1, 2]), turn=rnd.choice(["left", "right", "keep left"]),
                     turnDistance=rnd.uniform(0, 2000))
    if rnd.random() < 0.1:
        value["turn"] = rnd.choice([5, None])
    if rnd.random() < 0.1:
        value["turnDistance"] = "far"
    if rnd.random() < 0.2:
        value["offRoad"] = True
    return value


def payload(rnd, seq):
    kind = rnd.choice(KINDS)
    value = {"token": rnd.choice(["{token}"] * 6 + ["{status}", "{old}", "bogus", None]),
             "kind": kind}
    r = rnd.random()
    if r < 0.85:
        value["sequence"] = seq
    elif r < 0.9:
        value["sequence"] = rnd.choice(["7", 2.9, None, "x", float("inf")])
    if rnd.random() < 0.8:
        value["message"] = rnd.choice(["", "Driving assistant on", "left,right", "right,straight,x",
                                       "Turn now", "Destination reached. Assistant off"])
    elif rnd.random() < 0.3:
        value["message"] = 5
    if rnd.random() < 0.5:
        value["vehicle"] = rnd.choice([42, "car", None])
    if rnd.random() < 0.3:
        value["active"] = rnd.choice([True, False, 1])
    if kind in ("route", "route_set") or rnd.random() < 0.1:
        value["data"] = report(rnd) if rnd.random() < 0.85 else rnd.choice([None, "x", []])
    elif kind == "off" and rnd.random() < 0.6:
        value["data"] = {"arrival": rnd.choice([
            {"offRoad": True, "distance": 100, "bearing": -90},
            {"offRoad": False},
            {"offRoad": True, "distance": 30, "bearing": "x"},
            "notadict"])}
    elif kind == "say" and rnd.random() < 0.5:
        value["data"] = {"protect": rnd.choice([True, False, 1])}
    elif kind == "error":
        value["data"] = rnd.choice([{"detail": "lua:1: boom"}, {"detail": 5}, None])
    return value


def run_script(rnd, units):
    clock = Clock()
    log = []
    tokens = {}

    def tok(text):
        for value, name in tokens.items():
            text = text.replace(value, name)
        return text

    def note(value):
        if value and value not in tokens:
            tokens[value] = f"<tok{len(tokens) + 1}>"

    client = dam.DrivingAssistant(
        lambda text: log.append(["send", tok(text)]),
        lambda text, **kw: log.append(["speak", text, kw.get("interrupt"), kw.get("exclude_from_buffer")]),
        cue=lambda kind, choices=(): log.append(["cue", kind, list(choices)]),
        units=lambda: units,
        on_arrival=lambda arrival: log.append(["arrival", json.dumps(arrival)]),
    )
    ops = []
    old = None
    seq = 0
    with patch.object(dam.time, "monotonic", clock):
        for _ in range(rnd.randrange(20, 70)):
            clock.now += rnd.choice([0.1, 0.5, 0.5, 1.0, 2.0, 6.0])
            start = len(log)
            r = rnd.random()
            if r < 0.12:
                if client.token:
                    old = client.token
                client.toggle()
                note(client.token)
                op = {"op": "toggle"}
            elif r < 0.17:
                client.status()
                note(client.status_token)
                op = {"op": "status"}
            elif r < 0.27:
                client.heartbeat()
                op = {"op": "heartbeat"}
            elif r < 0.3:
                op = {"op": "raw", "payload": rnd.choice(["bad", "null", "[]", '{"token":false}',
                                                          '{"sequence":null}', '"x"', "5", '{"kind":"on"}'])}
                client.receive(op["payload"])
            else:
                seq += rnd.choice([1, 1, 1, 0, -1, 3])
                value = payload(rnd, seq)
                text = json.dumps(value)
                real = (text.replace('"{token}"', json.dumps(client.token))
                        .replace('"{status}"', json.dumps(client.status_token))
                        .replace('"{old}"', json.dumps(old)))
                op = {"op": "receive", "payload": text}
                try:
                    client.receive(real)
                    op["error"] = None
                except Exception as exc:  # noqa: BLE001
                    op["error"] = [type(exc).__name__, str(exc)]
            snap = client.snapshot()
            op.update(now=fbits(clock.now), log=log[start:],
                      snapshot=json.dumps({k: v for k, v in snap.items() if k != "response_age_s"}),
                      response_age=None if snap["response_age_s"] is None else fbits(snap["response_age_s"]),
                      since_arrival=(lambda v: None if v is None else fbits(v))(client.seconds_since_arrival()),
                      token=tok(client.token or ""), status_token=tok(client.status_token or ""))
            ops.append(op)
    return {"units": units, "ops": ops}


def main():
    rnd = random.Random(4449)
    scripts = [run_script(rnd, rnd.choice(["imperial", "metric"])) for _ in range(40)]
    write_fixture("driving_assistant", {"scripts": scripts})


if __name__ == "__main__":
    main()
