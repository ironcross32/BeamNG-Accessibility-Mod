"""Golden cases for beam_reports::audio_debug, from audio_debug.py."""

from __future__ import annotations

import random

from _common import fbits, outcome, write_fixture

import audio_debug as ad


def pairs(d):
    return [[k, v] for k, v in d.items()]


def main():
    rnd = random.Random(4483)
    payloads = ["", ";", "a=1", "a=1;b=2", " a = 1 ;b= x=y ", "20;soundCount=2", "20;7;count=9",
                "count=3;5", "=v", "k=", "noeq", "a=1;;b=2", "a=1;a=2", " ; =;", "x=a=b=c",
                "key=mainGain;kind=number;futureField=x;label=Main gain;unit=dB;value=-3.5;min=-30;max=10;step=0.5"]
    parse = [{"payload": p, "out": pairs(ad.parse_fields(p))} for p in payloads]

    values = ["", "x", "0", "-0", "-0.0001", "0.004", "0.005", "-0.005", "0.015", "1", "-3.5", "2.675",
              "1e-7", "123456.789", "1e21", "nan", "inf", "-inf", "1_0", " 7 ", "0.5", "1.5", "2.5", "-2.5"]
    hz_values = ["", "x", "0", "20", "120", "382.35", "999.4", "999.5", "1000", "1044", "2500", "22000",
                 "99999", "1e9", "nan", "inf", "-50"]
    rows = []
    for unit in ("", "dB", "count", "Hz", "%"):
        for v in values if unit != "Hz" else hz_values:
            row = {"kind": "number", "label": "L", "unit": unit}
            row["hz" if unit == "Hz" else "value"] = v
            rows.append(row)
    rows += [
        {"kind": "number", "label": "Only label"}, {"kind": "number", "unit": "Hz", "value": "500"},
        {"kind": "select", "index": "1", "count": "2", "ref": "exhaust", "device": "mainEngine",
         "blend": "V8_cross_exhaust", "modified": "1"},
        {"kind": "select"}, {"kind": "select", "index": "x", "count": "3"},
        {"kind": "select", "index": "2", "count": ""}, {"kind": "select", "index": " 4 ", "count": "+9",
                                                         "ref": "", "device": "d", "modified": "0"},
        {"kind": "select", "index": "99999999999999999999", "count": "2"},
        {"kind": "action", "key": "restore", "label": "Restore vehicle defaults", "editable": "0"},
        {"kind": "action", "key": "restore", "label": "R", "editable": "1"},
        {"kind": "action", "key": "other", "label": "Other", "editable": "0"}, {}, {"label": "Bare"},
    ]
    row_lines = [{"row": pairs(r), **outcome(ad.row_line, r), "spoken": outcome(ad.spoken_value, r)}
                 for r in rows]

    step_rows = [
        {"kind": "number", "value": "382.3529", "min": "0", "max": "1000", "step": "10"},
        {"kind": "number", "value": "1000", "min": "0", "max": "1000", "step": "10"},
        {"kind": "number", "value": "5", "min": "0", "max": "1000", "step": "10"},
        {"kind": "number", "value": "0.65", "min": "0", "max": "1", "step": "0.01"},
        {"kind": "number", "value": "0.65", "step": "0"}, {"kind": "number", "value": "0.65", "step": "-0"},
        {"kind": "number", "value": "0.65", "step": "x"}, {"kind": "number", "value": "x", "step": "1"},
        {"kind": "number", "value": "1", "step": "nan"}, {"kind": "number", "value": "nan", "step": "1"},
        {"kind": "number", "value": "1", "step": "1", "min": "nan"},
        {"kind": "number", "value": "1", "step": "1", "max": "nan"},
        {"kind": "number", "value": "1", "step": "1", "min": "5", "max": "2"},
        {"kind": "number", "value": "0.00001", "step": "0.00001"},
        {"kind": "number", "value": "-30", "step": "0.5", "min": "-30", "max": "10"},
        {"kind": "select", "value": "1", "step": "1"}, {},
    ]
    for _ in range(25):
        lo = rnd.uniform(-100, 0)
        step_rows.append({"kind": "number", "value": repr(rnd.uniform(lo, 100)), "min": repr(lo),
                          "max": repr(rnd.uniform(0, 100)), "step": rnd.choice(["0.01", "0.5", "1", "10", "0.001"])})
    steps = []
    for r in step_rows:
        for delta in (-10, -1, 1, 10):
            out = ad.stepped_value(r, delta)
            steps.append({"row": pairs(r), "delta": delta, "out": None if out is None else fbits(out)})

    wire = [{"v": fbits(v), "out": ad.wire_number(v)} for v in
            [0.66, -3.5, 0.0, -0.0, 382.3529, 1e-5, 123456.0, 1234567.0, 0.1 + 0.2, float("nan"),
             float("inf"), 1e21, 999999.5]]

    select = []
    for r in [{"index": "1", "count": "2"}, {"index": "0", "count": "3"}, {"index": "0", "count": "1"},
              {}, {"index": "x"}, {"index": "5", "count": "3"}, {"index": "-4", "count": "3"},
              {"index": " 2", "count": "4 "}, {"count": "5"}, {"index": "2", "count": "0"}]:
        for delta in (-10, -1, 0, 1, 5):
            select.append({"row": pairs(r), "delta": delta, "out": ad.select_target(r, delta)})

    sfx = [{"row": pairs(r), "out": ad.sfx_line(r)} for r in [
        {"kind": "dir", "name": "UI", "count": "71"}, {"kind": "dir", "name": "DLC", "count": "1"},
        {"kind": "dir", "name": "X"}, {"kind": "dir", "name": "X", "count": ""}, {"kind": "file", "name": "n"},
        {}, {"kind": "dir"}]]

    write_fixture("audio_debug", {
        "parse_fields": parse, "row_line": row_lines, "stepped_value": steps,
        "wire_number": wire, "select_target": select, "sfx_line": sfx,
    })


if __name__ == "__main__":
    main()
