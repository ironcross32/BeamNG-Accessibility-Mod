"""Golden cases for beam_base::pyval: json.loads / json.dumps(indent=2, ensure_ascii=False)
and Python's str(), repr(), int(), float() and bool() on the values json produces."""

from __future__ import annotations

import json

from _common import fbits, outcome, write_fixture


def dumps(v):
    return json.dumps(v, indent=2, ensure_ascii=False)


LOADS = [
    "", " ", "null", "true", "false", "0", "-0", "01", "-", "1.", ".5", "1e", "1e5", "1E+2",
    "1e-7", "2.50", "-0.0", "1e16", "1e400", "-1e400", "123456789012345678901234567890",
    "9223372036854775807", "9223372036854775808", "-9223372036854775808",
    "-9223372036854775809", "18446744073709551616", "NaN", "Infinity", "-Infinity", "nan",
    "infinity", "+1", '"a"', '"\\u00e9\\/\\b\\f\\n\\r\\t\\"\\\\"', '"\\ud83d\\ude00"',
    '"\\u12"', '"\\q"', '"a\tb"', '"\x7f"', '"unterminated', "[]", "[1,]", "[1 2]", "{}",
    '{"a":1,}', "{'a': 1}", "{a: 1}", '{"a" 1}', '{"a": 1, "a": 2, "b": 3}',
    '{"b": 1, "a": [1, [2, {}], {"c": null}]}', " [1] x", "﻿{}", "[" * 50 + "]" * 50,
    '{"k": "  \x7f é"}', "\r\n\t {} \r\n", "1_0", "0x10", "--1", "1.5e+3",
    "100000000000000000000.0", "5e-324", "1.7976931348623157e308", "4.9e-325",
]

VALUES = [
    "null", "true", "false", "0", "1", "-1", "7", "-0", "123456789012345678901234567890",
    "-123456789012345678901234567890", "0.0", "-0.0", "3.7", "-3.7", "0.5", "1e-05", "1e16",
    "1e300", "-1e300", "NaN", "Infinity", "-Infinity", "1.0e2", '""', '" "', '"abc"', '"5"',
    '" 12 "', '"+5"', '"-0"', '"5.5"', '"1_0"', '"1__0"', '"_1"', '"1_"', '"007"', '"inf"',
    '"-Infinity"', '"nan"', '"1e400"', '"1e5"', '" 1.5 "', '"\\u00a012\\u2003"', '"0x10"',
    '"TRUE"', '"it\'s"', '"say \\"hi\\""', '"both \' and \\""', '"\\\\ back"',
    '"\\n\\r\\t\\u0001\\u007f\\u0080\\u00a0\\u00ad\\u00e9\\u2028\\u200b\\ue000\\ufeff"',
    '"\\ud83d\\ude00 \\udb40\\udc01"', "[]", "{}", '[1, "a", 2.5, null, true, [], {}]',
    '{"a": [1.5, null], "b": "x\'y", "c\'": {"d": -0.0}}', "[NaN, Infinity]",
    '"' + "9" * 4300 + '"', '"' + "9" * 4301 + '"', "9" * 30 + ".0",
]


def main():
    loads = []
    for text in LOADS:
        try:
            loads.append({"text": text, "ok": dumps(json.loads(text))})
        except Exception as exc:  # noqa: BLE001
            loads.append({"text": text, "err": type(exc).__name__})

    values = []
    for text in VALUES:
        v = json.loads(text)
        rec = {"json": text, "str": str(v), "repr": repr(v), "bool": bool(v)}
        res = outcome(int, v)
        if "ok" in res:
            res["ok"] = str(res["ok"])
        rec["int"] = res
        res = outcome(float, v)
        if "ok" in res:
            res["ok"] = fbits(res["ok"])
        rec["float"] = res
        values.append(rec)

    write_fixture("pyval", {"loads": loads, "values": values})


if __name__ == "__main__":
    main()
