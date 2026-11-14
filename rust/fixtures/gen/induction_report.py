"""Golden cases for beam_reports::induction_report, from induction_report.py."""

from __future__ import annotations

import json
import random

from _common import write_fixture
from _pyvalue import enc

import induction_report as ir


def psi(value):
    return round(float(value), 1), "psi"


def bar(value):
    return round(float(value) / 14.504, 2), "bar"


SUPERCHARGER = {"boost": 7.26, "boostMax": 9.633, "type": "roots", "twisted": False,
                "blowerRPM": 7220.4, "lossCoef": 0.1083, "peakCoef": 1.4729, "peakRPM": 4225,
                "condition": 1}
TURBO = {"boost": 0, "boostMax": 28.51, "wastegateTarget": 28.505, "turboRPM": 0,
         "maxTurboRPM": 250000, "peakCoef": 2.0786, "peakRPM": 5137, "heat": 320.7,
         "heatLimit": 650, "hot": False, "condition": 1, "blowOff": True, "antilag": False}

ODD = [None, True, False, 0, 1, -1, 0.0, -0.0, 0.5, 2.5, 1e6, 1e300, float("nan"), float("inf"),
       "", "0", "1.5", "abc", "ROOTS", "Screws", " centrifugal", [], [1], {}, {"a": 1}, 10**400,
       12345678.5, -2.0, 0.995, 1.005, 0.0049999]


def phrase_case(report):
    out = {"report": enc(report)}
    for name, fn in (("psi", psi), ("bar", bar)):
        try:
            out[name] = {"ok": ir.report_phrase(report, fn)}
        except Exception as exc:  # noqa: BLE001
            out[name] = {"err": type(exc).__name__, "msg": str(exc)}
    return out


def main():
    rnd = random.Random(7823)
    texts = ["NONE", "DONE", "ERROR:boom", "FI:", "FI:{truncated", "FI:[1,2]", "SC:{}", "FI:{}",
             "FI:null", "FI:1", 'FI:"x"', "fi:{}", " FI:{}", "FI: {} ", 'FI:{"a":NaN}',
             "FI:" + json.dumps({"torqueCoef": 1.9, "turbo": TURBO, "supercharger": SUPERCHARGER}),
             'FI:{"a":1,"a":2}', "FI:{}x", "FI:" + "[" * 3 + "]" * 3]
    replies = []
    for t in texts:
        r = ir.parse_reply(t)
        replies.append({"text": t, "out": enc(r)})

    reports = [
        {"torqueCoef": 1.473, "damaged": False, "supercharger": dict(SUPERCHARGER)},
        {"torqueCoef": 0.974, "damaged": False, "turbo": dict(TURBO)},
        {"torqueCoef": 1.9, "damaged": False, "twoStep": True, "turbo": {**TURBO, "boost": 10.0},
         "supercharger": SUPERCHARGER},
        {}, {"torqueCoef": 1}, {"turbo": None}, {"turbo": {}}, {"supercharger": {}},
        {"turbo": 1}, {"supercharger": "x"}, {"turbo": [1]}, {"twoStep": False}, {"twoStep": None},
        {"twoStep": 0}, {"damaged": "yes"}, {"torqueCoef": None}, {"torqueCoef": "1.2"},
        {"torqueCoef": float("nan")}, {"torqueCoef": float("inf")}, {"torqueCoef": 10**400},
        {"turbo": {**TURBO, "maxTurboRPM": "0"}}, {"turbo": {**TURBO, "heatLimit": "0"}},
        {"turbo": {**TURBO, "heat": None}}, {"turbo": {**TURBO, "hot": True, "heat": 715.0}},
        {"turbo": {k: v for k, v in TURBO.items() if k != "peakCoef"}},
        {"turbo": {**TURBO, "peakRPM": None}}, {"supercharger": {**SUPERCHARGER, "type": None}},
        {"supercharger": {**SUPERCHARGER, "type": "ROOTS", "twisted": 1}},
        {"supercharger": {**SUPERCHARGER, "type": 5}},
        {"torqueCoef": 1.005}, {"torqueCoef": 0.995}, {"torqueCoef": 1.015}, {"torqueCoef": 0.985},
        {"turbo": {**TURBO, "turboRPM": 125000, "boost": -2.0}},
        {"turbo": {**TURBO, "maxTurboRPM": 0, "turboRPM": 100}},
    ]
    for _ in range(250):
        base = rnd.choice([dict(TURBO), dict(SUPERCHARGER)])
        for _ in range(rnd.randint(1, 3)):
            base[rnd.choice(list(base))] = rnd.choice(ODD)
        rep = {"torqueCoef": rnd.choice([1.0, 0.974, 1.9, rnd.choice(ODD)])}
        rep["turbo" if "turboRPM" in base else "supercharger"] = base
        if rnd.random() < 0.3:
            rep["supercharger" if "turbo" in rep else "turbo"] = dict(SUPERCHARGER if "turbo" in rep else TURBO)
        if rnd.random() < 0.4:
            rep["twoStep"] = rnd.choice(ODD)
        if rnd.random() < 0.4:
            rep["damaged"] = rnd.choice(ODD)
        reports.append(rep)

    write_fixture("induction_report", {
        "constants": {"REPLY_PREFIX": ir.REPLY_PREFIX, "TYPE_NAMES": [list(kv) for kv in ir.TYPE_NAMES.items()]},
        "parse_reply": replies,
        "report_phrase": [phrase_case(r) for r in reports],
    })


if __name__ == "__main__":
    main()
